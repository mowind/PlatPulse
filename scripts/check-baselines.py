#!/usr/bin/env python3
"""Hold code-quality counts at a baseline that may only go down.

Each counter is a pattern PlatPulse wants to retire without a big-bang cleanup:

- issue-refs: "issue #N" in Agent/Server/WebUI source. A comment states the
  current contract; the history belongs in the commit, PR or ADR (#285).
- ts-unsafe-casts: "as any", "as unknown" and ": any" in WebUI source outside
  the generated API client (#288).

scripts/baselines.json records the accepted count for each counter. The check
fails when a count rises above its baseline, and also when it falls below it,
so a change that removes occurrences lowers the baseline in the same pull
request and the gain cannot be spent again later.

Usage:
    python3 scripts/check-baselines.py
    python3 scripts/check-baselines.py --self-test
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE_FILE = Path("scripts/baselines.json")


@dataclass(frozen=True)
class Counter:
    name: str
    pattern: re.Pattern[str]
    roots: tuple[str, ...]
    suffixes: frozenset[str]
    excluded: tuple[str, ...] = ()
    hint: str = ""


COUNTERS = (
    Counter(
        name="issue-refs",
        pattern=re.compile(r"\bissues? #\d+", re.IGNORECASE),
        roots=("crates/platpulse-core/src", "crates/platpulse-agent/src",
               "crates/platpulse-server/src", "platpulse-web/src"),
        suffixes=frozenset({".rs", ".ts", ".tsx"}),
        excluded=("platpulse-web/src/api/generated",),
        hint="state the current constraint in the comment; put the history in the commit, PR or ADR",
    ),
    Counter(
        name="ts-unsafe-casts",
        pattern=re.compile(r"\bas\s+(?:any|unknown)\b|:\s*any\b"),
        roots=("platpulse-web/src",),
        suffixes=frozenset({".ts", ".tsx"}),
        excluded=("platpulse-web/src/api/generated",),
        hint="narrow the value with a type guard or a typed helper instead of casting",
    ),
)


def occurrences(counter: Counter, repo: Path) -> list[str]:
    found: list[str] = []
    for root in counter.roots:
        base = repo / root
        if not base.is_dir():
            continue
        for path in sorted(base.rglob("*")):
            if path.suffix not in counter.suffixes or not path.is_file():
                continue
            rel = path.relative_to(repo).as_posix()
            if any(rel == ex or rel.startswith(ex + "/") for ex in counter.excluded):
                continue
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                for _ in counter.pattern.finditer(line):
                    found.append(f"{rel}:{number}: {line.strip()}")
    return found


def check(repo: Path) -> list[str]:
    baselines = json.loads((repo / BASELINE_FILE).read_text(encoding="utf-8"))
    problems: list[str] = []
    for counter in COUNTERS:
        if counter.name not in baselines:
            problems.append(f"{BASELINE_FILE}: no baseline for {counter.name}")
            continue
        hits = occurrences(counter, repo)
        allowed = baselines[counter.name]
        if len(hits) > allowed:
            problems.append(
                f"{counter.name}: {len(hits)} occurrences, baseline {allowed}; "
                f"{counter.hint}. Occurrences:\n  " + "\n  ".join(hits)
            )
        elif len(hits) < allowed:
            problems.append(
                f"{counter.name}: {len(hits)} occurrences, baseline {allowed}; "
                f"lower {BASELINE_FILE} {counter.name} to {len(hits)}"
            )
    unknown = sorted(set(baselines) - {c.name for c in COUNTERS})
    problems += [f"{BASELINE_FILE}: unknown counter {name}" for name in unknown]
    return problems


def self_test() -> int:
    def write(repo: Path, rel: str, text: str) -> None:
        path = repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def scenario(files: dict[str, str], baselines: dict[str, int]) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            for rel, text in files.items():
                write(repo, rel, text)
            write(repo, BASELINE_FILE.as_posix(), json.dumps(baselines))
            return check(repo)

    rust = "crates/platpulse-server/src/lib.rs"
    web = "platpulse-web/src/a.ts"
    one = {"issue-refs": 1, "ts-unsafe-casts": 1}
    base_files = {rust: "// keep the store (issue #181)\n", web: "const x = y as any\n"}
    cases = [
        ("at baseline", base_files, one, 0),
        ("a new issue ref", {**base_files, "platpulse-web/src/b.tsx": "// Issue #9\n"}, one, 1),
        ("a new as unknown cast", {**base_files, "platpulse-web/src/c.ts": "f(z as unknown)\n"}, one, 1),
        ("a new : any annotation", {**base_files, "platpulse-web/src/d.ts": "let v: any\n"}, one, 1),
        ("below baseline", {web: "const x = y as any\n"}, one, 1),
        ("generated client ignored", {**base_files, "platpulse-web/src/api/generated/t.ts": "let v: any // issue #1\n"}, one, 0),
        ("tests outside src ignored", {**base_files, "crates/platpulse-server/tests/t.rs": "// issue #2\n"}, one, 0),
        ("words that merely contain any", {**base_files, "platpulse-web/src/e.ts": "const company = many\n"}, one, 0),
        ("missing baseline", base_files, {"issue-refs": 1}, 1),
    ]
    failed = 0
    for label, files, baselines, expected in cases:
        got = 1 if scenario(files, baselines) else 0
        status = "ok" if got == expected else "FAIL"
        if got != expected:
            failed += 1
        print(f"{status}: {label} (expected {'reject' if expected else 'accept'})")
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    problems = check(ROOT)
    for problem in problems:
        print(problem, file=sys.stderr)
    if not problems:
        print("baselines ok: " + ", ".join(
            f"{c.name}={len(occurrences(c, ROOT))}" for c in COUNTERS))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
