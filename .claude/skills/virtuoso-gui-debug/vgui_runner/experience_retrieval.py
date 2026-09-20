"""Experience Retrieval — FTS5 + structured metadata search over verified cases.

Three pools:
  GOLD       — VERIFIER_CONFIRMED + PASS/recovery → recommend
  NEGATIVE   — VERIFIER_CONFIRMED + failed        → avoid repeating
  OBSERVATION — UNKNOWN / unverified              → analyze only, never auto-recommend

Safety gates:
  1. Determinism: same DB + same query → same ranking
  2. Verified-only default: unknown cases excluded from default pool
  3. Negative memory: verified failures retrievable
  4. Provenance preserved: every match traces to case → events → evidence
  5. No execution authority: returns recommendations only
  6. Fallback safe: no good match → NO_MATCH
"""

import json
import sqlite3
from pathlib import Path
from typing import Any, Dict, List, Optional

DB_PATH = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"


# ── FTS5 schema ────────────────────────────────────────────────────────

FTS_SCHEMA = """
CREATE VIRTUAL TABLE IF NOT EXISTS experience_cases_fts USING fts5(
    case_id UNINDEXED,
    problem_signature,
    failure_type,
    action_summary,
    outcome_summary,
    content=''
);
"""


def init_fts(conn) -> None:
    conn.executescript(FTS_SCHEMA)
    conn.commit()


def sync_fts(conn) -> int:
    """Rebuild FTS index from experience_cases. Returns rows indexed."""
    conn.execute("DELETE FROM experience_cases_fts")
    rows = conn.execute("""
        SELECT case_id, problem_signature, failure_type, action_sequence, outcome
        FROM experience_cases
    """).fetchall()
    n = 0
    for r in rows:
        action_summary = ""
        try:
            actions = json.loads(r["action_sequence"]) if r["action_sequence"] else []
            action_summary = " ".join(
                a.get("step_id", "") + " " + str(a.get("outcomes", {}))
                for a in actions
            )
        except Exception:
            pass
        conn.execute("""
            INSERT INTO experience_cases_fts
            (case_id, problem_signature, failure_type, action_summary, outcome_summary)
            VALUES (?, ?, ?, ?, ?)
        """, (r["case_id"], r["problem_signature"] or "", r["failure_type"] or "",
              action_summary, r["outcome"] or ""))
        n += 1
    conn.commit()
    return n


# ── Query Builder ──────────────────────────────────────────────────────

def _build_filter(
    failure_type: Optional[str] = None,
    route_channel: Optional[str] = None,
    case_type: Optional[str] = None,
    verification_status: Optional[str] = None,
    virtuoso_version: Optional[str] = None,
    verified_only: bool = True,
) -> tuple:
    """Build WHERE clause and params. Returns (where_sql, params)."""
    clauses = []
    params: list = []

    if verified_only:
        # Only verifier-confirmed cases enter default pool
        clauses.append("verification_status IS NOT NULL")
        clauses.append("verification_status != 'UNKNOWN'")

    if failure_type:
        clauses.append("failure_type LIKE ?")
        params.append(f"%{failure_type}%")

    if route_channel:
        clauses.append("channel = ?")
        params.append(route_channel)

    if case_type:
        clauses.append("case_type = ?")
        params.append(case_type)

    if verification_status:
        clauses.append("verification_status = ?")
        params.append(verification_status)

    where = " AND ".join(clauses) if clauses else "1=1"
    return where, params


# ── Main API ──────────────────────────────────────────────────────────

def search_experience(
    state: str,
    top_k: int = 5,
    verified_only: bool = True,
    failure_type: Optional[str] = None,
    route_channel: Optional[str] = None,
    case_type: Optional[str] = None,
    db_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """Search verified experience cases similar to current state.

    Returns:
        {
            "matches": [...],         # GOLD cases (success/recovery)
            "negative_matches": [...], # NEGATIVE cases (verified failed)
            "query_signature": "...",
            "pool_size": int,
        }
    """
    path = str(db_path) if db_path else str(DB_PATH)
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row

    # Ensure FTS exists
    try:
        init_fts(conn)
    except Exception:
        pass

    # Rebuild FTS from current cases (cheap for small DB)
    try:
        sync_fts(conn)
    except Exception:
        pass

    # ── GOLD pool: verified success/recovery ──
    where, params = _build_filter(
        failure_type=failure_type,
        route_channel=route_channel,
        case_type=case_type or "recovery",
        verified_only=verified_only,
    )

    gold_rows = conn.execute(f"""
        SELECT case_id, run_id, case_type, failure_type, problem_signature,
               outcome, verification_status, quality_score, event_refs, provenance
        FROM experience_cases
        WHERE {where} AND verification_status = 'PASSED'
        ORDER BY quality_score DESC, created_at DESC
        LIMIT ?
    """, params + [top_k]).fetchall()

    # Also try success cases if no recovery found
    if not gold_rows:
        where2, params2 = _build_filter(
            failure_type=failure_type,
            route_channel=route_channel,
            case_type="success",
            verified_only=verified_only,
        )
        gold_rows = conn.execute(f"""
            SELECT case_id, run_id, case_type, failure_type, problem_signature,
                   outcome, verification_status, quality_score, event_refs, provenance
            FROM experience_cases
            WHERE {where2} AND verification_status = 'PASSED'
            ORDER BY quality_score DESC, created_at DESC
            LIMIT ?
        """, params2 + [top_k]).fetchall()

    # ── NEGATIVE pool: verified failures ──
    neg_where, neg_params = _build_filter(
        failure_type=failure_type,
        route_channel=route_channel,
        verified_only=verified_only,
    )
    neg_rows = conn.execute(f"""
        SELECT case_id, run_id, case_type, failure_type, problem_signature,
               outcome, verification_status, quality_score, event_refs, provenance
        FROM experience_cases
        WHERE {neg_where} AND verification_status = 'FAILED'
        ORDER BY quality_score DESC, created_at DESC
        LIMIT ?
    """, neg_params + [top_k]).fetchall()

    # ── FTS text search (if state has keywords) ──
    fts_matches = {}
    if state and len(state.strip()) > 2:
        try:
            # Tokenize: use words as FTS5 OR query
            tokens = [t for t in state.lower().split() if len(t) > 2]
            if tokens:
                fts_query = " OR ".join(tokens[:5])
                fts_rows = conn.execute("""
                    SELECT c.case_id, c.problem_signature, c.failure_type,
                           c.verification_status, c.quality_score,
                           bm25(experience_cases_fts) as rank
                    FROM experience_cases_fts f
                    JOIN experience_cases c ON c.case_id = f.case_id
                    WHERE experience_cases_fts MATCH ?
                    ORDER BY rank
                    LIMIT ?
                """, (fts_query, top_k * 2)).fetchall()
                for r in fts_rows:
                    fts_matches[r["case_id"]] = r["rank"]
        except Exception:
            pass

    # ── Merge: structured + FTS ──
    def _row_to_match(r, pool):
        d = dict(r)
        # Score: quality_score * pool_weight + FTS bonus
        score = d.get("quality_score", 0.0)
        if d["case_id"] in fts_matches:
            score += 0.2  # FTS boost
        return {
            "case_id": d["case_id"],
            "score": round(score, 3),
            "case_type": d["case_type"],
            "failure_type": d["failure_type"],
            "problem_signature": d["problem_signature"],
            "verification_status": d["verification_status"],
            "outcome": d["outcome"],
            "evidence_refs": json.loads(d["event_refs"]) if d["event_refs"] else [],
            "pool": pool,
        }

    gold = [_row_to_match(r, "GOLD") for r in gold_rows]
    neg = [_row_to_match(r, "NEGATIVE") for r in neg_rows]

    # Deduplicate: GOLD takes precedence
    gold_ids = {g["case_id"] for g in gold}
    neg = [n for n in neg if n["case_id"] not in gold_ids]

    conn.close()

    query_sig = f"{state[:50]}:{failure_type or 'any'}:{route_channel or 'any'}:{case_type or 'any'}"

    return {
        "matches": gold[:top_k],
        "negative_matches": neg[:top_k],
        "query_signature": query_sig,
        "pool_size": len(gold) + len(neg),
        "has_results": len(gold) > 0 or len(neg) > 0,
    }


# ── CLI ────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import sys
    q = sys.argv[1] if len(sys.argv) > 1 else "window_not_ready"
    result = search_experience(q, top_k=5)
    print(f"Query: {q}")
    print(f"GOLD ({len(result['matches'])}):")
    for m in result["matches"]:
        print(f"  [{m['score']}] {m['case_type']:10s} {m['failure_type']} -> {m['verification_status']}")
    print(f"NEGATIVE ({len(result['negative_matches'])}):")
    for m in result["negative_matches"]:
        print(f"  [{m['score']}] {m['case_type']:10s} {m['failure_type']} -> {m['verification_status']}")
    if not result["has_results"]:
        print("  NO_MATCH")
