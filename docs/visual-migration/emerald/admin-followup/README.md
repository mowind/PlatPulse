# Admin follow-up evidence (acceptance fixes)

Captured from local seeded e2e data only (`platpulse-web/e2e/start-server.sh`
and Playwright route fixtures). No production system, credential or token
appears in these images.

| Directory | Code | How it was produced |
| --- | --- | --- |
| `before/` | main `1891643` (unchanged baseline, checked out in a temporary worktree) | `ADMIN_REFINEMENT_EVIDENCE=before npx playwright test e2e/admin-refinement-evidence.spec.ts --project=desktop-1280`, running the corrected evidence spec of this change against the baseline UI |
| `after/` | the commit that adds this README | same command with `ADMIN_REFINEMENT_EVIDENCE=after` |
| `agents-mock/` | before = `1891643`, after = this commit | a throwaway route-mocked probe with six Agents (named, long-named and unnamed) at 1280 and 390, light and dark, with and without the first row's full Agent ID open; used to decide whether the Agents identity change is clearer |

Pages × widths × themes: `agents`, `networks`, `validators`, `sessions` at
360/390/768/1280 in light and dark.

Interaction captures — each one is taken only after the spec asserts the state:

- `agents-diagnostics-*`: the seeded Agent row's `Show diagnostics` button
  (located by accessible name inside that row) reports `aria-expanded="true"`,
  and its controlled detail row with the `Recorded diagnostic evidence` heading
  and list is visible.
- `agents-full-id-*`: `Show full Agent ID` expanded, the full ID visible, Copy
  pressed.
- `networks-genesis-*` (after only — the baseline has no such control): the
  `platon-e2e` row's `Show full Genesis hash` expanded with the full value.
- `nodes-expanded-*`, `audit-expanded-*`: the Node row `Expand …` button and
  the `session_created` Audit row `Show details` button, each asserted
  expanded with its detail region visible.
- `networks-form-*`: the registration form and its Genesis hash field visible.
- `nodes-error-1280-light`: the Node inventory alert after a 503.
- `overview-refresh-error-1280-light`: the refresh-failure message plus the
  unchanged last-good Agent summary.
- `sessions-long|confirm|error|revoke-others-*`: route fixture with a long user
  name and client hint; per-row confirmation open; a 409
  `session_already_revoked` alert; the `Revoke all other Sessions`
  confirmation open.

Correction to the earlier round: `../admin-refinement/*/agents-expanded-*`
were taken with the first `aria-expanded="false"` button in the Agent table,
which is `Show full Agent ID`, not `Show diagnostics`. They show the Agent ID
disclosure and are **not** evidence of the diagnostics disclosure; use
`after/agents-diagnostics-*` instead.

Full-page captures pin the sticky Admin top bar at the scroll position
Playwright used while stitching, so it can appear in the middle of a tall
phone image; this is a capture artifact, not a layout defect.
