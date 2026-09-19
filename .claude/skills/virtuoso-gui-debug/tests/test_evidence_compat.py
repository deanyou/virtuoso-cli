"""Tests for Evidence manifest backward compatibility."""
import sys, json, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.evidence import EvidenceManifest, SCHEMA_VERSION

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1


def read_manifest(dirpath: Path) -> dict:
    """Read and validate a manifest.json. Returns parsed dict."""
    raw = json.loads((dirpath / "manifest.json").read_text())
    sv = raw.get("schema_version")
    if sv is None:
        raise ValueError("missing schema_version")
    if sv > SCHEMA_VERSION:
        raise ValueError(f"unsupported schema_version={sv}, max={SCHEMA_VERSION}")
    # Required fields
    for f in ("run_id", "status", "started_at"):
        if f not in raw:
            raise ValueError(f"missing required field: {f}")
    # Unknown fields are preserved but ignored
    return raw


print("=== Test 1: Read schema_version=1 manifest ===")
tmp = Path(tempfile.mkdtemp())
m = EvidenceManifest(tmp, "t1", "s1", 1, ":0")
m.write_initial()
m.add_artifact("trace", "trace", tmp / "agent-actions.jsonl")
m.finalize("passed")
manifest = read_manifest(tmp)
check("schema_version=1", manifest["schema_version"] == 1)
check("status=passed", manifest["status"] == "passed")
check("has run_id", "run_id" in manifest)
check("has artifacts", "artifacts" in manifest)

print("\n=== Test 2: Missing optional fields use defaults ===")
# Simulate an old manifest without newer fields
old_manifest = {
    "schema_version": 1,
    "run_id": "old-run",
    "task_id": "t",
    "session": {"id": "s", "pid": 1, "display": ":0"},
    "status": "running",
    "started_at": "2026-01-01T00:00:00Z",
    "artifacts": [],
    "steps": [],
}
(tmp / "manifest.json").write_text(json.dumps(old_manifest))
parsed = read_manifest(tmp)
check("old manifest reads OK", parsed["status"] == "running")
check("missing finished_at is OK", "finished_at" not in parsed)
check("missing last_updated is OK", "last_updated" not in parsed)

print("\n=== Test 3: Unknown fields are ignored but preserved ===")
extended = dict(old_manifest)
extended["future_field"] = "new-value"
extended["experimental"] = {"nested": [1, 2, 3]}
(tmp / "manifest.json").write_text(json.dumps(extended))
parsed = read_manifest(tmp)
check("unknown field preserved", parsed.get("future_field") == "new-value")
check("unknown nested preserved", parsed.get("experimental", {}).get("nested") == [1, 2, 3])

print("\n=== Test 4: Incompatible version raises ===")
bad = dict(old_manifest)
bad["schema_version"] = 99
(tmp / "manifest.json").write_text(json.dumps(bad))
try:
    read_manifest(tmp)
    check("should raise", False)
except ValueError as e:
    check("raises on v99", "unsupported" in str(e))

print("\n=== Test 5: Three statuses readable ===")
for status in ("running", "failed", "passed"):
    tmp_s = Path(tempfile.mkdtemp())
    ms = EvidenceManifest(tmp_s, "t", "s", 1, ":0")
    ms.write_initial()
    if status == "passed":
        ms.finalize("passed")
    elif status == "failed":
        ms.fail("boom")
    # running stays as-is
    m_s = read_manifest(tmp_s)
    check(f"status={status} readable", m_s["status"] == status)

print("\n=== Test 6: Hash and path rules preserved ===")
(tmp / "manifest.json").write_text(json.dumps(old_manifest))
m6 = EvidenceManifest(tmp, "t", "s", 1, ":0")
# Path traversal should be rejected
outside = Path(tempfile.mkdtemp()) / "evil.png"
outside.write_text("x")
m6.add_artifact("evil", "screenshot", outside)
check("traversal rejected", "path escapes" in " ".join(m6._consistency_notes))

print("\n=== Test 7: Sensitive fields redacted ===")
tmp7 = Path(tempfile.mkdtemp())
m7 = EvidenceManifest(tmp7, "t", "s", 1, ":0")
m7.write_initial()
m7._session["token"] = "secret123"
m7.finalize("passed")
m7_dict = json.loads((tmp7 / "manifest.json").read_text())
check("token redacted", m7_dict["session"].get("token") == "<redacted>")

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
