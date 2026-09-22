# ADR 0006: Show PlatScan Validator Activity on the Home Node card

**Status:** Accepted; implemented by the Home Node-card top-right status badge. This reverses the Home-card decision recorded by commit `288421f` ("refit Home overview, map and Node cards to the Emerald reference"), which deliberately rendered Validator Activity nowhere on a Home card and left Node Health as the only header status cue.

## Context

A Home Node card's top-right corner showed a local Node-role chip derived from `consensus.validator` (Validator / Non-validator). Consensus pool membership is not a Validator's current status, and reading it as one invited exactly the confusion the domain language forbids: Node role, Node Health, consensus membership, Current Validator Status, and Validator Activity are five separate dimensions.

The Server already projects Canonical last-good Validator Activity onto a Public Node through an effective automatic Node Validator Link (#100, #101), matched by Validator identity (the observed full P2P public key) inside the correct Network. No new request, cache, or client-side matching is needed to render it.

## Decision

The Home Node-card top-right badge shows the Node's PlatScan Validator Activity: an SVG icon plus the English status name, with the source and the real update time in a keyboard-, pointer- and touch-operable explanation. The two-state Node Health marker beside the name stays an independent cue, and the local Node role stays in Node detail only.

- The four named states map from PlatScan's numeric `data.status` exactly as the Server already adapts it: `active` (1|2), `producing` (3), `verifying` (6), and `observing` (an authoritative empty staking identity).
- `exiting`, `exited` and `locked` keep their own real names with the neutral treatment rather than being forced into the four named states. An unlisted value keeps its own name too.
- No status is ever inferred from Node role, `isValidator`, Node Health/online state, CPU, sync height, rank, or rewards.

## Consequences

- **`observing` is also the presentation for unavailable evidence.** A `validator` of `null` (no effective Node Validator Link) and an `unknown` Activity both render the Observing state. This deliberately departs from the earlier "Observe is never the default for missing data" phrasing and from the `AGENTS.md` rule that unknown is never shown as a definite value. The visible state is unified; the explanation still names the actual reason (no effective Link, unconfigured deployment, request failure, and so on). It must not be "repaired" into a neutral placeholder without reversing this ADR.
- **Producing is mapped but not yet reachable from real data.** PlatScan's `aliveStakingList` reports the current producer as status 3, but the Server's ranking normalizer reads only `nodeId` and `ranking`, and the `stakingDetails` endpoint has not presented status 3 in the captured evidence. Real data therefore currently yields `verifying` for a producer, and only labelled test data exercises Producing. Which endpoint is authoritative for "currently producing" is a separate Server-side decision and is not resolved here.
- Node Health, consensus membership metrics, Current Validator Status, and the local Node role are untouched. The badge is single-line and no taller than the chip it replaces, so the equal-height Node-card region rules and card height are preserved.
- The badge reuses the existing Public Projection, cache, and SSE-driven refresh channel; it adds no per-card Provider request.

## Related

[ADR 0005](0005-automatic-validator-identity.md) for automatic Validator correspondence and Current Validator Status; [ADR 0002](0002-webui-emerald-visual-authority.md) for the Emerald token and component authority; `CONTEXT.md` for Validator Activity, Current Validator Status, Node Validator Link, and Node Health Summary.
