"""vgui_runner.empirical_calibration - P0.5 经验校准。

用历史 Verifier 结果把硬编码 confidence 变成经验概率：

- 数据源：experience_events（value_source = 'VERIFIER_CONFIRMED'，
  outcome IN ('PASSED', 'FAILED')）。
- 分组：exact（route × failure × retry_count）→ channel_failure →
  channel → global，按序回退；每个分组样本量 < n_min 时向更粗分组回退。
- 平滑：Beta(α, β)，默认 α=β=1（Laplace）：1 PASS / 0 FAIL → 2/3 ≈ 0.667，
  而不是 1.00 —— 稀疏样本不得给出极端置信度。
- 门槛：n_min 以下一律不解释为概率（调用方降级为 RULE / UNKNOWN）。
- 确定性：同一 DB + 同一查询 → 同一结果；结果只依赖聚合计数。
- DB 读取用 EAFP：DB 缺失/损坏时 load_calibration() 返回 None，调用方优雅降级。
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Optional, Tuple

# 与冻结版同日；数据分布变化时应显式换版本（calibration_version 独立管理）。
CALIBRATION_VERSION = "2026-09-22"

# 技能包 data/ 下的 Experience DB（与 engine 写回路径一致）。
DEFAULT_DB_PATH = Path(__file__).resolve().parent.parent / "data" / "skill_db.sqlite3"


@dataclass(frozen=True)
class EmpiricalEstimate:
    """一次经验估计：概率 + 样本量 + 置信区间 + 来源。"""

    probability: float
    sample_count: int
    successes: int
    failures: int
    ci_low: float
    ci_high: float
    grouping_level: str  # exact | channel_failure | channel | global
    calibration_version: str


def wilson_interval(successes: int, n: int, z: float = 1.96) -> Tuple[float, float]:
    """Wilson score interval for a binomial proportion（标准库可实现，无依赖）。"""
    if n <= 0:
        return (0.0, 1.0)
    p = successes / n
    denom = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denom
    margin = z * (p * (1 - p) / n + z * z / (4 * n * n)) ** 0.5 / denom
    lo = max(0.0, centre - margin)
    hi = min(1.0, centre + margin)
    return (lo, hi)


@dataclass
class EmpiricalCalibration:
    """分层经验校准表。

    key 约定：channel / failure_type 均归一化为字符串，None 视为空串。
    exact key = (channel, failure_type, retry_count)。
    """

    exact: Dict[Tuple[str, str, int], Tuple[int, int]] = field(default_factory=dict)
    channel_failure: Dict[Tuple[str, str], Tuple[int, int]] = field(default_factory=dict)
    channel: Dict[str, Tuple[int, int]] = field(default_factory=dict)
    global_stats: Tuple[int, int] = (0, 0)
    n_min: int = 5
    alpha: float = 1.0
    beta: float = 1.0
    version: str = CALIBRATION_VERSION

    def estimate(
        self,
        channel: Optional[str],
        failure_type: Optional[str],
        retry_count: int,
    ) -> Optional[EmpiricalEstimate]:
        """按 exact → channel_failure → channel → global 回退。

        首个样本量 ≥ n_min 的分组胜出；全部不足时返回 None（降级）。
        """
        ch = channel or ""
        fail = failure_type or ""
        candidates = (
            ("exact", self.exact.get((ch, fail, retry_count))),
            ("channel_failure", self.channel_failure.get((ch, fail))),
            ("channel", self.channel.get(ch)),
            ("global", self.global_stats),
        )
        for level, stats in candidates:
            if stats is None:
                continue
            succ, fail_n = stats
            n = succ + fail_n
            if n >= self.n_min:
                return self._to_estimate(level, succ, fail_n)
        return None

    def _to_estimate(self, level: str, succ: int, fail_n: int) -> EmpiricalEstimate:
        n = succ + fail_n
        # Beta 平滑：稀疏样本不给出 0.0 / 1.0 极端置信度。
        probability = (succ + self.alpha) / (n + self.alpha + self.beta)
        ci_low, ci_high = wilson_interval(succ, n)
        return EmpiricalEstimate(
            probability=probability,
            sample_count=n,
            successes=succ,
            failures=fail_n,
            ci_low=ci_low,
            ci_high=ci_high,
            grouping_level=level,
            calibration_version=self.version,
        )


def load_calibration(
    db_path: Optional[Path] = None,
    n_min: int = 5,
    alpha: float = 1.0,
    beta: float = 1.0,
    version: str = CALIBRATION_VERSION,
) -> Optional[EmpiricalCalibration]:
    """从 Experience DB 构建校准表。

    只统计 VERIFIER_CONFIRMED 且 outcome 为 PASSED / FAILED 的事件；
    UNKNOWN / UNAVAILABLE 不进入真值池。DB 不可读时返回 None。
    """
    path = Path(db_path) if db_path else DEFAULT_DB_PATH
    try:
        conn = sqlite3.connect(str(path))
        try:
            rows = conn.execute(
                "SELECT channel, failure_type, attempt, outcome "
                "FROM experience_events "
                "WHERE value_source = 'VERIFIER_CONFIRMED' "
                "AND outcome IN ('PASSED', 'FAILED')"
            ).fetchall()
        finally:
            conn.close()
    except Exception:
        return None

    exact: Dict[Tuple[str, str, int], Tuple[int, int]] = {}
    channel_failure: Dict[Tuple[str, str], Tuple[int, int]] = {}
    channel: Dict[str, Tuple[int, int]] = {}
    g_succ = 0
    g_fail = 0

    for row in rows:
        ch = row[0] or ""
        failure = row[1] or ""
        attempt = int(row[2] or 0)
        succ = 1 if row[3] == "PASSED" else 0
        fail = 1 if row[3] == "FAILED" else 0

        exact_key = (ch, failure, attempt)
        s0, f0 = exact.get(exact_key, (0, 0))
        exact[exact_key] = (s0 + succ, f0 + fail)

        cf_key = (ch, failure)
        s1, f1 = channel_failure.get(cf_key, (0, 0))
        channel_failure[cf_key] = (s1 + succ, f1 + fail)

        s2, f2 = channel.get(ch, (0, 0))
        channel[ch] = (s2 + succ, f2 + fail)

        g_succ += succ
        g_fail += fail

    return EmpiricalCalibration(
        exact=exact,
        channel_failure=channel_failure,
        channel=channel,
        global_stats=(g_succ, g_fail),
        n_min=n_min,
        alpha=alpha,
        beta=beta,
        version=version,
    )


def empirical_to_dict(est: EmpiricalEstimate) -> dict:
    """序列化为 JSON-safe trace details（随决策写入 trace 的 support 信息）。"""
    return {
        "probability": round(est.probability, 4),
        "sample_count": est.sample_count,
        "successes": est.successes,
        "failures": est.failures,
        "ci_low": round(est.ci_low, 4),
        "ci_high": round(est.ci_high, 4),
        "grouping_level": est.grouping_level,
        "calibration_version": est.calibration_version,
    }
