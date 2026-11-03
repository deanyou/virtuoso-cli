# Workflow patch — `skill-tests.yml` path filters

**Status:** NOT applied. `.github/workflows/` is owned and pushed separately by
the repository owner with a real `git`; this file only carries the required
change so it can be applied there.

**Apply with:**

```bash
git apply WORKFLOW-PATCH.md.patch      # or paste the diff below into a PR
```

## Why

`.github/scripts/run-skill-tests.sh` now covers **both** skill trees: it runs
the unittest suites under `.agents/skills` as well as `.claude/skills`, and it
runs `scripts/sync_skills.py --check` as a drift gate.

The workflow is path-filtered, so without the patch below a PR that touches
only `.agents/skills/**` (or only the sync tool) would never trigger the job
that now guards that tree — the new coverage would be dead code.

## The patch

```diff
--- a/.github/workflows/skill-tests.yml
+++ b/.github/workflows/skill-tests.yml
@@ -10,12 +10,16 @@
     branches: [main]
     paths:
       - ".claude/skills/**"
+      - ".agents/skills/**"
+      - "scripts/sync_skills.py"
       - ".github/workflows/skill-tests.yml"
       - ".github/scripts/run-skill-tests.sh"
   pull_request:
     branches: [main]
     paths:
       - ".claude/skills/**"
+      - ".agents/skills/**"
+      - "scripts/sync_skills.py"
       - ".github/workflows/skill-tests.yml"
       - ".github/scripts/run-skill-tests.sh"
```

## What each added filter buys

| Added path | Reason |
|---|---|
| `.agents/skills/**` | A Codex-side-only edit (a fix applied to the mirror but not yet to the canonical tree, or a new Codex-only skill) must re-run the Codex suites. |
| `scripts/sync_skills.py` | The sync tool owns the adaptation table and the manifest. Changing it can make an existing tree stale, so the drift gate must re-run. |

## Notes for the owner

- No change is needed to the job body: `bash .github/scripts/run-skill-tests.sh`
  already runs the script that was extended in this branch.
- The script's drift gate runs **before** the test suites, so a drifted tree
  fails fast with a single clear message
  (`ERROR: the skill trees have drifted; run 'python3 scripts/sync_skills.py'.`)
  instead of testing stale content and reporting a second, confusing failure.
- The sync gate is *not* fatal for the two skills still listed as
  **PENDING DECISION** (`vcli-architecture`, `virastack-auto-allow`): the tool
  reports them and leaves them untouched. Use `--strict-pending` if the owner
  wants pending items to fail the job.
