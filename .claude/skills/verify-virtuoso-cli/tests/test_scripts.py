"""Contract tests for the verify-virtuoso-cli helper scripts.

The two helpers under test are shell scripts. These tests stub `vcli` with a
tiny fake binary in a temp dir, so no product build, toolchain, Virtuoso
instance or network is involved. The fake proves the *scripts'* contract
(verdict shape, exit-code pass-through, artifact hygiene) — it proves nothing
about the real vcli build, which is what a verification run is for.

Both helpers locate the binary as `<repo>/target/debug/vcli`:
  - doctor.sh takes the repo root as its first argument;
  - probe.sh reads the repo root from $VCLI_REPO.
"""

import glob
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SKILL_DIR = Path(__file__).resolve().parents[1]
DOCTOR = SKILL_DIR / "scripts" / "doctor.sh"
PROBE = SKILL_DIR / "scripts" / "probe.sh"

# Fake vcli. Every branch prints a JSON document and exits 0 unless the
# matching FAKE_*_RC says otherwise, so a test can shape the response it
# needs without a second fixture.
FAKE_VCLI = """#!/usr/bin/env bash
# Fake vcli used by the verify-virtuoso-cli script tests.

if [[ " $* " == *" --version "* ]]; then
  echo "vcli 0.0.0-fake"
  exit 0
fi

config_json="${FAKE_CONFIG_JSON:-}"
[[ -n "$config_json" ]] || config_json='{"status":"ok","valid":true,"entries":[],"diagnostics":[]}'
session_json="${FAKE_SESSION_JSON:-}"
[[ -n "$session_json" ]] || session_json='{"count":0,"sessions":[]}'
bridge_json="${FAKE_BRIDGE_JSON:-}"
[[ -n "$bridge_json" ]] || bridge_json='{"daemon_responsive":false,"skill_eval_ok":false}'
default_json="${FAKE_DEFAULT_JSON:-}"
[[ -n "$default_json" ]] || default_json='{"status":"ok"}'

args=" $* "
case "$args" in
  *" config check "*)
    printf '%s' "$config_json"
    exit "${FAKE_CONFIG_RC:-0}"
    ;;
  *" session list "*)
    printf '%s' "$session_json"
    exit "${FAKE_SESSION_RC:-0}"
    ;;
  *" tunnel diagnose "*)
    printf '%s' "$bridge_json"
    exit "${FAKE_BRIDGE_RC:-0}"
    ;;
esac

printf '%s' "$default_json"
exit "${FAKE_DEFAULT_RC:-0}"
"""

WARNING_CONFIG = {
    "status": "warn",
    "valid": True,
    "entries": [],
    "diagnostics": [
        {
            "code": "LEGACY_ENV_PROFILE",
            "level": "warning",
            "field": "profile",
            "message": "VB_PROFILE= is a deprecated fallback",
        }
    ],
}

BROKEN_CONFIG = {
    "status": "error",
    "valid": False,
    "entries": [],
    "diagnostics": [
        {
            "code": "ILLEGAL_CONFIG",
            "level": "error",
            "field": "VB_PORT",
            "message": "port is not a number",
        }
    ],
}


def work_dirs():
    """Every /tmp/vcli-doctor-work-* directory currently on disk."""
    found = set()
    for root in ("/tmp", tempfile.gettempdir()):
        found.update(glob.glob(os.path.join(root, "vcli-doctor-work-*")))
    return found


@unittest.skipUnless(os.name == "posix", "the helper scripts are POSIX shell scripts")
class HelperScriptTestCase(unittest.TestCase):
    """Shared fake-repo fixture: <repo>/target/debug/vcli is the fake."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="verify-virtuoso-cli-tests-")
        self.root = Path(self.temporary.name).resolve()
        self.repo = self.root / "repo"
        (self.repo / "target" / "debug").mkdir(parents=True)
        self.vcli = self.repo / "target" / "debug" / "vcli"
        self.vcli.write_text(FAKE_VCLI, encoding="utf-8")
        self.vcli.chmod(0o755)
        # Start from a clean env: an ambient VB_HOME would make the helpers
        # reuse the caller's data dir instead of their throwaway one.
        self.env = dict(
            (key, value)
            for key, value in os.environ.items()
            if not key.startswith("VB_") and not key.startswith("VCLI_")
        )

    def tearDown(self):
        self.temporary.cleanup()


class DoctorScriptTests(HelperScriptTestCase):
    def run_doctor(self, **fake_env):
        env = dict(self.env)
        env.update(fake_env)
        return subprocess.run(
            ["bash", str(DOCTOR), str(self.repo)],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
            timeout=60,
        )

    def verdict_of(self, result):
        self.assertTrue(result.stdout.strip(), "doctor.sh printed no verdict: %s" % result.stderr)
        return json.loads(result.stdout.strip().splitlines()[-1])

    def test_warning_level_config_diagnostic_still_allows_tier_a(self):
        # `config check` exits 0 with a warning-level diagnostic; the skill's
        # own rule says tier A is drivable iff identity is ok and config
        # check exits 0.
        result = self.run_doctor(FAKE_CONFIG_JSON=json.dumps(WARNING_CONFIG))
        self.assertEqual(result.returncode, 0, result.stderr)
        verdict = self.verdict_of(result)
        self.assertEqual(verdict["tier_a"], "drivable")
        self.assertEqual(verdict["probes"]["config"], "ok")
        # The diagnostic must not be lost: level and code are in the reason.
        self.assertIn("warning", verdict["reason"])
        self.assertIn("LEGACY_ENV_PROFILE", verdict["reason"])

    def test_failing_config_check_makes_tier_a_not_drivable(self):
        result = self.run_doctor(
            FAKE_CONFIG_JSON=json.dumps(BROKEN_CONFIG),
            FAKE_CONFIG_RC="10",
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        verdict = self.verdict_of(result)
        self.assertEqual(verdict["tier_a"], "not drivable")
        self.assertEqual(verdict["probes"]["config"], "failed")
        self.assertIn("ILLEGAL_CONFIG", verdict["reason"])

    def test_no_work_directory_is_left_behind(self):
        before = work_dirs()
        result = self.run_doctor()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            work_dirs() - before,
            set(),
            "doctor.sh leaked its work directory",
        )


class ProbeScriptTests(HelperScriptTestCase):
    def setUp(self):
        HelperScriptTestCase.setUp(self)
        self.evidence = self.root / "evidence"

    def run_probe(self, *arguments, **fake_env):
        env = dict(self.env)
        env["VCLI_REPO"] = str(self.repo)
        env.update(fake_env)
        return subprocess.run(
            ["bash", str(PROBE), str(self.evidence), "sample", "--"] + list(arguments),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
            timeout=60,
        )

    def test_stale_stdout_json_is_removed_when_stdout_is_not_json(self):
        first = self.run_probe("init", FAKE_DEFAULT_JSON='{"status":"created"}')
        self.assertEqual(first.returncode, 0, first.stderr)
        out_dir = self.evidence / "sample"
        self.assertTrue((out_dir / "stdout.json").is_file())
        self.assertEqual(
            json.loads((out_dir / "stdout.json").read_text(encoding="utf-8")),
            {"status": "created"},
        )

        # Same slug, so the same output dir: the previous run's success JSON
        # must not survive as evidence for a run that produced no JSON.
        second = self.run_probe("init", FAKE_DEFAULT_JSON="this is not json")
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertFalse(
            (out_dir / "stdout.json").exists(),
            "stale stdout.json survived a non-JSON run",
        )
        self.assertEqual(
            (out_dir / "stdout.txt").read_text(encoding="utf-8"), "this is not json"
        )

    def test_probed_exit_code_is_passed_through(self):
        result = self.run_probe("init", FAKE_DEFAULT_RC="5")
        self.assertEqual(result.returncode, 5, result.stderr)
        exit_code = self.evidence / "sample" / "exit-code.txt"
        self.assertEqual(exit_code.read_text(encoding="utf-8").strip(), "5")

    def test_side_effects_report_says_its_diff_is_filename_only(self):
        result = self.run_probe("init")
        self.assertEqual(result.returncode, 0, result.stderr)
        side_effects = (self.evidence / "sample" / "side-effects.txt").read_text(
            encoding="utf-8"
        )
        self.assertIn("file additions and removals only", side_effects)
        self.assertIn("checked manually", side_effects)


if __name__ == "__main__":
    unittest.main()
