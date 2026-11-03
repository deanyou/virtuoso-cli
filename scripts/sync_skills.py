#!/usr/bin/env python3
"""Mirror the Claude skill tree into the Codex skill tree.

The repository keeps two copies of every agent skill:

    .claude/skills/    canonical tree (Claude Code host)
    .agents/skills/    Codex host

``.agents`` holds real files, not symlinks, so the two trees drift: files added
to the canonical tree never reach the Codex side, and files edited there keep
stale bodies.  This tool re-establishes the mirror -- but deliberately *not*
byte-for-byte, because a few SKILL.md differences between the trees are
intentional host adaptations that a blind copy would erase.

Everything the tool knows lives in three explicit tables at the top of this
file:

    ADAPTATIONS      host adaptations applied to the canonical text
    SKILL_RULES      per-skill disposition (mirror / link / pending / exclude)
    FILE_EXCLUSIONS  files that must never be mirrored

Nothing is inferred at runtime.  A skill whose disposition is not listed is
mirrored with the default adaptation set; a skill whose disposition is
``pending`` is reported and left untouched rather than guessed at.

Usage:

    python3 scripts/sync_skills.py             # mirror .claude -> .agents
    python3 scripts/sync_skills.py --check     # report drift, exit 1 if any
    python3 scripts/sync_skills.py --dry-run   # print the plan, write nothing
    python3 scripts/sync_skills.py --report    # print the tables and verdict

Standard library only; no third-party imports; Python 3.9 compatible.
"""

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Dict, List, Optional, Sequence, Tuple

SOURCE_DIR = ".claude/skills"
TARGET_DIR = ".agents/skills"

# Files whose adaptation is ``mirror`` are copied verbatim; only SKILL.md
# carries host adaptations (see ADAPTATIONS for the evidence behind that).
SKILL_FILENAME = "SKILL.md"


# --------------------------------------------------------------------------
# Host adaptation table
# --------------------------------------------------------------------------
#
# Each entry is a rule with an id, a human summary and a pure function
# ``(text) -> (new_text, hit_count)``.  ``hit_count`` is how many places the
# rule actually rewrote, so the report can show which rules are live.
#
# Scope note: every rule below applies to ``<skill>/SKILL.md`` only.  The
# non-SKILL.md files of the canonical tree are copied byte-for-byte, because
# the observed drift shows the trees were *not* adapted there: e.g.
# ``.claude/skills/_shared/vcli.sh`` and ``.agents/skills/_shared/vcli.sh`` are
# byte-identical even though both mention ``.claude/skills/``.


@dataclass(frozen=True)
class Adaptation:
    """One host adaptation rule."""

    id: str
    summary: str
    applies_to: Callable[[str], bool]
    apply: Callable[[str], Tuple[str, int]]
    scope: str = "skill-md"

    def matches(self, rel_path: str) -> bool:
        return self.applies_to(rel_path)


# -- frontmatter helpers ----------------------------------------------------

FRONTMATTER_RE = re.compile(r"\A---[ \t]*\r?\n(.*?)\r?\n---[ \t]*(?:\r?\n|\Z)", re.S)

# Bash(<command> *) -- a bare command name, no path separator.
BASH_GLOB_RE = re.compile(r"Bash\(([^()\s]+) \*\)")

# The commands whose glob the Codex tree carries with a "*/" path prefix.
#
# Induced from the tree itself: every SKILL.md that the Codex side adapted
# prefixes exactly one command -- "virtuoso" -- e.g.
#     .claude  allowed-tools: Bash(virtuoso *) Read Write
#     .agents  allowed-tools: Bash(*/virtuoso *) Read Write
# and no Codex-tree file prefixes "cat", "ls", "jq", "grep", "find", "wc",
# "python", "python3", "vcli" or "spectre".  Restricting the rewrite to this
# list therefore reproduces the Codex convention that is already in the tree
# and changes nothing the tree currently leaves alone -- it cannot tighten an
# allow-list that is working today.
#
# Whether "vcli" and "spectre" belong here is a question about how the Codex
# matcher resolves a command, not something the repository records; widening
# this tuple is an explicit, one-line decision by the repository owner.
CODEX_GLOB_PREFIX_COMMANDS = ("virtuoso",)


def _split_frontmatter(text: str) -> Tuple[Optional[str], str]:
    """Return (frontmatter_block, remainder).  frontmatter is None when absent."""
    match = FRONTMATTER_RE.match(text)
    if not match:
        return None, text
    return match.group(1), text[match.end():]


def _join_frontmatter(frontmatter: Optional[str], rest: str) -> str:
    if frontmatter is None:
        return rest
    return "---\n" + frontmatter + "\n---\n" + rest


def _is_skill_md(rel_path: str) -> bool:
    return rel_path.split("/")[-1] == SKILL_FILENAME


def _strip_argument_hint(text: str) -> Tuple[str, int]:
    """Drop the ``argument-hint`` frontmatter field.

    ``argument-hint`` is a Claude Code UI field (the text shown next to the
    slash command in autocomplete).  The Codex skill validator accepts only
    ``name``, ``description``, ``license``, ``allowed-tools`` and
    ``metadata`` in the frontmatter, so carrying the field over makes the
    skill fail validation on the Codex side.

    Observed in the tree: the 6 SKILL.md files that still carry the field on
    the Codex side are exactly the 6 that the first bulk import copied
    verbatim (commit 83c9d35); every copy made afterwards dropped it.  The
    rule is applied uniformly here, which normalises those 6.
    """
    frontmatter, rest = _split_frontmatter(text)
    if frontmatter is None:
        return text, 0
    kept = []
    dropped = 0
    for line in frontmatter.split("\n"):
        if line.lstrip().startswith("argument-hint:"):
            dropped += 1
            continue
        kept.append(line)
    if not dropped:
        return text, 0
    return _join_frontmatter("\n".join(kept), rest), dropped


def _rewrite_allowed_tools_globs(text: str) -> Tuple[str, int]:
    """Rewrite ``Bash(cmd *)`` into ``Bash(*/cmd *)`` in the frontmatter.

    Only the ``allowed-tools`` frontmatter field is rewritten, and only for
    the commands in ``CODEX_GLOB_PREFIX_COMMANDS``.

    The scope matters.  The same strings also appear in skill *prose* -- the
    ``skill-review`` skill documents which glob form is correct for which
    host -- and rewriting those would corrupt the documentation and make the
    file contradict itself (it would print "CORRECT: Bash(*/virtuoso *)" on
    the line below "WRONG: Bash(*/virtuoso *)").
    """
    frontmatter, rest = _split_frontmatter(text)
    if frontmatter is None:
        return text, 0

    hits = 0
    kept = []
    for line in frontmatter.split("\n"):
        if line.lstrip().startswith("allowed-tools:"):
            def _replace(match: "re.Match[str]") -> str:
                nonlocal hits
                command = match.group(1)
                if "/" in command:
                    # Already path-qualified (e.g. "*/virtuoso"): leave it.
                    return match.group(0)
                if command not in CODEX_GLOB_PREFIX_COMMANDS:
                    # Not part of the observed Codex convention: leave it.
                    return match.group(0)
                hits += 1
                return "Bash(*/%s *)" % command
            line = BASH_GLOB_RE.sub(_replace, line)
        kept.append(line)
    if not hits:
        return text, 0
    return _join_frontmatter("\n".join(kept), rest), hits


def _literal_replace(old: str, new: str) -> Callable[[str], Tuple[str, int]]:
    """Build a rule function that replaces a literal substring."""
    def _apply(text: str) -> Tuple[str, int]:
        count = text.count(old)
        if not count:
            return text, 0
        return text.replace(old, new), count
    return _apply


ADAPTATIONS: Sequence[Adaptation] = (
    Adaptation(
        id="A1",
        summary="frontmatter: drop the Claude-only 'argument-hint' field",
        applies_to=_is_skill_md,
        apply=_strip_argument_hint,
        scope="frontmatter",
    ),
    Adaptation(
        id="A2",
        summary="frontmatter: rewrite 'Bash(cmd *)' to 'Bash(*/cmd *)' for Codex",
        applies_to=_is_skill_md,
        apply=_rewrite_allowed_tools_globs,
        scope="frontmatter",
    ),
    Adaptation(
        id="A3",
        summary="prose: 'Claude Code' -> 'Codex'",
        applies_to=_is_skill_md,
        apply=_literal_replace("Claude Code", "Codex"),
        scope="text",
    ),
    Adaptation(
        id="A4",
        summary="prose: 'Claude' -> 'Codex'",
        applies_to=_is_skill_md,
        apply=_literal_replace("Claude", "Codex"),
        scope="text",
    ),
    Adaptation(
        id="A5",
        summary="prose: '.claude/' -> '.Codex/' (skill-root references)",
        applies_to=_is_skill_md,
        apply=_literal_replace(".claude/", ".Codex/"),
        scope="text",
    ),
    Adaptation(
        id="A6",
        summary="prose: 'CLAUDE.md' -> 'AGENTS.md' (host instruction file)",
        applies_to=_is_skill_md,
        apply=_literal_replace("CLAUDE.md", "AGENTS.md"),
        scope="text",
    ),
)


# --------------------------------------------------------------------------
# Manifest: per-skill disposition
# --------------------------------------------------------------------------
#
#   mirror   copy the skill into .agents, applying the adaptation table
#   link     .agents holds a relative symlink to the canonical directory, so
#            the two can never drift; the link target is verified, not copied
#   exclude  the skill belongs to the Claude host only and is never mirrored
#   pending  ownership is undecided -- the skill is NOT mirrored and NOT
#            touched; the tool reports it instead of guessing

MODE_MIRROR = "mirror"
MODE_LINK = "link"
MODE_EXCLUDE = "exclude"
MODE_PENDING = "pending"


@dataclass(frozen=True)
class SkillRule:
    """Declared disposition of one skill directory."""

    skill: str
    mode: str
    reason: str
    link_target: str = ""


SKILL_RULES: Dict[str, SkillRule] = {
    "pstack-vcli": SkillRule(
        skill="pstack-vcli",
        mode=MODE_LINK,
        reason=(
            "Already a relative symlink (.agents/skills/pstack-vcli -> "
            "../../.claude/skills/pstack-vcli). AGENTS.md states Codex must "
            "discover the same skill there, and docs/pstack-vcli-assessment.md "
            "records the symlink as the deliberate anti-drift mechanism. "
            "Copying it would reintroduce the drift it exists to prevent."
        ),
        link_target="../../.claude/skills/pstack-vcli",
    ),
    "vcli-architecture": SkillRule(
        skill="vcli-architecture",
        mode=MODE_PENDING,
        reason=(
            "Host-neutral governance skill (frontmatter has only name and "
            "description), but nothing in the repo states whether Codex "
            "should load it. Note: pstack-vcli/SKILL.md points at "
            ".claude/skills/vcli-architecture/SKILL.md, which exists for both "
            "hosts today only because pstack-vcli is a symlink. Deciding this "
            "one decides whether pstack-vcli may become a real copy."
        ),
    ),
    "virastack-auto-allow": SkillRule(
        skill="virastack-auto-allow",
        mode=MODE_PENDING,
        reason=(
            "Its subject is the Claude Code permission dialog ('ViraStack "
            "(Claude Code UI)'), which has no Codex counterpart, yet the "
            "mechanism underneath is host-neutral vcli GUI automation with a "
            "unittest suite. Both readings are defensible, so the tool does "
            "not guess."
        ),
    ),
}


# --------------------------------------------------------------------------
# Manifest: file-level exclusions
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class FileExclusion:
    """A file that must never be mirrored into the Codex tree."""

    id: str
    summary: str
    matches: Callable[[str], bool]


FILE_EXCLUSIONS: Sequence[FileExclusion] = (
    FileExclusion(
        id="F1",
        summary="__pycache__ directories are interpreter runtime artifacts",
        matches=lambda rel: "__pycache__" in rel.split("/"),
    ),
    FileExclusion(
        id="F2",
        summary="compiled .pyc files are bound to one interpreter build",
        matches=lambda rel: rel.endswith(".pyc"),
    ),
)


# --------------------------------------------------------------------------
# Plan / result model
# --------------------------------------------------------------------------


@dataclass
class FileAction:
    """One mirrored file."""

    rel_path: str
    source: Path
    target: Path
    status: str              # "created" | "updated" | "unchanged" | "pending" | "skipped"
    adaptations: List[str] = field(default_factory=list)
    detail: str = ""


@dataclass
class LinkAction:
    """One skill that is linked instead of copied."""

    skill: str
    target: Path
    status: str              # "ok" | "missing" | "wrong-target"
    detail: str = ""


@dataclass
class Drift:
    """One difference between the canonical tree and the expected mirror."""

    kind: str                # "missing" | "differs" | "unexpected" | "link" | "excluded-present"
    rel_path: str
    detail: str = ""


@dataclass
class SyncResult:
    actions: List[FileAction] = field(default_factory=list)
    links: List[LinkAction] = field(default_factory=list)
    drifts: List[Drift] = field(default_factory=list)
    pending: List[Tuple[str, str]] = field(default_factory=list)
    excluded: List[str] = field(default_factory=list)
    ignored: List[str] = field(default_factory=list)
    adaptation_hits: Dict[str, int] = field(default_factory=dict)

    @property
    def changed(self) -> List[FileAction]:
        return [a for a in self.actions if a.status in ("created", "updated")]

    @property
    def ok(self) -> bool:
        return not self.drifts


# --------------------------------------------------------------------------
# Core logic
# --------------------------------------------------------------------------


def repo_root() -> Path:
    """Repository root, derived from this file's location."""
    return Path(__file__).resolve().parent.parent


def _iter_source_files(source_root: Path) -> List[str]:
    """Relative paths of every regular file under the canonical tree.

    Symlinked directories are not followed: the canonical tree contains none,
    and following them would copy a file into itself.
    """
    found: List[str] = []
    for dirpath, dirnames, filenames in os.walk(source_root, followlinks=False):
        dirnames.sort()
        for name in sorted(filenames):
            full = Path(dirpath) / name
            if full.is_symlink():
                continue
            found.append(str(full.relative_to(source_root)).replace(os.sep, "/"))
    return found


def _skill_of(rel_path: str) -> str:
    return rel_path.split("/")[0]


def _rule_for(skill: str) -> Optional[SkillRule]:
    return SKILL_RULES.get(skill)


def _exclusion_for(rel_path: str) -> Optional[FileExclusion]:
    for exclusion in FILE_EXCLUSIONS:
        if exclusion.matches(rel_path):
            return exclusion
    return None


def transform_text(rel_path: str, text: str) -> Tuple[str, List[str]]:
    """Apply the adaptation table to one text blob.

    Frontmatter-scoped rules run on the frontmatter block only; text-scoped
    rules run over the whole document (frontmatter included), because e.g.
    ``author: Claude Code`` lives in the frontmatter.
    """
    applied: List[str] = []
    result = text
    for adaptation in ADAPTATIONS:
        if not adaptation.matches(rel_path):
            continue
        result, hits = adaptation.apply(result)
        if hits:
            applied.append("%s(x%d)" % (adaptation.id, hits))
    return result, applied


def expected_bytes(rel_path: str, source_bytes: bytes) -> Tuple[bytes, List[str]]:
    """Return the bytes the Codex tree should hold for one canonical file."""
    if not _is_skill_md(rel_path):
        # Non-SKILL.md files are copied byte-for-byte.
        return source_bytes, []
    try:
        text = source_bytes.decode("utf-8")
    except UnicodeDecodeError:
        return source_bytes, []
    new_text, applied = transform_text(rel_path, text)
    return new_text.encode("utf-8"), applied


def build_plan(root: Path) -> SyncResult:
    """Compare the canonical tree with the Codex tree."""
    result = SyncResult()
    source_root = root / SOURCE_DIR
    target_root = root / TARGET_DIR

    if not source_root.is_dir():
        raise SystemExit("error: canonical skill tree not found: %s" % source_root)

    expected: Dict[str, bytes] = {}

    for rel in _iter_source_files(source_root):
        rule = _rule_for(_skill_of(rel))
        if rule is not None and rule.mode == MODE_PENDING:
            result.pending.append((_skill_of(rel), rule.reason))
            continue
        if rule is not None and rule.mode in (MODE_EXCLUDE, MODE_LINK):
            # Excluded: belongs to the Claude host only.
            # Linked: the Codex side is a symlink to the canonical directory,
            # so there is nothing to copy -- the link is verified separately.
            result.excluded.append(rel)
            continue
        exclusion = _exclusion_for(rel)
        if exclusion is not None:
            result.excluded.append(rel)
            continue

        source_bytes = (source_root / rel).read_bytes()
        payload, applied = expected_bytes(rel, source_bytes)
        expected[rel] = payload
        for entry in applied:
            rule_id = entry.split("(")[0]
            result.adaptation_hits[rule_id] = result.adaptation_hits.get(rule_id, 0) + 1

        target = target_root / rel
        if not target.exists():
            result.drifts.append(Drift("missing", rel, "not present in the Codex tree"))
            result.actions.append(FileAction(rel, source_root / rel, target, "missing", applied))
        elif target.read_bytes() != payload:
            result.drifts.append(
                Drift("differs", rel, "content differs from the adapted canonical file")
            )
            result.actions.append(FileAction(rel, source_root / rel, target, "differs", applied))
        else:
            result.actions.append(FileAction(rel, source_root / rel, target, "unchanged", applied))

    # Files that exist in the Codex tree but have no canonical counterpart.
    if target_root.is_dir():
        for rel in _iter_source_files(target_root):
            if rel in expected:
                continue
            rule = _rule_for(_skill_of(rel))
            if rule is not None and rule.mode in (MODE_LINK, MODE_PENDING, MODE_EXCLUDE):
                continue
            exclusion = _exclusion_for(rel)
            if exclusion is not None:
                # Runtime artifacts (bytecode caches) in the Codex tree are
                # ignored, not drift: a local test run leaves them behind and
                # they must never be mirrored or committed.  The sync prunes
                # them; --check does not fail on them.
                result.ignored.append(rel)
                result.actions.append(
                    FileAction(rel, source_root / rel, target_root / rel, "ignored")
                )
                continue
            result.drifts.append(Drift("unexpected", rel, "no canonical counterpart"))
            result.actions.append(
                FileAction(rel, source_root / rel, target_root / rel, "unexpected")
            )

    # Linked skills: verify the symlink, never rewrite it.
    for skill, rule in sorted(SKILL_RULES.items()):
        if rule.mode != MODE_LINK:
            continue
        link_path = target_root / skill
        if not link_path.exists() and not link_path.is_symlink():
            result.links.append(LinkAction(skill, link_path, "missing", "symlink not present"))
            result.drifts.append(Drift("link", skill, "symlink not present in the Codex tree"))
            continue
        if not link_path.is_symlink():
            result.links.append(
                LinkAction(skill, link_path, "wrong-target", "expected a symlink, found a real directory")
            )
            result.drifts.append(
                Drift("link", skill, "expected a symlink to %s, found a real directory" % rule.link_target)
            )
            continue
        actual = os.readlink(link_path)
        if actual != rule.link_target:
            result.links.append(
                LinkAction(
                    skill, link_path, "wrong-target",
                    "symlink points at %s, expected %s" % (actual, rule.link_target),
                )
            )
            result.drifts.append(
                Drift("link", skill, "symlink points at %s, expected %s" % (actual, rule.link_target))
            )
        else:
            result.links.append(LinkAction(skill, link_path, "ok", ""))

    return result


def apply_plan(root: Path, result: SyncResult, prune: bool) -> int:
    """Write the plan to disk.  Returns the number of files written."""
    target_root = root / TARGET_DIR
    written = 0

    for action in result.actions:
        if action.status in ("unexpected", "ignored"):
            if not prune:
                continue
            if action.target.exists():
                action.target.unlink()
                action.status = "removed"
                written += 1
            continue
        if action.status not in ("missing", "differs"):
            continue
        action.target.parent.mkdir(parents=True, exist_ok=True)
        payload, _ = expected_bytes(action.rel_path, action.source.read_bytes())
        action.target.write_bytes(payload)
        action.status = "created" if action.status == "missing" else "updated"
        written += 1

    # Drop directories that became empty after pruning.
    if prune and target_root.is_dir():
        for dirpath, dirnames, filenames in os.walk(target_root, topdown=False):
            if Path(dirpath) == target_root:
                continue
            if not os.listdir(dirpath):
                os.rmdir(dirpath)

    return written


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def print_report(result: SyncResult, root: Path) -> None:
    """Print the classification tables and the verdict."""
    print("== adaptation table (%d rules) ==" % len(ADAPTATIONS))
    for adaptation in ADAPTATIONS:
        hits = result.adaptation_hits.get(adaptation.id, 0)
        print("   %-3s %-4s %s" % (adaptation.id, "[%d]" % hits if hits else "[0]", adaptation.summary))

    print()
    print("== skill manifest ==")
    for skill in sorted(SKILL_RULES):
        rule = SKILL_RULES[skill]
        print("   %-22s %-8s %s" % (skill, rule.mode, _first_sentence(rule.reason)))

    print()
    print("== file exclusions (%d) ==" % len(FILE_EXCLUSIONS))
    for exclusion in FILE_EXCLUSIONS:
        print("   %-3s %s" % (exclusion.id, exclusion.summary))

    if result.excluded:
        print()
        print("== files excluded from mirroring (%d) ==" % len(result.excluded))
        for rel in result.excluded:
            print("   %s" % rel)

    if result.pending:
        print()
        print("== PENDING DECISION (%d skills) ==" % len({s for s, _ in result.pending}))
        for skill, reason in sorted(set(result.pending)):
            print("   %s" % skill)
            for chunk in _wrap(reason, 72):
                print("      %s" % chunk)


def _first_sentence(text: str) -> str:
    """The first sentence of a reason, cut at a sentence boundary.

    A plain ``split(".")`` would cut inside ``.agents`` or ``.claude``, so the
    boundary is only accepted when it is followed by a space.
    """
    for index, char in enumerate(text):
        if char == "." and index + 1 < len(text) and text[index + 1] == " ":
            return text[: index + 1]
    return text


def _wrap(text: str, width: int) -> List[str]:
    words = text.split()
    lines: List[str] = []
    current = ""
    for word in words:
        if current and len(current) + 1 + len(word) > width:
            lines.append(current)
            current = word
        else:
            current = word if not current else current + " " + word
    if current:
        lines.append(current)
    return lines


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Mirror .claude/skills into .agents/skills, applying host adaptations."
    )
    parser.add_argument(
        "--check", action="store_true",
        help="report drift only; write nothing; exit 1 when the trees disagree",
    )
    parser.add_argument(
        "--dry-run", action="store_true",
        help="print what would be written, write nothing",
    )
    parser.add_argument(
        "--report", action="store_true",
        help="print the adaptation table, the manifest and the pending list",
    )
    parser.add_argument(
        "--no-prune", action="store_true",
        help="do not delete Codex-tree files that have no canonical counterpart",
    )
    parser.add_argument(
        "--strict-pending", action="store_true",
        help="treat pending-decision skills as a failure (exit 1)",
    )
    parser.add_argument(
        "--json", action="store_true", dest="as_json",
        help="emit a machine-readable JSON report on stdout",
    )
    parser.add_argument(
        "--root", default=None,
        help="repository root (default: the parent of this script's directory)",
    )
    args = parser.parse_args(argv)

    root = Path(args.root).resolve() if args.root else repo_root()
    result = build_plan(root)

    # What the plan says has to change, captured before anything is written.
    planned = [a for a in result.actions if a.status in ("missing", "differs", "unexpected", "ignored")]
    if args.check or args.dry_run:
        # "ignored" files are runtime artifacts, not problems: they are pruned
        # by a real sync but must not be reported as drift.
        shown = [a for a in planned if a.status != "ignored"]
    else:
        shown = planned

    wrote = 0
    if not args.check and not args.dry_run:
        wrote = apply_plan(root, result, prune=not args.no_prune)
        # Re-verify: the drift list above was computed before the write.
        result = build_plan(root)

    if args.as_json:
        payload = {
            "ok": result.ok,
            "drift": [d.__dict__ for d in result.drifts],
            "pending": sorted({s for s, _ in result.pending}),
            "written": wrote,
            "adaptation_hits": result.adaptation_hits,
        }
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        if args.report:
            print_report(result, root)
            print()

        for action in shown:
            print("%-9s %s" % (action.status.upper(), action.rel_path))

        if args.dry_run:
            print("dry run: nothing written (%d file(s) would change)" % len(shown))
        elif wrote:
            print("wrote %d file(s)" % wrote)

        if result.drifts:
            print()
            print("DRIFT (%d):" % len(result.drifts))
            for drift in result.drifts:
                print("   %-18s %s -- %s" % (drift.kind, drift.rel_path, drift.detail))
        else:
            print("no drift: %s mirrors %s" % (TARGET_DIR, SOURCE_DIR))

        if result.pending:
            print()
            print("PENDING DECISION (%d skill(s) left untouched):" % len({s for s, _ in result.pending}))
            for skill, reason in sorted(set(result.pending)):
                print("   %s" % skill)

    pending_skills = {s for s, _ in result.pending}
    if args.strict_pending and pending_skills:
        return 2
    if result.drifts:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
