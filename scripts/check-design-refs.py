#!/usr/bin/env python3
"""Check that design section references in code point at real headings.

Code, tests and Web comments cite the design documents by section number:

  * "design §X.Y"   -> a heading in docs/design/platpulse.md (the design
                       authority named by AGENTS.md);
  * "webui.md §X.Y" -> a heading in docs/design/webui.md.

A document may be split into files: its sections then live in the Markdown
files of a directory beside it named after it (docs/design/webui/ for
docs/design/webui.md), each section keeping its number and heading. A
reference resolves against the headings of the document and of every file in
that directory together, so a section moved into its own file keeps its
citation.

A reference is a run that starts with one of those prefixes and may continue
with more section numbers joined by ",", "/", "and", "or", "to", "–" or "-",
for example "design §11.4, §11.6 and §11.7" or "webui.md §6.1–§6.3". Each
continuation repeats "§", except a tight range such as "§6.1–6.3"; a range is
checked at both ends.

The check fails when

  * a cited section number has no heading in the document it names, nor in
    any file of its directory (a dangling reference, such as a section of an
    earlier design revision);
  * a cited section number is headed in more than one of those files, so the
    reference is ambiguous;
  * a "design §" number is not a section number at all (for example a line
    number such as "design §561");
  * a "§" appears without a recognised source prefix, so a reader cannot tell
    which document it means.

Scanned: crates/**, platpulse-web/src/**, platpulse-web/e2e/**, scripts/**.
Excluded: the Agent and Server migrations/ directories (SQLx checksums applied
migrations, so their comments are frozen) and platpulse-web/src/api/generated/
(regenerated from the OpenAPI document).

Only these two documents are checked; cite any other document (an ADR, a
research note) in words rather than with a bare "§".

Usage:
    python3 scripts/check-design-refs.py            # check the repository
    python3 scripts/check-design-refs.py --self-test
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Each reference source names a document; its sections may also live in the
# files of the directory of the same name beside it (see split_directory).
DOCUMENTS = {
    "design": Path("docs/design/platpulse.md"),
    "webui.md": Path("docs/design/webui.md"),
}

SCAN_ROOTS = (
    Path("crates"),
    Path("platpulse-web/src"),
    Path("platpulse-web/e2e"),
    Path("scripts"),
)
EXCLUDED_PARTS = {"migrations", "node_modules", "target", "dist", "test-results"}
EXCLUDED_PREFIXES = (
    Path("platpulse-web/src/api/generated"),
    # This checker quotes the reference syntax it checks.
    Path("scripts/check-design-refs.py"),
)
TEXT_SUFFIXES = {
    ".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs", ".css", ".html",
    ".py", ".sh", ".toml", ".json", ".md", ".sql", ".yml", ".yaml", ".txt",
}

HEADING = re.compile(r"^#{1,6}\s+(\d+(?:\.\d+)*)\.?(?:\s|$)")
NUMBER = r"\d+(?:\.\d+)*"
# A run starts at a source prefix and the first section number ...
RUN_START = re.compile(r"(?P<source>\b[Dd]esign|\bwebui\.md)\s+§\s?(?P<number>" + NUMBER + ")")
# ... and continues with more numbers of the same source.
RUN_CONTINUE = re.compile(
    r"(?:\s*(?:,|/|、|\band\b|\bor\b|\bto\b|[–-])\s*§\s?|[–-])(?P<number>" + NUMBER + ")"
)
# A comment may wrap between the source and its first "§", or between two
# numbers of one run: "(design" / "/// §11.4)" or "design §11.4," / "§11.6".
COMMENT_LEADER = re.compile(r"^\s*(?:\/\/[\/!]?|\/?\*+|#|<!--)?\s*")
TRAILING_SOURCE = re.compile(r"(?P<source>\b[Dd]esign|\bwebui\.md)\s*$")
TRAILING_SEPARATOR = re.compile(r"\s*(?:,|/|、|and|or|to|[–-])\s*")
WRAPPED_START = re.compile(r"§\s?(?P<number>" + NUMBER + ")")
# Section numbers have a small chapter; anything larger is a line number or
# some other count, never a heading of these documents.
MAX_CHAPTER = 99


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    reference: str
    reason: str

    def render(self) -> str:
        return f"{self.path}:{self.line}: {self.reference}: {self.reason}"


def headings(text: str) -> list[str]:
    """The section numbers headed in one Markdown text, in order."""
    found: list[str] = []
    in_fence = False
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("~~~") or line.startswith("```"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        match = HEADING.match(line)
        if match:
            found.append(match.group(1))
    return found


# Where each section number of one document is headed: section -> the files
# (the document itself and the files of its directory) that head it.
SectionIndex = dict[str, list[str]]


def section_index(files: dict[str, str]) -> SectionIndex:
    """Index the headings of a document's files, given as {path: text}."""
    index: SectionIndex = {}
    for path, text in files.items():
        for section in headings(text):
            index.setdefault(section, []).append(path)
    return index


def split_directory(document: Path) -> Path:
    """The directory holding the files a document was split into."""
    return document.with_suffix("")


def document_files(root: Path, document: Path) -> dict[str, str]:
    """The document and every Markdown file of its split directory."""
    files = {document.as_posix(): (root / document).read_text(encoding="utf-8")}
    directory = root / split_directory(document)
    if directory.is_dir():
        for path in sorted(directory.rglob("*.md")):
            files[path.relative_to(root).as_posix()] = path.read_text(encoding="utf-8")
    return files


def describe(document: Path) -> str:
    return f"{document.as_posix()} or {split_directory(document).as_posix()}/"


def check_text(path: str, text: str, design: SectionIndex, webui: SectionIndex) -> list[Finding]:
    documents = {
        "design": (design, describe(DOCUMENTS["design"])),
        "webui.md": (webui, describe(DOCUMENTS["webui.md"])),
    }
    findings: list[Finding] = []
    # The source a wrapped reference carries onto the next line, if any.
    carried: str | None = None
    for number, line in enumerate(text.splitlines(), start=1):
        incoming, carried = carried, None
        trailing = TRAILING_SOURCE.search(line)
        if trailing:
            carried = trailing.group("source").lower()
        if "§" not in line:
            continue
        runs = []
        if incoming:
            leader = COMMENT_LEADER.match(line).end()
            wrapped = WRAPPED_START.match(line, leader)
            if wrapped:
                runs.append((incoming, wrapped))
        runs.extend((start.group("source").lower(), start) for start in RUN_START.finditer(line))
        covered: list[tuple[int, int]] = []
        for source, start in runs:
            sections, document = documents[source]
            cited = [start.group("number")]
            end = start.end()
            while True:
                more = RUN_CONTINUE.match(line, end)
                if not more:
                    break
                cited.append(more.group("number"))
                end = more.end()
            covered.append((start.start(), end))
            if TRAILING_SEPARATOR.fullmatch(line, end):
                carried = source
            reference = line[start.start():end].strip()
            for section in cited:
                chapter = int(section.split(".")[0])
                if chapter > MAX_CHAPTER:
                    findings.append(Finding(path, number, reference,
                        f"§{section} is not a section number (line numbers and counts are not references)"))
                elif section not in sections:
                    findings.append(Finding(path, number, reference,
                        f"§{section} has no heading in {document}"))
                elif len(sections[section]) > 1:
                    findings.append(Finding(path, number, reference,
                        f"§{section} is ambiguous: headed in {', '.join(sections[section])}"))
        for index, char in enumerate(line):
            if char != "§":
                continue
            if any(lo <= index < hi for lo, hi in covered):
                continue
            snippet = line[max(0, index - 20): index + 12].strip()
            findings.append(Finding(path, number, snippet,
                "§ without a source; write 'design §X.Y' (platpulse.md) or 'webui.md §X.Y'"))
    return findings


def scanned_files(root: Path):
    for scan_root in SCAN_ROOTS:
        base = root / scan_root
        if not base.exists():
            continue
        for path in sorted(base.rglob("*")):
            if not path.is_file():
                continue
            relative = path.relative_to(root)
            if EXCLUDED_PARTS.intersection(relative.parts):
                continue
            if any(relative.is_relative_to(prefix) for prefix in EXCLUDED_PREFIXES):
                continue
            if path.suffix not in TEXT_SUFFIXES:
                continue
            yield relative, path


def check_repository(root: Path) -> list[Finding]:
    design = section_index(document_files(root, DOCUMENTS["design"]))
    webui = section_index(document_files(root, DOCUMENTS["webui.md"]))
    findings: list[Finding] = []
    for relative, path in scanned_files(root):
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        findings.extend(check_text(relative.as_posix(), text, design, webui))
    return findings


def split_cases() -> list[tuple[str, dict[str, str], int]]:
    """Repositories whose documents are split into files: (label, files, findings)."""
    design = {"docs/design/platpulse.md": "# Design\n## 10. 身份\n"}
    webui_index = {"docs/design/webui.md": "# WebUI\n## 1. Purpose\n"}
    webui_child = {"docs/design/webui/15-management.md": "## 15. Accepted\n### 15.16 Metric history\n"}
    design_child = {"docs/design/platpulse/20-later.md": "## 20. Later\n"}
    return [
        ("a section headed in a child file resolves",
         {**design, **webui_index, **webui_child, "crates/a.rs": "// webui.md §15.16 and §1\n"}, 0),
        ("a section headed in no file is rejected",
         {**design, **webui_index, **webui_child, "crates/a.rs": "// webui.md §15.99\n"}, 1),
        ("a section headed in two files is ambiguous",
         {**design, **webui_index, **webui_child,
          "docs/design/webui/15-copy.md": "### 15.16 Metric history again\n",
          "crates/a.rs": "// webui.md §15.16\n"}, 1),
        ("a section in the index and a child file is ambiguous",
         {**design, "docs/design/webui.md": "# WebUI\n## 15. Accepted\n", **webui_child,
          "crates/a.rs": "// webui.md §15\n"}, 1),
        ("a design section headed in a child file resolves",
         {**design, **design_child, **webui_index, "crates/a.rs": "// design §20 and §10\n"}, 0),
        ("a child file of the other document does not count",
         {**design, **webui_index, **webui_child, "crates/a.rs": "// design §15.16\n"}, 1),
    ]


def run_split_case(files: dict[str, str]) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for relative, text in files.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        return len(check_repository(root))


def self_test() -> int:
    design = section_index({"platpulse.md": "# Title\n## 10. 身份\n### 10.2 Human\n### 15.4.1 Trend\n~~~text\n# 99.9 not a heading\n~~~\n"})
    webui = section_index({"webui.md": "## 15. Accepted\n### 15.16 Metric history\n"})
    cases = [
        ("// design §10.2 governs sessions", 0),
        ("// design §10, §10.2 and §15.4.1", 0),
        ("// Design §10.2 at a sentence start", 0),
        ("// webui.md §15.16", 0),
        ("// design §10.2; webui.md §15.16", 0),
        ("// design §10.2–§15.4.1", 0),
        ("// design §19.4: startup safety", 1),
        ("// design §561", 1),
        ("// design §15.16 means webui.md", 1),
        ("// webui.md §10.2 is not a webui heading", 1),
        ("// design §10.2, §17.5", 1),
        ("// bare (§13.3) reference", 1),
        ("// design §10.2 then a bare §15.4.1", 1),
        ("// design §99.9 inside a fence is not a heading", 1),
        ("// no references at all", 0),
        ("/// wrapped (design\n/// §10.2) reference", 0),
        ("/// wrapped (webui.md\n/// §15.16) reference", 0),
        ("/// wrapped run design §10.2,\n/// §15.4.1 and more", 0),
        ("/// wrapped (design\n/// §19.4) dangling", 1),
        ("/// a design sentence\n/// then §10.2 mid-line is bare", 1),
        ("/// design §10.2.\n/// §10.2 after a full stop is bare", 1),
        ("// design §10.2–15.4.1 is a tight range", 0),
        ("// design §10.2 - 24 hours is prose, not a range", 0),
        ("// webui.md §1218", 1),
    ]
    failures = 0
    for text, expected in cases:
        got = len(check_text("case", text, design, webui))
        if got != expected:
            failures += 1
            print(f"self-test FAILED: {text!r}: expected {expected} finding(s), got {got}")
    splits = split_cases()
    for label, files, expected in splits:
        got = run_split_case(files)
        if got != expected:
            failures += 1
            print(f"self-test FAILED: {label}: expected {expected} finding(s), got {got}")
    if failures:
        return 1
    print(f"self-test passed ({len(cases) + len(splits)} cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--self-test", action="store_true", help="run the built-in cases and exit")
    parser.add_argument("--root", type=Path, default=ROOT, help="repository root (default: this checkout)")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    findings = check_repository(args.root)
    for finding in findings:
        print(finding.render())
    if findings:
        print(f"\n{len(findings)} design section reference(s) do not match a heading.", file=sys.stderr)
        return 1
    print("design section references: all match a heading")
    return 0


if __name__ == "__main__":
    sys.exit(main())
