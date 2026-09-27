# ADR 0006: Show PlatScan Validator Activity on the Home Node card

**Status:** Accepted; implemented by the Home Node-card top-right status badge. Amended 2026-09-27 to show PlatScan `candidate` distinctly from `active` (see Amendment history). This reverses the Home-card decision recorded by commit `288421f` ("refit Home overview, map and Node cards to the Emerald reference"), which deliberately rendered Validator Activity nowhere on a Home card and left Node Health as the only header status cue.

## Context

A Home Node card's top-right corner showed a local Node-role chip derived from `consensus.validator` (Validator / Non-validator). Consensus pool membership is not a Validator's current status, and reading it as one invited exactly the confusion the domain language forbids: Node role, Node Health, consensus membership, Current Validator Status, and Validator Activity are five separate dimensions.

The Server already projects Canonical last-good Validator Activity onto a Public Node through an effective automatic Node Validator Link (#100, #101), matched by Validator identity (the observed full P2P public key) inside the correct Network. No new request, cache, or client-side matching is needed to render it.

## Decision

The Home Node-card top-right badge shows the Node's PlatScan Validator Activity: an SVG icon plus the English status name, with the source and the real update time in a keyboard-, pointer- and touch-operable explanation. The two-state Node Health marker beside the name stays an independent cue, and the local Node role stays in Node detail only.

- The named states map from PlatScan's numeric `data.status`: `candidate` (1), `active` (2), `producing` (3), `verifying` (6), and `observing` (an authoritative empty staking identity). `candidate` and `active` are distinct upstream states and are never collapsed into one visible status.
- `exiting`, `exited` and `locked` keep their own real names with the neutral treatment rather than being forced into the named states. An unlisted value keeps its own name too.
- Switching status never reflows the card: all named states share one frame — position, size, padding and a fixed 14px, 2px-stroke `currentColor` icon slot — while the glyph and the tone (foreground plus its matching border and background tint) name the state. `candidate` is an outline chip — transparent background, a lighter Emerald border, and the solid per-theme `emerald-700`/`emerald-400` foreground — with the `user-check` glyph; `observing` stays neutral; `active` keeps Producing's background tint and a lighter border, and takes a solid Emerald foreground chosen per theme (`emerald-700` on light, `emerald-400` on dark) instead of one opacity, so the text and waveform stay legible on either surface without a brighter background, glow or solid fill.
- No status is ever inferred from Node role, `isValidator`, Node Health/online state, CPU, sync height, rank, or rewards.

## Consequences

- **`observing` is also the presentation for unavailable evidence.** A `validator` of `null` (no effective Node Validator Link) and an `unknown` Activity both render the Observing state. This deliberately departs from the earlier "Observe is never the default for missing data" phrasing and from the `AGENTS.md` rule that unknown is never shown as a definite value. The visible state is unified; the explanation still names the actual reason (no effective Link, unconfigured deployment, request failure, and so on). It must not be "repaired" into a neutral placeholder without reversing this ADR.
- **Producing is mapped but not yet reachable from real data.** PlatScan's `aliveStakingList` reports the current producer as status 3, but the Server's ranking normalizer reads only `nodeId` and `ranking`, and the `stakingDetails` endpoint has not presented status 3 in the captured evidence. Real data therefore currently yields `verifying` for a producer, and only labelled test data exercises Producing. Which endpoint is authoritative for "currently producing" is a separate Server-side decision and is not resolved here.
- Node Health, consensus membership metrics, Current Validator Status, and the local Node role are untouched. The badge is single-line and no taller than the chip it replaces, so the equal-height Node-card region rules and card height are preserved.
- The badge reuses the existing Public Projection, cache, and SSE-driven refresh channel; it adds no per-card Provider request.

## Amendment history

- 2026-09-27: split PlatScan status 1 (`candidate`) from status 2 (`active`). Both had mapped to the single canonical `active` Activity, so a Node whose upstream status was `candidate` rendered as Active on Home. `CONTEXT.md` now lists Candidate as its own Validator Activity, the Server's `platscan_status_activity` maps 1 to `candidate` and 2 to `active`, and the badge gained the `candidate` outline treatment. `classify_activity` still treats Candidate as a currently valid staking identity (`Validator`), so only the visible Activity is split, not Current Validator Status.

## Related

[ADR 0005](0005-automatic-validator-identity.md) for automatic Validator correspondence and Current Validator Status; [ADR 0002](0002-webui-emerald-visual-authority.md) for the Emerald token and component authority; `CONTEXT.md` for Validator Activity, Current Validator Status, Node Validator Link, and Node Health Summary.
