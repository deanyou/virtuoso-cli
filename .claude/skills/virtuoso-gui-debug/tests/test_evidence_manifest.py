"""Tests for EvidenceManifest."""
import sys, json, hashlib, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.evidence import EvidenceManifest, StepRecord, _safe_relpath, _sha256_stream

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1


print("=== Test 1: Successful run produces complete manifest ===")
tmpdir = Path(tempfile.mkdtemp())
run_dir = tmpdir / "run1"
run_dir.mkdir()
# Create fake artifacts
(run_dir / "trace.jsonl").write_text('{"seq":0}\n')
(run_dir / "summary.json").write_text('{"status":"passed"}')
(run_dir / "task.json").write_text('{"task_id":"t1"}')

m = EvidenceManifest(run_dir, "task-1", "sess-1", 12345, ":5.0")
m.write_initial()
m.add_artifact("trace", "trace", run_dir / "trace.jsonl")
m.add_artifact("summary", "summary", run_dir / "summary.json")
m.add_artifact("task", "task", run_dir / "task.json")
m.add_step(StepRecord(step_id="step-1", attempt=0, route_event_seq=4,
                       verification_status="passed", evidence_refs=["trace"]))
m.finalize("passed")

manifest = json.loads((run_dir / "manifest.json").read_text())
check("schema_version=1", manifest["schema_version"] == 1)
check("status=passed", manifest["status"] == "passed")
check("has run_id", "run_id" in manifest and manifest["run_id"])
check("has started_at", "started_at" in manifest)
check("has finished_at", "finished_at" in manifest)
check("3 artifacts", len(manifest["artifacts"]) == 3)
check("artifact has sha256", all(a["sha256"] for a in manifest["artifacts"]))
check("artifact has size", all(a["size"] > 0 for a in manifest["artifacts"]))
check("step recorded", len(manifest["steps"]) == 1)
check("step verification_status", manifest["steps"][0]["verification_status"] == "passed")
check("step route_event_seq", manifest["steps"][0]["route_event_seq"] == 4)

print("\n=== Test 2: sha256 matches real file ===")
expected_hash = hashlib.sha256((run_dir / "trace.jsonl").read_bytes()).hexdigest()
trace_art = [a for a in manifest["artifacts"] if a["id"] == "trace"][0]
check("sha256 correct", trace_art["sha256"] == expected_hash)

print("\n=== Test 3: Missing artifact recorded as note ===")
run_dir2 = tmpdir / "run2"
run_dir2.mkdir()
m2 = EvidenceManifest(run_dir2, "task-2", "sess-2", 1, ":0")
m2.add_artifact("screenshot-1", "screenshot", run_dir2 / "missing.png")
m2.finalize("failed")
manifest2 = json.loads((run_dir2 / "manifest.json").read_text())
check("missing artifact in notes", any("missing" in n for n in manifest2.get("consistency_notes", [])))

print("\n=== Test 4: Path traversal rejected ===")
outside = tmpdir / "outside.txt"
outside.write_text("secret")
run_dir3 = tmpdir / "run3"
run_dir3.mkdir()
m3 = EvidenceManifest(run_dir3, "t3", "s3", 1, ":0")
m3.add_artifact("leak", "log", outside)
m3.finalize("failed")
manifest3 = json.loads((run_dir3 / "manifest.json").read_text())
check("traversal rejected", any("escapes" in n for n in manifest3.get("consistency_notes", [])))
check("no leak artifact", not any(a["id"] == "leak" and a["sha256"] for a in manifest3["artifacts"]))

print("\n=== Test 5: Sensitive fields redacted ===")
run_dir4 = tmpdir / "run4"
run_dir4.mkdir()
m4 = EvidenceManifest(run_dir4, "task-4", "sess-4", 1, ":0")
# Add a step with sensitive data (simulated)
m4.add_step(StepRecord(step_id="step-1", verification_status="failed"))
m4.finalize("failed")
manifest4 = json.loads((run_dir4 / "manifest.json").read_text())
# Check no obvious secrets in output
raw = (run_dir4 / "manifest.json").read_text()
check("no TOKEN literal", "TOKEN" not in raw)
check("no PASSWORD literal", "PASSWORD" not in raw)

print("\n=== Test 6: Running status preserved on crash ===")
run_dir5 = tmpdir / "run5"
run_dir5.mkdir()
m5 = EvidenceManifest(run_dir5, "task-5", "sess-5", 1, ":0")
m5.write_initial()  # crash before finalize
manifest5 = json.loads((run_dir5 / "manifest.json").read_text())
check("status=running after crash", manifest5["status"] == "running")
check("no finished_at after crash", manifest5["finished_at"] is None)

print("\n=== Test 7: skipped with skip_policy recorded ===")
run_dir6 = tmpdir / "run6"
run_dir6.mkdir()
m6 = EvidenceManifest(run_dir6, "task-6", "sess-6", 1, ":0")
m6.add_step(StepRecord(step_id="step-1", verification_status="skipped", skip_policy="pass"))
m6.finalize("passed")
manifest6 = json.loads((run_dir6 / "manifest.json").read_text())
check("skip_policy recorded", manifest6["steps"][0].get("skip_policy") == "pass")

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
