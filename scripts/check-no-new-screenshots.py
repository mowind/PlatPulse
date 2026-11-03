#!/usr/bin/env python3
"""Reject new or changed screenshots under docs/visual-migration/.

The visual-migration evidence already committed there (about 1,000 PNGs,
over 200 MB) stays as history, but the directory must stop growing: every
added or rewritten image is a permanent cost to each clone. New acceptance
screenshots go into the pull request description or a GitHub Actions
artifact instead (issue #283).

The check compares BASE with HEAD and fails when an image file under a frozen
directory is added or modified. Renames count as an addition (the diff runs
with --no-renames). Deleting an image, and adding or editing non-image files
such as Markdown notes or JSON fixtures, is allowed.

Usage:
    python3 scripts/check-no-new-screenshots.py --base HEAD^1
    python3 scripts/check-no-new-screenshots.py --self-test
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath

FROZEN_DIRS = (PurePosixPath("docs/visual-migration"),)
IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".bmp"}


def is_frozen_image(path: str) -> bool:
    p = PurePosixPath(path)
    if p.suffix.lower() not in IMAGE_SUFFIXES:
        return False
    return any(frozen == p or frozen in p.parents for frozen in FROZEN_DIRS)


def findings(base: str, head: str, cwd: Path) -> list[str]:
    out = subprocess.run(
        ["git", "diff", "--name-status", "--no-renames", "-z", base, head, "--"],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    fields = [f for f in out.split("\0") if f]
    bad: list[str] = []
    for status, path in zip(fields[0::2], fields[1::2]):
        if status[0] in {"A", "M", "T"} and is_frozen_image(path):
            bad.append(f"{status[0]} {path}")
    return bad


def report(bad: list[str]) -> int:
    if not bad:
        print("no new or changed screenshots under " + ", ".join(map(str, FROZEN_DIRS)))
        return 0
    print("screenshots must not be added to or rewritten in the repository:")
    for line in bad:
        print(f"  {line}")
    print(
        "Attach acceptance screenshots to the pull request description or upload them as a "
        "GitHub Actions artifact instead (issue #283)."
    )
    return 1


def self_test() -> int:
    def git(repo: Path, *args: str) -> str:
        return subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
            cwd=repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def commit(repo: Path, message: str) -> str:
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "--allow-empty", "-m", message)
        return git(repo, "rev-parse", "HEAD")

    def write(repo: Path, path: str, data: bytes) -> None:
        target = repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)

    shot = "docs/visual-migration/emerald/home.png"
    # (description, mutation, expected number of findings)
    cases = [
        ("add a PNG", lambda r: write(r, "docs/visual-migration/new/a.png", b"new"), 1),
        ("add an uppercase JPEG", lambda r: write(r, "docs/visual-migration/B.JPG", b"x"), 1),
        ("rewrite a PNG", lambda r: write(r, shot, b"changed"), 1),
        ("rename a PNG", lambda r: (r / shot).rename(r / "docs/visual-migration/emerald/moved.png"), 1),
        ("delete a PNG", lambda r: (r / shot).unlink(), 0),
        ("add a Markdown note", lambda r: write(r, "docs/visual-migration/emerald/NOTES.md", b"# n"), 0),
        ("edit a JSON fixture", lambda r: write(r, "docs/visual-migration/mobile-map/networks.json", b"[1]"), 0),
        ("add a PNG elsewhere", lambda r: write(r, "platpulse-web/public/icon.png", b"x"), 0),
        ("add a PNG under a look-alike dir", lambda r: write(r, "docs/visual-migration-old/a.png", b"x"), 0),
    ]
    failed = 0
    for description, mutate, expected in cases:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init", "-q")
            write(repo, shot, b"original")
            write(repo, "docs/visual-migration/mobile-map/networks.json", b"[]")
            base = commit(repo, "base")
            mutate(repo)
            head = commit(repo, "change")
            got = len(findings(base, head, repo))
            if got != expected:
                failed += 1
                print(f"self-test FAILED: {description}: expected {expected} finding(s), got {got}")
    if failed:
        return 1
    print(f"self-test passed ({len(cases)} cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", help="revision to compare HEAD against")
    parser.add_argument("--head", default="HEAD", help="revision to check (default: HEAD)")
    parser.add_argument("--self-test", action="store_true", help="run the built-in cases and exit")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not args.base:
        parser.error("--base is required")
    root = Path(__file__).resolve().parent.parent
    return report(findings(args.base, args.head, root))


if __name__ == "__main__":
    sys.exit(main())
