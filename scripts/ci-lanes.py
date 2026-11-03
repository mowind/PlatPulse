#!/usr/bin/env python3
"""Select the CI lanes a change set needs.

The CI workflow runs this on the files a pull request changes and turns the
result into the `if:` conditions of its lanes; `all-checks-passed` then
treats a lane skipped here as passed. A push to `main` runs every lane
(`--all`), so `main` itself is always fully verified after a merge.

Each changed path selects lanes by the first matching rule below. A path no
rule recognises selects every lane, so a new top-level directory or tool file
is fully checked until a rule says otherwise. The repository checks lane
(design references, logging discipline, screenshots) always runs and is not
selected here.

  docs/openapi/**                       rust, web (spec freshness, generated client)
  docs/deployment.md, docs/deployment/**,
  LICENSE                               release-package, rc-harness (packaged)
  docs/visual-migration/**/*.json       e2e (frozen fixtures read by specs)
  .github/pull_request_template.md,
  .github/ISSUE_TEMPLATE/**,
  .agents/skills/**/*.md                nothing
  docs/**/*.md, top-level *.md          nothing
  crates/platpulse-agent/**             rust, rust-postgres, release-package, rc-harness
  crates/platpulse-server/**,
  crates/platpulse-core/**, Cargo.toml,
  Cargo.lock, vendor/**                 every lane (e2e and the harness boot the Server)
  platpulse-web/**                      web, e2e, release-package, rc-harness
  anything else                         every lane

An empty change set also selects every lane.

Usage:
    git diff --name-only BASE HEAD | python3 scripts/ci-lanes.py >> "$GITHUB_OUTPUT"
    python3 scripts/ci-lanes.py --all >> "$GITHUB_OUTPUT"
    python3 scripts/ci-lanes.py --self-test
"""

from __future__ import annotations

import argparse
import sys
from fnmatch import fnmatchcase

LANES = ("rust", "rust_postgres", "web", "e2e", "release_package", "rc_harness")
ALL = frozenset(LANES)

# (patterns, lanes): the first rule with a matching pattern wins. fnmatch's
# "*" also matches "/", so "docs/*.md" covers every Markdown file under docs/.
RULES: tuple[tuple[tuple[str, ...], frozenset[str]], ...] = (
    (("docs/openapi/*",), frozenset({"rust", "web"})),
    (("docs/deployment.md", "docs/deployment/*", "LICENSE"), frozenset({"release_package", "rc_harness"})),
    (("docs/visual-migration/*.json",), frozenset({"e2e"})),
    ((".github/pull_request_template.md", ".github/ISSUE_TEMPLATE/*", ".agents/skills/*.md"), frozenset()),
    (("docs/*.md",), frozenset()),
    (("crates/platpulse-agent/*",), frozenset({"rust", "rust_postgres", "release_package", "rc_harness"})),
    (
        ("crates/platpulse-server/*", "crates/platpulse-core/*", "Cargo.toml", "Cargo.lock", "vendor/*"),
        ALL,
    ),
    (("platpulse-web/*",), frozenset({"web", "e2e", "release_package", "rc_harness"})),
)


def lanes_for_path(path: str) -> frozenset[str]:
    if "/" not in path and path.endswith(".md"):
        return frozenset()
    for patterns, lanes in RULES:
        if any(fnmatchcase(path, pattern) for pattern in patterns):
            return lanes
    return ALL


def select(paths: list[str]) -> frozenset[str]:
    paths = [p.strip() for p in paths if p.strip()]
    if not paths:
        return ALL
    selected: set[str] = set()
    for path in paths:
        selected |= lanes_for_path(path)
    return frozenset(selected)


def render(selected: frozenset[str]) -> str:
    return "".join(f"{lane}={'true' if lane in selected else 'false'}\n" for lane in LANES)


def self_test() -> int:
    cases: list[tuple[list[str], frozenset[str]]] = [
        (["docs/adr/0014-x.md"], frozenset()),
        (["README.md", "AGENTS.md", "docs/research/archive/a.md"], frozenset()),
        ([".github/pull_request_template.md"], frozenset()),
        ([".agents/skills/platpulse-schema-change/SKILL.md"], frozenset()),
        ([".agents/skills/some-skill/helper.py"], ALL),
        (["docs/openapi/openapi.json"], frozenset({"rust", "web"})),
        (["docs/deployment.md"], frozenset({"release_package", "rc_harness"})),
        (["docs/deployment/upgrade-and-rollback.md"], frozenset({"release_package", "rc_harness"})),
        (["docs/visual-migration/mobile-map/networks.json"], frozenset({"e2e"})),
        (["crates/platpulse-agent/src/config.rs"], frozenset({"rust", "rust_postgres", "release_package", "rc_harness"})),
        (["crates/platpulse-server/src/http/admin.rs"], ALL),
        (["crates/platpulse-core/src/lib.rs"], ALL),
        (["Cargo.lock"], ALL),
        (["platpulse-web/src/App.tsx"], frozenset({"web", "e2e", "release_package", "rc_harness"})),
        (["platpulse-web/e2e/home.spec.ts", "docs/design/webui.md"], frozenset({"web", "e2e", "release_package", "rc_harness"})),
        (["crates/platpulse-agent/src/a.rs", "platpulse-web/src/b.ts"], ALL),
        # Unrecognised paths and tool files fail safe to every lane.
        ([".github/workflows/ci.yml"], ALL),
        (["scripts/check-logging.py"], ALL),
        (["release/systemd/platpulse-server.service"], ALL),
        (["docs/visual-migration/emerald/x.png"], ALL),
        (["deny.toml"], ALL),
        (["brand-new-dir/file.txt"], ALL),
        ([], ALL),
        (["", "  "], ALL),
    ]
    failed = 0
    for paths, expected in cases:
        got = select(paths)
        if got != expected:
            failed += 1
            print(f"self-test FAILED: {paths!r}: expected {sorted(expected)}, got {sorted(got)}")
    if failed:
        return 1
    rendered = render(frozenset({"web"}))
    if "web=true\n" not in rendered or "rust=false\n" not in rendered:
        print(f"self-test FAILED: unexpected rendering {rendered!r}")
        return 1
    print(f"self-test passed ({len(cases)} cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--all", action="store_true", help="select every lane")
    group.add_argument("--self-test", action="store_true", help="run the built-in cases and exit")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    selected = ALL if args.all else select(sys.stdin.read().splitlines())
    sys.stdout.write(render(selected))
    return 0


if __name__ == "__main__":
    sys.exit(main())
