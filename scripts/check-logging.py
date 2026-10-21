#!/usr/bin/env python3
"""Logging gate (issue #250).

Diagnostics leave the Agent and Server only through the `tracing` egress in
`src/logging.rs`, which is where redaction is enforced. This gate keeps it that way:

1. No bare `eprintln!`/`eprint!`/`dbg!` in crate sources. The only exception is
   the binary's `main.rs`, which prints the final CLI error after flushing logs.
2. `println!`/`print!` (stdout) only in `main.rs` and `cli.rs`: stdout is
   reserved for machine-readable and CLI result output, never diagnostics.
3. Structured fields on `tracing` events and spans come from the shared vocabulary
   (docs/design/logging.md) so journald/JSON queries stay stable.

Test modules (`#[cfg(test)]` blocks and `tests/`) are not runtime paths and are
skipped. Exit status is non-zero on any violation.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES = ["platpulse-agent", "platpulse-server", "platpulse-core"]

# The single machine-checked copy of the field vocabulary; the "字段词表"
# section of docs/design/logging.md documents the same names and their meaning.
VOCABULARY = {
    "request_id",
    "route_group",
    "route",
    "status",
    "duration_ms",
    "agent_id",
    "boot_id",
    "report_id",
    "report_sequence",
    "node_id",
    "operation_id",
    "outcome",
    "error",
    "listen",
}

STDERR_MACROS = re.compile(r"\b(eprintln|eprint|dbg)!\s*\(")
STDOUT_MACROS = re.compile(r"(?<![e\w])(println|print)!\s*\(")
# Qualified (`tracing::warn!`) or imported (`warn!`) event and span macros.
TRACING_MACROS = re.compile(
    r"(?:\btracing::|(?<![\w:]))(trace|debug|info|warn|error)(_span)?!\s*\("
)
# One top-level macro argument naming a field: `name`, `%name`, `?name`,
# `name = value`, `name = %value`.
FIELD_ARGUMENT = re.compile(r"^[%?]?\s*([A-Za-z_][A-Za-z0-9_.]*)\s*(?:=(?!=)|$)", re.S)
# Late-bound span fields: `span.record("name", …)` (read from the raw source).
RECORD = re.compile(r"\.record\(\s*\"([A-Za-z_][A-Za-z0-9_.]*)\"")
MACRO_KEYWORDS = ("target:", "parent:", "name:")


def strip_strings_and_comments(source: str) -> str:
    """Blank out string/char literals and comments, keeping offsets and lines."""
    out = []
    i = 0
    n = len(source)
    while i < n:
        c = source[i]
        if source.startswith("//", i):
            j = source.find("\n", i)
            j = n if j == -1 else j
            out.append(" " * (j - i))
            i = j
        elif source.startswith("/*", i):
            j = source.find("*/", i + 2)
            j = n if j == -1 else j + 2
            out.append(re.sub(r"[^\n]", " ", source[i:j]))
            i = j
        elif c == "r" and re.match(r'r(#*)"', source[i:]) and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_")):
            hashes = re.match(r'r(#*)"', source[i:]).group(1)
            end = '"' + hashes
            j = source.find(end, i + 2 + len(hashes))
            j = n if j == -1 else j + len(end)
            out.append('"' + re.sub(r"[^\n]", " ", source[i + 1 : j - 1]) + '"')
            i = j
        elif c == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            j = min(j + 1, n)
            out.append('"' + re.sub(r"[^\n]", " ", source[i + 1 : j - 1]) + '"')
            i = j
        elif c == "'" and re.match(r"'(\\.|[^\\'])'", source[i:]):
            m = re.match(r"'(\\.|[^\\'])'", source[i:])
            out.append(" " * len(m.group(0)))
            i += len(m.group(0))
        else:
            out.append(c)
            i += 1
    return "".join(out)


def test_ranges(code: str) -> list[tuple[int, int]]:
    """Byte ranges of `#[cfg(test)]` items (modules or functions)."""
    ranges = []
    for match in re.finditer(r"#\[cfg\(test\)\]", code):
        brace = code.find("{", match.end())
        semicolon = code.find(";", match.end())
        if brace == -1 or (semicolon != -1 and semicolon < brace):
            continue
        depth = 0
        for k in range(brace, len(code)):
            if code[k] == "{":
                depth += 1
            elif code[k] == "}":
                depth -= 1
                if depth == 0:
                    ranges.append((match.start(), k))
                    break
    return ranges


def macro_body(code: str, open_paren: int) -> str:
    depth = 0
    for k in range(open_paren, len(code)):
        if code[k] in "([{":
            depth += 1
        elif code[k] in ")]}":
            depth -= 1
            if depth == 0:
                return code[open_paren + 1 : k]
    return code[open_paren + 1 :]


def top_level(body: str) -> str:
    """Keep only depth-0 text of a macro body (drop nested call arguments)."""
    out = []
    depth = 0
    for c in body:
        if c in "([{":
            depth += 1
            out.append(" ")
        elif c in ")]}":
            depth -= 1
            out.append(" ")
        else:
            out.append(c if depth == 0 else " ")
    return "".join(out)


def field_names(body: str, is_span: bool) -> list[str]:
    """Field names among a macro's top-level arguments.

    Event fields precede the message format string; anything after it is a
    format argument. A span's first argument is its name and every later
    argument is a field.
    """
    segments = top_level(body).split(",")
    if is_span:
        segments = segments[1:]
    names = []
    for segment in segments:
        segment = segment.strip()
        if not segment or segment.startswith(MACRO_KEYWORDS):
            continue
        if not is_span and '"' in segment:
            break
        match = FIELD_ARGUMENT.match(segment)
        if match:
            names.append(match.group(1))
    return names


def check_file(path: Path) -> list[str]:
    rel = path.relative_to(ROOT)
    raw = path.read_text()
    code = strip_strings_and_comments(raw)
    tests = test_ranges(code)

    def in_tests(offset: int) -> bool:
        return any(start <= offset <= end for start, end in tests)

    def line_of(offset: int) -> int:
        return code.count("\n", 0, offset) + 1

    problems = []
    is_main = path.name == "main.rs"
    stdout_allowed = path.name in {"main.rs", "cli.rs"}
    for match in STDERR_MACROS.finditer(code):
        if in_tests(match.start()) or is_main:
            continue
        problems.append(
            f"{rel}:{line_of(match.start())}: bare {match.group(1)}! in a runtime path; use tracing"
        )
    for match in STDOUT_MACROS.finditer(code):
        if in_tests(match.start()) or stdout_allowed:
            continue
        problems.append(
            f"{rel}:{line_of(match.start())}: {match.group(1)}! writes stdout outside main.rs/cli.rs"
        )
    for match in TRACING_MACROS.finditer(code):
        if in_tests(match.start()):
            continue
        body = macro_body(code, match.end() - 1)
        for name in field_names(body, is_span=bool(match.group(2))):
            if name not in VOCABULARY:
                problems.append(
                    f"{rel}:{line_of(match.start())}: log field '{name}' is not in the vocabulary"
                )
    for match in RECORD.finditer(raw):
        if in_tests(match.start()) or match.group(1) in VOCABULARY:
            continue
        problems.append(
            f"{rel}:{line_of(match.start())}: recorded span field '{match.group(1)}' is not in the vocabulary"
        )
    return problems


def main() -> int:
    problems = []
    for crate in CRATES:
        for path in sorted((ROOT / "crates" / crate / "src").rglob("*.rs")):
            problems.extend(check_file(path))
    for problem in problems:
        print(problem)
    if problems:
        print(f"check-logging: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    print("check-logging: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
