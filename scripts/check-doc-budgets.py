#!/usr/bin/env python3
"""Hold every Markdown document under a length budget.

A document nobody can read end to end stops being an authority: readers and
agents skim it, miss the clause that matters, and copy facts into a second
place instead. Each tracked Markdown file therefore has a budget, measured in
words: a run of Latin text between spaces counts as one word and each CJK
character counts as one, so Chinese and English prose are measured alike.

scripts/doc-budgets.json holds the default budget for every document and a
frozen ceiling for each document that was already over it. A frozen document
may only shrink: the check fails when it grows past its ceiling, and also when
it falls below it, so a change that shortens the document lowers the ceiling in
the same pull request. Content added to a frozen document is offset in the same
change, by removing text or by moving a section into its own file. Once a
frozen document fits the default budget its entry is removed.

Frozen history (docs/research/archive/) and vendored upstream files (vendor/)
are not measured.

Usage:
    python3 scripts/check-doc-budgets.py
    python3 scripts/check-doc-budgets.py --self-test
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUDGET_FILE = Path("scripts/doc-budgets.json")
EXCLUDED = ("docs/research/archive/", "vendor/")
CJK = re.compile(r"[\u3040-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]")


def word_count(text: str) -> int:
    return len(CJK.findall(text)) + len(CJK.sub(" ", text).split())


def markdown_files(repo: Path) -> list[str]:
    try:
        out = subprocess.run(
            ["git", "-C", str(repo), "ls-files", "-z", "--cached", "--others",
             "--exclude-standard", "--", "*.md"],
            check=True, capture_output=True, text=True,
        ).stdout
        files = [f for f in out.split("\0") if f]
    except (subprocess.CalledProcessError, FileNotFoundError):
        files = [p.relative_to(repo).as_posix() for p in repo.rglob("*.md")]
    return sorted(f for f in files if not f.startswith(EXCLUDED) and (repo / f).is_file())


def check(repo: Path, files: list[str]) -> list[str]:
    budgets = json.loads((repo / BUDGET_FILE).read_text(encoding="utf-8"))
    default = budgets["default"]
    frozen: dict[str, int] = budgets.get("frozen", {})
    problems: list[str] = []
    for rel in files:
        words = word_count((repo / rel).read_text(encoding="utf-8"))
        ceiling = frozen.get(rel)
        if ceiling is None:
            if words > default:
                problems.append(
                    f"{rel}: {words} words, budget {default}; split it into sections "
                    f"in separate files or cut it")
        elif words > ceiling:
            problems.append(
                f"{rel}: {words} words, frozen at {ceiling}; offset the {words - ceiling} "
                f"added words in this change (cut text or move a section into its own file)")
        elif words <= default:
            problems.append(
                f"{rel}: {words} words now fits the default budget {default}; "
                f"remove its entry from {BUDGET_FILE}")
        elif words < ceiling:
            problems.append(
                f"{rel}: {words} words, frozen at {ceiling}; lower its ceiling in "
                f"{BUDGET_FILE} to {words}")
    missing = sorted(set(frozen) - set(files))
    problems += [f"{BUDGET_FILE}: {rel} is frozen but not a measured document; remove it"
                 for rel in missing]
    return problems


def self_test() -> int:
    def scenario(files: dict[str, str], budgets: dict) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            for rel, text in {**files, BUDGET_FILE.as_posix(): json.dumps(budgets)}.items():
                path = repo / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8")
            return check(repo, markdown_files(repo))

    def words(n: int) -> str:
        return " ".join(["word"] * n) + "\n"

    big = "docs/design/big.md"
    budgets = {"default": 10, "frozen": {big: 20}}
    cases = [
        ("within budget", {"README.md": words(10), big: words(20)}, budgets, 0),
        ("a new document over budget", {"docs/new.md": words(11), big: words(20)}, budgets, 1),
        ("CJK characters count one each", {"docs/zh.md": "中文" * 6 + "\n", big: words(20)}, budgets, 1),
        ("mixed text under budget", {"docs/zh.md": "状态 is Healthy 节点\n", big: words(20)}, budgets, 0),
        ("a frozen document grows", {big: words(21)}, budgets, 1),
        ("a frozen document shrinks without lowering", {big: words(19)}, budgets, 1),
        ("a frozen document fits the default", {big: words(9)}, budgets, 1),
        ("a frozen entry for a missing document", {"README.md": words(1)}, budgets, 1),
        ("archive and vendor are not measured",
         {big: words(20), "docs/research/archive/old.md": words(99), "vendor/x/README.md": words(99)},
         budgets, 0),
    ]
    failed = 0
    for label, files, case_budgets, expected in cases:
        got = 1 if scenario(files, case_budgets) else 0
        if got != expected:
            failed += 1
        print(f"{'ok' if got == expected else 'FAIL'}: {label} "
              f"(expected {'reject' if expected else 'accept'})")
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    files = markdown_files(ROOT)
    problems = check(ROOT, files)
    for problem in problems:
        print(problem, file=sys.stderr)
    if not problems:
        print(f"doc budgets ok: {len(files)} documents")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
