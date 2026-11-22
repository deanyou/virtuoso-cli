#!/usr/bin/env bash
set -euo pipefail

export PYTHONDONTWRITEBYTECODE=1

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

TREES=(".claude/skills" ".agents/skills")

echo "== sync check: .agents/skills mirrors .claude/skills =="
if ! python3 scripts/sync_skills.py --check --strict-pending; then
    echo "ERROR: the skill trees have drifted; run 'python3 scripts/sync_skills.py'." >&2
    exit 1
fi
echo "   OK"

python3 -m unittest discover -s tests -p test_sync_skills.py

total_suites=0
total_failures=0

for tree in "${TREES[@]}"; do
    echo
    echo "######## $tree ########"

    echo "== syntax check: $tree/**/*.py =="
    find "$tree" -name "*.py" -print0 | xargs -0 -r python3 -c '
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

    for tests_dir in "$tree"/*/tests; do
        [ -d "$tests_dir" ] || continue
        skill_dir="$(dirname "$tests_dir")"
        suites=$((suites + 1))
        echo "== unittest discover: $skill_dir =="
        status=0
        out="$(cd "$skill_dir" && python3 -m unittest discover tests 2>&1)" || status=$?
        printf '%s\n' "$out"
        if [ "$status" -eq 0 ] && ! printf '%s\n' "$out" | grep -qE '^Ran [1-9][0-9]* tests?'; then
            echo "   ERROR: discovered 0 tests in $skill_dir"
            status=1
        fi
        if [ "$status" -eq 0 ]; then
            echo "   OK"
        else
            echo "   FAILED: $skill_dir"
            failures=$((failures + 1))
        fi
    done

    if [ "$suites" -eq 0 ]; then
        echo "ERROR: no $tree/*/tests directory found; refusing to pass." >&2
        exit 1
    fi

    if [ "$failures" -ne 0 ]; then
        echo "ERROR: $failures of $suites skill suite(s) failed in $tree." >&2
        exit 1
    fi

    echo "OK: $suites skill suite(s) passed in $tree."
    total_suites=$((total_suites + suites))
    total_failures=$((total_failures + failures))
done

echo
echo "OK: $total_suites skill suite(s) passed across ${#TREES[@]} tree(s)."
