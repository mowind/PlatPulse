#!/usr/bin/env python3
"""Reject edits to committed database migrations.

sqlx records the checksum of every migration it applies and refuses to start
when a migration file it already applied has changed or is missing. A
migration committed to main may already be applied by a deployment, so it is
frozen: a schema fix goes into a new migration with the next version.

The check compares BASE with HEAD and fails when a migration file that exists
in BASE is modified, deleted, renamed or changes type. Renames count as a
deletion (the diff runs with --no-renames). Adding a migration, and changing
any file outside the migration directories, is allowed.

Usage:
    python3 scripts/check-frozen-migrations.py --base HEAD^1
    python3 scripts/check-frozen-migrations.py --self-test
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath

MIGRATION_DIRS = (
    PurePosixPath("crates/platpulse-server/migrations"),
    PurePosixPath("crates/platpulse-server/migrations-postgres"),
    PurePosixPath("crates/platpulse-agent/migrations"),
)


def is_migration(path: str) -> bool:
    p = PurePosixPath(path)
    return any(directory in p.parents for directory in MIGRATION_DIRS)


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
        if status[0] in {"M", "D", "T"} and is_migration(path):
            bad.append(f"{status[0]} {path}")
    return bad


def report(bad: list[str]) -> int:
    if not bad:
        print("no committed migration changed under " + ", ".join(map(str, MIGRATION_DIRS)))
        return 0
    print("committed migrations must not be modified, deleted or renamed:")
    for line in bad:
        print(f"  {line}")
    print(
        "A deployment that already applied the migration fails its sqlx checksum check at "
        "startup. Restore the file and put the fix in a new migration with the next version "
        "(see the platpulse-schema-change skill)."
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

    def write(repo: Path, path: str, data: str) -> None:
        target = repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(data)

    server = "crates/platpulse-server/migrations/0001_server_identity.sql"
    postgres = "crates/platpulse-server/migrations-postgres/0075_postgres_baseline.sql"
    agent = "crates/platpulse-agent/migrations/0001_agent_store.sql"
    # (description, mutation, expected number of findings)
    cases = [
        ("edit a SQLite Server migration", lambda r: write(r, server, "CREATE TABLE b (id);\n"), 1),
        ("edit a PostgreSQL Server migration", lambda r: write(r, postgres, "-- changed\n"), 1),
        ("edit an Agent migration", lambda r: write(r, agent, "CREATE TABLE a (id);  \n"), 1),
        ("delete a migration", lambda r: (r / agent).unlink(), 1),
        ("rename a migration", lambda r: (r / server).rename(r / server.replace("0001_", "0001b_")), 1),
        ("make a migration executable", lambda r: (r / agent).chmod(0o755), 1),
        ("add a migration", lambda r: write(r, "crates/platpulse-agent/migrations/0002_next.sql", "x\n"), 0),
        ("add a migration pair", lambda r: (
            write(r, "crates/platpulse-server/migrations/0076_next.sql", "x\n"),
            write(r, "crates/platpulse-server/migrations-postgres/0076_next.sql", "x\n"),
        ), 0),
        ("edit Server source", lambda r: write(r, "crates/platpulse-server/src/database/mod.rs", "// v2\n"), 0),
        ("edit a look-alike directory", lambda r: write(r, "crates/platpulse-server/migrations-old/0001.sql", "y\n"), 0),
    ]
    failed = 0
    for description, mutate, expected in cases:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init", "-q")
            git(repo, "config", "core.fileMode", "true")
            write(repo, server, "CREATE TABLE a (id);\n")
            write(repo, postgres, "CREATE TABLE a (id bigint);\n")
            write(repo, agent, "CREATE TABLE a (id);\n")
            write(repo, "crates/platpulse-server/src/database/mod.rs", "// v1\n")
            write(repo, "crates/platpulse-server/migrations-old/0001.sql", "x\n")
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
