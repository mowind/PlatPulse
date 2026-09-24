# Node Detail refinement — visual evidence

Screenshots for the Node Detail hierarchy/surface pass at `bb59fe6d8fa2d54426d80255cddba36ec5e968f1`.

Not the full `../DELIVERY.md` matrix. These four only cover the region this pass
changed most, so the Linked Validator work is reviewable without running the
whole refinement harness.

| File | What it shows |
| --- | --- |
| `linked-validator-decided.png` | A decided staking verdict: `Validator` / `Current` / `Producing`, the six-metric grid, icon-only identifier controls, default-collapsed Validator diagnostics. |
| `linked-validator-unknown-staking.png` | The non-verdict: no `Validator status unknown` chip, one muted canonical line with its info control, `Current` and `Verifying` left as the two independent dimensions. |
| `linked-validator-diagnostics-open.png` | The same disclosure expanded, using the shared disclosure component inside the Linked Validator card. |
| `node-detail-full.png` | The whole page at 1440: four summary cards, three observation panels, Linked Validator, the six-chart deck, then the three identically styled collapsed disclosures. |

## Provenance

- Viewport: `desktop-1440` (1440x900); full page for `node-detail-full.png`.
- Served by `platpulse-web/e2e/start-server.sh` (real `platpulse-server`, temporary
  SQLite, production Vite bundle), the same harness `playwright.config.ts` uses.
- Node A from the seeded fixtures, whose enode carries the matching P2P key, so
  the Node Validator Link is discovered automatically rather than mocked. The
  unknown-staking capture overrides only `currentValidatorStatus` on the public
  Node response.
- The committed per-viewport captures under `../screenshots/` are the evidence
  harness's own output; only the two Node Detail pages were refreshed here.
