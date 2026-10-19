# Admin Node / Network editor state — region captures

Scope: the editor-state round for the Node `Server-owned metadata` rename and
the Network `Expected identity tuple` edit. Only button state and copy
changed, so only those two panels are captured — no full Admin visual
re-acceptance.

Captured by `e2e/admin-editors.spec.ts` with `ADMIN_EDITORS_EVIDENCE=1`
against the local seeded e2e Server, at 390 (phone-390-touch) and 1280
(desktop-1280), Light and Dark; the spec's behaviour assertions run on all
five fixed projects. Every write in those runs is a local
Playwright route that is held open and then answers 503, so no Server state
was changed and no production service was touched.

| File | State |
|---|---|
| `node-rename-saving-*` | Rename in flight: `Saving…` keeps focus and is `aria-disabled`; input, Keep editing and Cancel disabled |
| `node-rename-failed-*` | Failed rename: draft kept, confirmation still open for a retry, error shown |
| `network-tuple-saving-*` | Tuple update in flight, with the complete risk statement beside it |
| `network-tuple-failed-*` | Failed tuple update: draft kept, controls usable again, error shown |

Behaviour is asserted by the spec itself and by the unit tests in
`src/pages/AdminNodes.test.tsx` (`Server-owned display name editor`) and
`src/pages/AdminNetworks.test.tsx` (`Expected identity tuple editor`).
