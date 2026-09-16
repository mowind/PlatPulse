# Validation record

All commands run from `platpulse-web`, against local temporary development data.
No production API test writes, branch resets, commits or screenshot-baseline updates.

## Static / unit / build

- `npm run lint`: passed.
- `npm run typecheck`: passed.
- `npm test`: 26 files / 307 tests passed. Existing React `act` environment warnings
  remain in output; they are not suppressed.
- `npm run build`: passed. Existing >500kB bundle warning remains (ECharts).
- `git diff --check`: passed.

New unit coverage holds Nodes and Agents separately pending after Overview finishes,
asserts the Overview query is idle, and checks disabled state, spinner and refreshing
label until the final query resolves. New Attention coverage checks two Critical
items on one subject, the additional Critical count, visible discarded reports,
full-ID disclosure/copy, and the safe Agent detail route. Existing cache isolation,
permission, unknown/empty/last-good, independent panel error and disclosure tests
were retained. A first full unit run exposed an exact identity text lookup; keeping
identity in its own span restored the original session-cache test without removing
or weakening its assertion.

## Browser coverage

The selected regression command is:

```sh
E2E_PORT=4273 npx playwright test \
  e2e/shell.spec.ts e2e/admin-overview.spec.ts \
  e2e/admin-list-matrix.spec.ts e2e/admin-workspace.spec.ts \
  e2e/overview-responsive-evidence.spec.ts e2e/theme.spec.ts \
  e2e/home-convergence.spec.ts e2e/mobile-map-responsive.spec.ts
```

The new workspace matrix explicitly visits Overview, Agents, Nodes, Networks,
Settings, Sessions and Audit at 360/390/768/1280/1440/1920 in both themes, checking
heading alignment, root width and no page overflow. The shell test replaces the
old cap assertion (ADR 0003) with shared brand/sidebar and status/body alignment
and available-width assertions at 1280/1440/1920/2560. No business assertions were
removed. Overview tests exercise SSE expansion persistence, Escape/focus restore,
offline recovery, mobile drawer, touch targets, reduced motion and zoom/reflow.
The long-name table test retains data-stack and column/overflow checks. Added
sibling-target geometry checks caught a real 768px overlap: minmax(0,1fr) could
allocate less than the button’s 44px target. The local Grid now reserves that
minimum plus a 12rem desktop/tablet identity-cell reading width; the table keeps
its existing local scroller. The strengthened test passes at all five projects.

Evidence runs: before and after each passed (12 captures each, frozen identical
REST data/time). These are full-page image artifacts, not pixel-diff approvals.
No manual screen-reader audit, real-device testing or exhaustive visual contrast
review is claimed.

## Existing differences / test-harness limitations

- Initial selected suite: **160 passed, 17 failed, 33 parameter-gated skips**
  (210 cases, 6.5 minutes). All shell/Overview/Agent-Audit matrices and the new
  seven-page workspace matrix passed. Public mobile-map responsive checks passed.
- Fifteen failures came from two `theme.spec.ts` bodies still expecting zero
  Admin BackgroundDecoration elements (three cases × five viewports). Baseline
  reruns confirmed the contradiction with the existing single decoration and
  shell assertion. ADR 0003 explicitly retains the decoration: these assertions
  now require **one aria-hidden decoration and no public Geo chart**, retaining
  all typography, theme, readability, form and keyboard checks. No failed test
  was deleted or disabled.
- Two initial Home convergence failures were on the phone projects. At 360 the
  old test requires an 80–120px summary card, but the already-migrated public card
  is 62px. At 390 an initial login timeout interrupted the assertion; the isolated
  follow-up reaches the same **62px versus >=80px** mismatch. A separate browser
  measurement against both baseline and modified bundles confirms **62px before
  and after at 360px** (`public-summary-geometry.json`). Public implementation is
  untouched; the old public layout assertion is left for its own migration scope,
  not weakened to obtain a green run. The earlier baseline full Home test also
  encountered an aged-fixture Healthy-label mismatch, so it is not presented as
  an exact reproduction of the card-height assertion.
- The initial browser attempt reused a pre-existing test server and could not
  reach the expected login state. Testing moved to a new temporary Server on 4273.
  Another preliminary run encountered cached entry HTML after rebuilding;
  it was interrupted, the owned test Server restarted, and the suite rerun.
  Final baseline evidence uses a separate baseline bundle/Server/database and
  asserts the old cap so a wrong-bundle capture cannot silently pass.
- Parameter-gated tests in existing suites remain gated (for example desktop-only
  SSE mutation and single-project explicit-width matrices); these are not passes.
  The new read-only width matrix likewise executes once rather than five times.
- Back-end/Rust checks and the entire repository-wide e2e suite were not run:
  no backend code, generated client, authorization or API contract changed.

## Follow-up executions

- Updated-theme assertions: 15/15 passed across the five fixed projects before
  the final local Node-cell sizing refinement; repeated on the final build below.
- Strengthened long-name / non-overlapping Node actions: 5/5 passed on the final
  build, including 768px (no assertion weakened after finding the overlap).
- The final refinement was followed by lint, typecheck, all 307 unit tests and
  build again; all passed. Modification-after screenshots were recaptured.

- Final Admin/core rerun (shell, Overview, Agent/Audit matrices, workspace, long-name
  actions): **91 passed, 29 parameter-gated skips, 0 failed** (120 cases).
- Final-build theme/readability follow-up: **15 passed, 0 failed** across all five
  fixed projects. The two phone Home summary-height assertions remain unresolved
  outside the Admin-focused reruns; the overall repository e2e suite is **not**
  claimed green.
