#!/usr/bin/env bash
# Syntax-check every skill script and run each skill's unittest suite.
#
# Used by .github/workflows/skill-tests.yml. Exits non-zero if any skill fails,
# or if no skill suite was found at all (a renamed/removed tests/ directory must
# not silently turn this job green).
set -euo pipefail

# Keep the working tree clean: importing skill modules must not litter
# __pycache__ directories (the repo tracks bytecode under .claude).
export PYTHONDONTWRITEBYTECODE=1

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

echo "== syntax check: .claude/skills/**/*.py =="
# AST parse rather than compileall: same syntax coverage, but writes no
# __pycache__ (the repo tracks a .pyc under .claude, so stray bytecode files
# would show up as working-tree noise).
find .claude/skills -name "*.py" -print0 | xargs -0 -r python3 -c '
import ast, sys
bad = 0
for path in sys.argv[1:]:
    try:
        ast.parse(open(path, "rb").read(), filename=path)
    except SyntaxError as e:
        print(f"   SyntaxError: {path}: {e}")
        bad += 1
sys.exit(1 if bad else 0)
' || exit 1
echo "   OK"

suites=0
failures=0

for tests_dir in .claude/skills/*/tests; do
    [ -d "$tests_dir" ] || continue
    skill_dir="$(dirname "$tests_dir")"
    suites=$((suites + 1))
    echo "== unittest discover: $skill_dir =="
    # Tests inject their own sys.path for the skill's scripts/ package, so run
    # from the skill directory rather than the repo root.
    if (cd "$skill_dir" && python3 -m unittest discover tests); then
        echo "   OK"
    else
        echo "   FAILED: $skill_dir"
        failures=$((failures + 1))
    fi
done

if [ "$suites" -eq 0 ]; then
    echo "ERROR: no .claude/skills/*/tests directory found; refusing to pass." >&2
    exit 1
fi

if [ "$failures" -ne 0 ]; then
    echo "ERROR: $failures of $suites skill suite(s) failed." >&2
    exit 1
fi

echo "OK: $suites skill suite(s) passed."
