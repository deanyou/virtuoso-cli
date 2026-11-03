"""vgui_runner.evidence - Run evidence bundle manifest.

A stable, verifiable index of all artifacts produced during a run.
Not a file packer — references by relative path + sha256.

Contract:
- manifest.json exists from run start (status=running)
- Final manifest written atomically via temp file rename
- All paths are relative to run output dir (no .., no absolute)
- SHA-256 computed in streaming mode (no full file load)
- Sensitive fields redacted recursively
- Old summary.json remains alongside for backward compat
"""

import hashlib
import json
import os
import time
from typing import Any
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


SCHEMA_VERSION = 1


def normalized_scenario_hash(scenario_data: dict) -> str:
    """Compute sha256 of canonically-encoded JSON (sorted keys, no whitespace).

    Equivalent scenarios with different formatting produce the same hash.
    """
    canonical = json.dumps(scenario_data, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def _sha256_stream(path: Path, chunk_size: int = 65536) -> Tuple[str, int]:
    """Stream a file and compute (sha256_hex, size_bytes)."""
    h = hashlib.sha256()
    size = 0
    with open(path, "rb") as f:
        while True:
            chunk = f.read(chunk_size)
            if not chunk:
                break
            h.update(chunk)
            size += len(chunk)
    return h.hexdigest(), size


def _safe_relpath(path: Path, root: Path) -> Optional[str]:
    """Return posix-style relative path if within root, else None.

    Rejects:
    - absolute paths outside root
    - path traversal (..)
    - symlinks pointing outside root
    """
    try:
        resolved = path.resolve()
        resolved_root = root.resolve()
        resolved.relative_to(resolved_root)
    except (ValueError, OSError):
        return None
    rel = resolved.relative_to(resolved_root)
    # No .. allowed
    if ".." in rel.parts:
        return None
    return rel.as_posix()


@dataclass
class Artifact:
    id: str
    kind: str  # trace | screenshot | summary | task | log
    path: str  # relative posix path
    sha256: str = ""
    size: int = 0

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "kind": self.kind,
            "path": self.path,
            "sha256": self.sha256,
            "size": self.size,
        }


@dataclass
class StepRecord:
    step_id: str
    attempt: int = 0
    route_event_seq: Optional[int] = None
    verification_status: Optional[str] = None
    skip_policy: Optional[str] = None
    evidence_refs: List[str] = field(default_factory=list)  # artifact ids

    def to_dict(self) -> dict:
        d = {
            "step_id": self.step_id,
            "attempt": self.attempt,
            "route_event_seq": self.route_event_seq,
            "verification_status": self.verification_status,
            "evidence_refs": list(self.evidence_refs),
        }
        if self.skip_policy:
            d["skip_policy"] = self.skip_policy
        return d


class EvidenceManifest:
    """Builds and writes manifest.json for a run."""

    def __init__(self, output_dir: Path, task_id: str, session_id: str,
                 pid: int, display: str):
        self._dir = output_dir
        self._run_id = f"run-{int(time.time())}-{os.urandom(3).hex()}"
        self._task_id = task_id
        self._session = {"id": session_id, "pid": pid, "display": display}
        self._started = datetime.now(timezone.utc).isoformat()
        self._finished: Optional[str] = None
        self._status = "running"
        self._scenario_sha256: str = ""
        self._artifacts: List[Artifact] = []
        self._steps: List[StepRecord] = []
        self._consistency_notes: List[str] = []
        self._last_updated = datetime.now(timezone.utc).isoformat()

    def set_scenario_hash(self, sha256_hex: str) -> None:
        self._scenario_sha256 = sha256_hex

    def add_artifact(self, artifact_id: str, kind: str, abs_path: Path) -> None:
        """Register an artifact. Computes sha256 + size if file exists."""
        rel = _safe_relpath(abs_path, self._dir)
        if rel is None:
            self._consistency_notes.append(
                f"artifact {artifact_id} path escapes run dir: {abs_path}"
            )
            return
        if not abs_path.exists():
            self._consistency_notes.append(f"artifact missing: {artifact_id} at {rel}")
            self._artifacts.append(Artifact(id=artifact_id, kind=kind, path=rel))
            return
        digest, size = _sha256_stream(abs_path)
        self._artifacts.append(Artifact(
            id=artifact_id, kind=kind, path=rel, sha256=digest, size=size,
        ))

    def add_step(self, record: StepRecord) -> None:
        self._steps.append(record)

    def finalize(self, status: str, consistency_notes: Optional[List[str]] = None) -> None:
        """Mark run finished and write final manifest atomically."""
        self._status = status
        self._finished = datetime.now(timezone.utc).isoformat()
        if consistency_notes:
            self._consistency_notes.extend(consistency_notes)
        self._write()

    def fail(self, reason: str) -> None:
        self._status = "failed"
        self._consistency_notes.append(reason)
        self._finished = datetime.now(timezone.utc).isoformat()
        try:
            self._write()
        except Exception:
            pass  # best-effort on failure path

    def _write(self) -> None:
        """Write manifest.json atomically via temp file rename."""
        self._last_updated = datetime.now(timezone.utc).isoformat()
        manifest = {
            "schema_version": SCHEMA_VERSION,
            "run_id": self._run_id,
            "task_id": self._task_id,
            "scenario_sha256": self._scenario_sha256,
            "session": self._session,
            "status": self._status,
            "started_at": self._started,
            "finished_at": self._finished,
            "last_updated": self._last_updated,
            "artifacts": [a.to_dict() for a in self._artifacts],
            "steps": [s.to_dict() for s in self._steps],
        }
        if self._consistency_notes:
            manifest["consistency_notes"] = self._consistency_notes

        # Redact sensitive keys recursively
        manifest = _redact_sensitive(manifest)

        tmp = self._dir / ".manifest.json.tmp"
        final = self._dir / "manifest.json"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(manifest, f, separators=(",", ":"))
        tmp.replace(final)

    def write_initial(self) -> None:
        """Write initial manifest (status=running) at run start."""
        self._write()


_SENSITIVE = frozenset({
    "password", "token", "secret", "key", "auth", "credential",
    "cookie", "api_key", "private", "env",
})


def _redact_sensitive(value: Any, depth: int = 0) -> Any:
    if depth > 6:
        return "<max_depth>"
    if value is None or isinstance(value, (bool, int, float)):
        return value
    if isinstance(value, str):
        if len(value) > 300:
            return value[:100] + "..."
        return value
    if isinstance(value, dict):
        out = {}
        for k, v in value.items():
            if isinstance(k, str) and k.lower() in _SENSITIVE:
                out[k] = "<redacted>"
            else:
                out[k] = _redact_sensitive(v, depth + 1)
        return out
    if isinstance(value, (list, tuple)):
        return [_redact_sensitive(item, depth + 1) for item in value[:50]]
    return f"<{type(value).__name__}>"
