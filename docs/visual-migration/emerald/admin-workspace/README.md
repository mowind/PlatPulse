# Admin adaptive workspace evidence

## Revisions and environment

- Before: `d08b2a82e42cbdf399a5e3ef732c865eedd7f86e` on
  `feat/webui-emerald-visual-migration`.
- After: **uncommitted working tree** on that same commit; no branch reset or
  commit was made. The pre-existing untracked `increct-maps.jpg` was untouched.
- Real development Server with temporary SQLite data only. Current-build tests
  use localhost:4273; the final baseline capture uses a separately served saved
  baseline production bundle on localhost:4473 with its own temporary database.
  No test mutation targets the production Server.
- Chromium, widths 360/390/768/1280/1440/1920; viewport heights 844 for phones,
  900 otherwise. Both Light and Dark; full-page captures (image heights vary
  with content). Browser time is fixed at `2026-08-12T08:01:00Z` for both builds.
- `fixtures/` contains the same frozen, real test-server Overview/Nodes/Agents
  REST responses for both captures. Timestamps are not rewritten to look fresh.
  They are screenshot fixtures, not production observations. Authentication and
  realtime still use the local test Server.
- `e2e/admin-workspace-evidence.spec.ts` records evidence, not screenshot gates;
  it also asserts no horizontal page overflow and the actual 1920px workspace
  widths: before 1280px, after 1656px. It does not update image baselines.

## Matched screenshot pairs

| Width | Light before / after | Dark before / after |
|---|---|---|
| 360 | [before](before/360-light.png) / [after](after/360-light.png) | [before](before/360-dark.png) / [after](after/360-dark.png) |
| 390 | [before](before/390-light.png) / [after](after/390-light.png) | [before](before/390-dark.png) / [after](after/390-dark.png) |
| 768 | [before](before/768-light.png) / [after](after/768-light.png) | [before](before/768-dark.png) / [after](after/768-dark.png) |
| 1280 | [before](before/1280-light.png) / [after](after/1280-light.png) | [before](before/1280-dark.png) / [after](after/1280-dark.png) |
| 1440 | [before](before/1440-light.png) / [after](after/1440-light.png) | [before](before/1440-dark.png) / [after](after/1440-dark.png) |
| 1920 | [before](before/1920-light.png) / [after](after/1920-light.png) | [before](before/1920-dark.png) / [after](after/1920-dark.png) |

## Source audit and intentional scope

- The shell and Overview/Agents/Nodes/Networks/Settings repeated centered page
  maximums. Sessions and Audit already used full-width roots and need no edits.
  Local Settings, Network and Agent forms retain their reading-width limits.
- Header/sidebar share `--admin-sidebar: 13.5rem`; header workspace and main use
  `--admin-padding` (24px desktop, 16px narrow). See ADR 0003, which supersedes
  the Admin 1280px cap rather than labelling the old choice a defect.
- Overview already aggregates three independent fetch states but used only the
  Overview state for Refresh text. Disabled, spinner and label now agree.
- Server ordering, grouping, severity selection, observed_at, generated_at,
  last-good values, error isolation, summary limits and full-list links remain.
- Attention leads with severity/message/detail navigation; identity/time and
  additional-issue disclosure follow. Extra Critical count and the actual
  `agent_spool_overflow` report-discard message remain visible. Full Agent ID
  has native disclosure, selectable text and a copy button with failure feedback.
- Node name expansion and detail navigation are sibling targets in one grid
  row; ID is below. Existing keyed expansion, Escape and focus restoration stay.
  Head digits are not coerced to Number or abbreviated, and do not wrap internally.
- Agent inventory already had a short-ID primary link and full-ID title/detail
  access; these are retained. Its secondary text is now 12px rather than 11px.
  Reporting, resources, risk and retained-Node semantics remain untouched.
- Shared Button, RealtimeNotice, API queries, emerald.css, public Home, maps and
  Node cards are unchanged. Emerald BackgroundDecoration is intentionally retained.

## Reproduction

From `platpulse-web`, serve the desired build through a local throwaway Server,
then run (choose `before` or `after`, and the matching local port):

```sh
E2E_PORT=4473 ADMIN_WORKSPACE_EVIDENCE=before npx playwright test e2e/admin-workspace-evidence.spec.ts --project=desktop-1280
E2E_PORT=4273 ADMIN_WORKSPACE_EVIDENCE=after npx playwright test e2e/admin-workspace-evidence.spec.ts --project=desktop-1280
```

Existing frozen fixtures are reused. The Server caches its entry HTML: restart it after building,
or use a separate Server per bundle. Do not point this harness at production.

Test results and remaining differences are recorded in `validation.md`.
