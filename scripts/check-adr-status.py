#!/usr/bin/env python3
"""Check the numbering and Status line of every ADR under docs/adr/.

An ADR is `docs/adr/NNNN-slug.md`, numbered from 0001 without gaps or
duplicates, titled `# ADR NNNN: <title>` with its own number. Its first
non-blank line after the title is the Status line:

    **Status:** <value>[ — <one sentence on the current state>]

(`- **Status:** ...` is accepted as the first item of a metadata list).
The value is one of Proposed, Accepted, Rejected, Deprecated, or
`Superseded by [ADR NNNN](NNNN-slug.md)` naming an existing ADR.

The Status line says what holds now, so it stays short and carries no
delivery history: at most 200 characters, no issue references. An Accepted
ADR describes delivered state, so its Status line never says the work is
pending, in progress or not yet implemented; an undelivered decision is
Proposed.

Usage:
    python3 scripts/check-adr-status.py
    python3 scripts/check-adr-status.py --self-test
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from pathlib import Path

ADR_DIR = Path("docs/adr")
MAX_STATUS_CHARS = 200
FILE_RE = re.compile(r"^(\d{4})-[a-z0-9]+(?:-[a-z0-9]+)*\.md$")
TITLE_RE = re.compile(r"^# ADR (\d{4}): \S")
STATUS_RE = re.compile(
    r"^(?:- )?\*\*Status:\*\* "
    r"(?P<value>Proposed|Accepted|Rejected|Deprecated|Superseded by \[ADR (?P<by>\d{4})\]\((?P<target>[^)]+)\))"
    r"(?P<rest>.*)$"
)
ISSUE_REF_RE = re.compile(r"#\d+|\bissues?\b", re.IGNORECASE)
UNDELIVERED_RE = re.compile(r"pending|in progress|not yet implemented|to be implemented", re.IGNORECASE)


def check_adr(path: Path, numbers: set[str]) -> list[str]:
    name = path.name
    number = FILE_RE.match(name)
    assert number is not None
    lines = path.read_text(encoding="utf-8").splitlines()
    problems: list[str] = []
    title = TITLE_RE.match(lines[0]) if lines else None
    if title is None:
        problems.append(f"{name}: first line must be '# ADR {number.group(1)}: <title>'")
    elif title.group(1) != number.group(1):
        problems.append(f"{name}: title numbers the ADR {title.group(1)}, the file name {number.group(1)}")
    status_line = next((line for line in lines[1:] if line.strip()), "")
    status = STATUS_RE.match(status_line)
    if status is None:
        problems.append(
            f"{name}: the first line after the title must be '**Status:** <Proposed|Accepted|Rejected|"
            f"Deprecated|Superseded by [ADR NNNN](NNNN-slug.md)>', found: {status_line[:80]!r}"
        )
        return problems
    rest = status.group("rest")
    if rest and not rest.startswith(" — "):
        problems.append(f"{name}: a note after the Status value starts with ' — ', found: {rest[:40]!r}")
    if len(status_line) > MAX_STATUS_CHARS:
        problems.append(
            f"{name}: Status line is {len(status_line)} characters (at most {MAX_STATUS_CHARS}); "
            "move lasting facts into the body and delivery history into the pull request"
        )
    if ISSUE_REF_RE.search(rest):
        problems.append(f"{name}: Status line cites an issue; it states the current state, not its history")
    if status.group("value") == "Accepted" and UNDELIVERED_RE.search(rest):
        problems.append(f"{name}: an Accepted ADR is delivered; an undelivered decision is Proposed")
    by = status.group("by")
    if by is not None:
        target = status.group("target")
        if not FILE_RE.match(target) or not target.startswith(by + "-") or not (path.parent / target).is_file():
            problems.append(f"{name}: 'Superseded by' must link an existing ADR {by} file, found {target!r}")
        elif by == number.group(1):
            problems.append(f"{name}: an ADR cannot supersede itself")
    numbers.add(number.group(1))
    return problems


def check(root: Path) -> list[str]:
    adr_dir = root / ADR_DIR
    problems: list[str] = []
    seen: dict[str, str] = {}
    adrs: list[Path] = []
    for path in sorted(adr_dir.glob("*.md")):
        match = FILE_RE.match(path.name)
        if match is None:
            problems.append(f"{path.name}: ADR files are named NNNN-slug.md (lowercase slug)")
            continue
        if match.group(1) in seen:
            problems.append(f"{path.name}: number {match.group(1)} is also used by {seen[match.group(1)]}")
            continue
        seen[match.group(1)] = path.name
        adrs.append(path)
    numbers: set[str] = set()
    for path in adrs:
        problems.extend(check_adr(path, numbers))
    expected = [f"{n:04d}" for n in range(1, len(seen) + 1)]
    missing = sorted(set(expected) - set(seen))
    if missing:
        problems.append(f"ADR numbers must run from 0001 without gaps; missing {', '.join(missing)}")
    return problems


def report(problems: list[str]) -> int:
    if not problems:
        print(f"ADR status ok: {len(list((Path(__file__).resolve().parent.parent / ADR_DIR).glob('*.md')))} ADRs")
        return 0
    print("ADR numbering or Status lines need fixing:")
    for line in problems:
        print(f"  {line}")
    return 1


def self_test() -> int:
    good = "# ADR {n}: Title\n\n**Status:** {status}\n\nBody.\n"

    def adr(n: str, status: str) -> tuple[str, str]:
        return (f"{n}-slug-{n}.md", good.format(n=n, status=status))

    base = [adr("0001", "Accepted"), adr("0002", "Proposed")]
    # (description, files, expected number of problems)
    cases: list[tuple[str, list[tuple[str, str]], int]] = [
        ("well-formed set", base, 0),
        ("note after the value", [adr("0001", "Accepted — ships with 0.3.0.")], 0),
        ("metadata list item", [("0001-a.md", "# ADR 0001: T\n\n- **Status:** Rejected\n- **Date:** 2026-01-01\n")], 0),
        ("every value", [adr("0001", "Rejected"), adr("0002", "Deprecated"),
                         adr("0003", "Superseded by [ADR 0001](0001-slug-0001.md)")], 0),
        ("unknown value", [adr("0001", "Approved")], 1),
        ("plain Status: prefix", [("0001-a.md", "# ADR 0001: T\n\nStatus: Accepted\n")], 1),
        ("Status not first", [("0001-a.md", "# ADR 0001: T\n\nContext first.\n\n**Status:** Accepted\n")], 1),
        ("note without dash", [adr("0001", "Accepted; implemented.")], 1),
        ("issue in Status", [adr("0001", "Accepted — delivered by #12.")], 1),
        ("Accepted but pending", [adr("0001", "Accepted — implementation pending.")], 1),
        ("Accepted not yet implemented", [adr("0001", "Accepted — not yet implemented.")], 1),
        ("Proposed may be pending", [adr("0001", "Proposed — implementation pending.")], 0),
        ("Status too long", [adr("0001", "Accepted — " + "x" * 200)], 1),
        ("superseded by missing ADR", [adr("0001", "Superseded by [ADR 0009](0009-gone.md)")], 1),
        ("superseded by itself", [adr("0001", "Superseded by [ADR 0001](0001-slug-0001.md)")], 1),
        ("title number mismatch", [("0001-a.md", "# ADR 0002: T\n\n**Status:** Accepted\n")], 1),
        ("title without ADR prefix", [("0001-a.md", "# T\n\n**Status:** Accepted\n")], 1),
        ("gap in numbering", [adr("0001", "Accepted"), adr("0003", "Accepted")], 1),
        ("duplicate number", [adr("0001", "Accepted"), ("0001-other.md", good.format(n="0001", status="Accepted"))], 1),
        ("bad file name", base + [("ADR-3.md", "# ADR 0003: T\n\n**Status:** Accepted\n")], 1),
    ]
    failed = 0
    for description, files, expected in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ADR_DIR).mkdir(parents=True)
            for name, text in files:
                (root / ADR_DIR / name).write_text(text, encoding="utf-8")
            got = check(root)
            if len(got) != expected:
                failed += 1
                print(f"self-test FAILED: {description}: expected {expected} problem(s), got {got}")
    if failed:
        return 1
    print(f"self-test passed ({len(cases)} cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--self-test", action="store_true", help="run the built-in cases and exit")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    return report(check(Path(__file__).resolve().parent.parent))


if __name__ == "__main__":
    sys.exit(main())
