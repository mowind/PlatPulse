Part of the [Emerald visual migration record](README.md): sections 23–31. Sections keep their numbers; image and evidence links are relative to this directory.

## 23. The map is visible in the evidence now, and the scatter question is answered

The first captures showed an empty band where the map belongs. The cause was not
a bug: the e2e Server's seeded Geo provider is **Disabled**, so the map correctly
renders nothing and states "Peer countries · Disabled by server" to assistive
technology. The evidence capture now enables the Local MMDB provider before
capturing, so the screenshots show the map the acceptance is about.

With the map enabled, the 1440 Home capture shows:

- the world map in the right six columns of the top band, the four statistics in
  the left six, which is Emerald's 6/6 composition;
- the map's upstream faint slate fill and its borders;
- **a scatter symbol with its aggregate numeral** (a green dot labelled 10 around
  the seeded country) and the corner counter at "10" - so the scatter does paint
  on real data. The earlier doubt from a pixel scan is resolved: that scan looked
  at a Disabled provider, where there is nothing to paint by design;
- the world vertically cropped inside the band, which is upstream's own
  composition (its map box is 352px inside a 232px band, shifted up by a sixth).

This is an observation of our own render, not a comparison against upstream's. The
side-by-side review is still the one acceptance step not performed, and no
fidelity claim is made until it is.

## 24. First real comparison: our 1440 Home against the local reference image

The repo carries a local design reference, emerald-maps.jpg (2446x1101, a 2x
capture of roughly 1223 CSS px), which is upstream in maps mode. Comparing it
with our own desktop-1440 Home capture:

**Same** - the world map in the right half of the top band with the statistics in
the left half and the toolbar and node-card grid below; the faint slate basemap
with visible country borders; teal scatter symbols carrying white aggregate
numerals; the white rounded tooltip; the card surfaces, radii, muted label
typography, dotted-leader info rows and the table/heading treatment.

**Content differences (expected)** - the reference shows six resource tiles where
PlatPulse shows four counters (deviation 2, approved); it has many observed
countries where the seeded Server has one; its node cards carry ping strips, cost
rows and tags that PlatPulse has no data for.

**Style differences found, to resolve or record:**

1. **The proportional split differs.** The reference image puts the statistics at
   about 37% of the content width and the map at about 61%. The pinned source
   (v1.0.12) is a 12-column grid with the statistics at columns 1-6 and the map
   at 7-12, which is 50/50 and is what this migration implements. The image and
   the pinned source disagree; the source was followed, and the discrepancy is
   recorded rather than guessed at.
2. **The background grid decoration is not visible in our render.** The reference
   clearly shows the inclined 72x56 grid in the upper left. The classes are
   upstream's literals, so this needs a computed-style probe of the grid SVG
   rather than a guess.
3. **The top wash is fainter in our render** than in the reference.
4. **The header affordance differs**: the reference shows the current theme name
   as a chip ("light theme") beside a sun icon; PlatPulse keeps its icon-only
   theme control with an accessible name.

Items 2 and 3 are the only ones that look like defects of this migration. They are
not yet fixed, and no fidelity percentage is claimed.

## 25. A real regression found by the comparison: the whole background was invisible

Comparing the crops exposed it. The pre-paint canvas added to index.html during
this migration set the background on :root (the html element). CSS only propagates
a BODY background to the canvas while the root element has none; because html had
one, body painted its own opaque white in the normal-flow step and covered the
background decoration, which is a z-index:-1 layer painted before it. The wash and
the inclined grid were therefore never visible on any page, and the translucent
card surfaces had nothing to be translucent over.

Fixed by moving the pre-paint canvas to the body element (html.dark body for the
dark theme). A 1:1 crop now shows the emerald-to-lime wash, the inclined 72x56
grid, and the 60%-opacity cards sitting over both - which is the reference
composition.

This also invalidates every screenshot captured before it: the evidence set was
re-captured, and the full suite was re-run because the change touches a surface
every page renders.

## 26. After the background fix

fixing the background exposed one assertion that had been reading a proxy rather
than the thing itself: the pre-mount canvas check read
getComputedStyle(documentElement).backgroundColor. With the canvas colour now
propagating from the body (as it must, for the decoration to be visible), the root
element is transparent and the assertion failed even though the first paint is
correct. It reads the effective canvas colour now - the root's background, or the
body's when the root is transparent - which is what "the first paint is the theme
background" means. theme.spec.ts: 60 passed, 0 failed.

The two style defects from section 24 are therefore fixed: the inclined grid is
visible and the wash reads at the reference's strength. The remaining recorded
differences are the 4-vs-6 statistics (approved content), the map box height, the
header's theme-chip affordance, and the statistics-to-map split where the pinned
source and the reference image disagree.

## 27. Reviewing the rest of the evidence

Login and Node Detail at desktop-1440 were reviewed against the same reference.

**Login** now shows the wash and the inclined grid behind a centred, cardless form
column: muted label typography above each field, Emerald's rounded input surface,
its neutral near-black primary action ("Sign in"), and the theme control in the
corner. That matches upstream's treatment of ordinary primary actions.

One structural difference: **the reference's App shell renders its header, and so
its brand, on every route including the login page; PlatPulse's login page renders
only the theme control.** Recorded rather than changed, because PlatPulse's login
page is its own surface and the e2e asserts its current shape.

**Node Detail** shows the accepted A container: an uncarded identity block, four
summary tiles, three parallel observation panels, then the six 60-second charts.
The Host resources panel carries the "collected once per Agent; shared by every
Node it monitors" note and its own progress bars, so the Host de-duplication rule
is visible on the page rather than only in the data layer. Unknown values read
Unknown, never zero.

Two evidence-quality notes, both capture artefacts rather than product defects:
the charts were caught while their history query was still resolving (the capture
now waits for that placeholder to clear), and the chart loading placeholder is
what the seeded Server shows when no history has been retained yet.

## 28. Human-approved composition decisions

The continuation decision “37/61、232、加页头” resolves the open split, height, and
Login header questions above. Home now uses 37:61 fractional tracks after the
existing 8px gap at md and above, rather than upstream’s equal six-column slots.
The outer band remains 232px including padding; mobile stacking is unchanged.
This is an explicit reference-image exception to the pinned-source composition.

Login now includes the PlatPulse brand/Home link and theme toggle in the 56px
Emerald header, with the same 1280px alignment and scroll blur as Home. The form
sits in the remaining viewport space. No Home queries, Admin controls, auth
changes, or new brand assets are introduced. Earlier sections describe the
historical review state, not unresolved decisions.

Added regression coverage checks the Login banner and access boundary, both
theme layouts and touch targets, the 37:61 measured track ratio, the 232px band,
and unchanged phone track widths. No existing assertions or visual baselines
are removed or re-recorded.

Validation for this slice: lint, typecheck, all 250 unit tests and production
build pass. The five-project run of emerald-decisions, shell, theme,
home-convergence and home-geo-map enumerated 195 scenarios and exited 0 with
no failed tests (viewport-specific skips retained). CI=1 forced a fresh real
Server rather than reusing an old build. The newly captured 768px and 1440px
Home images were inspected for the new split and summary/map fit. This is a
targeted regression run, not a new full-suite result; remaining evidence review
and the checkbox/radio indicator refinement were follow-up work at this point.

> **Superseded layout:** the 37:61 tracks and 232px band recorded in this section
> were replaced by the six-card overview in **deviation 11** (4:3 statistics/map
> at ≥1024px, a contained 2:1 map, statistics above the map below that). This
> section remains as the record of that earlier, human-approved composition.

## 29. Compact native selection indicators

`Checkbox` and `Radio` now share a native-backed selection primitive. The actual
input remains 44×44px and keyboard-focusable; its pointer-inert, aria-hidden
indicator is 16×16px. CSS follows native checked, indeterminate, focus-visible,
invalid and disabled states, including a disabled fieldset. There is no duplicate
React state and no custom click/key handler. Forced-colors mode deliberately
restores the browser-native control and hides the decorative indicator. This
completes deviation 10’s oversized-checkbox/radio follow-up without reducing
touch targets.

Settings uses the Radio primitive without changing provider selection or save
behavior. Checkbox consumers are retained, currently unrouted Agent recovery
and rotation components; no route or remote-control feature was added to expose
them. Nine primitive tests cover native props/refs, controlled and uncontrolled
state, labels, required validation, disabled fieldsets, form values/reset, radio
grouping and indeterminate state. Browser coverage tests the shared geometry
and radio keyboard behavior on the real Settings route: 16px indicator, 44px
input, activation from the input corner, arrow navigation, visible focus ring,
and forced-colors fallback. Both themes and all five fixed viewports pass.

The explicit evidence run passed all 20 tests and refreshed five pages per
viewport (25 PNGs): Login, Home, Node detail, Admin overview and
Settings. These remain review evidence, not screenshot assertion baselines.
Final full-suite validation and the visual-review findings are recorded below.

## 30. The remaining screenshot review found real gaps

All 30 light-theme images were inspected across the five projects. Login,
Settings and Home had no apparent overlapping surfaces. Inspection did not
claim pixel parity with upstream or replace keyboard/geometry assertions.

Three findings required action:

- **Capture readiness:** some Node images contained only Loading Node or
  stopped at the first chart row, and some Network captures ended mid-card.
  Waiting for the absence of Loading metric history before the Node exists
  can succeed immediately. The evidence script now positively requires the
  Node title, six metric cards and summary before requiring settled history;
  Network title/panel and a painted map are also mandatory. Absent route links
  and failed readiness checks no longer silently skip evidence or only warn.
- **Missing-value copy:** the Node directory helper concatenated an optional
  formatter result into a string, producing literal `undefined`. A call-site
  `Unknown` fallback fixes that without changing the shared formatter or
  converting unobserved values to zero. Eight rendered cases cover missing,
  partial, zero, non-zero and retained stale data; the missing cases reproduced
  the exact defect before the one-line fix.
- **Overview responsiveness:** migration commit `ff1c0e3` dropped Node/Agent
  cell labels and phone priority-card styling, leaving unconditional table
  minimum widths; the new Button also made long Node labels nowrap. This is
  a migration regression, not an intentional phone scroller. A new browser
  regression measures cells against the visible wrapper, not merely against
  the document, so a hidden right-hand column cannot pass by hiding overflow.

The earlier full browser run was deliberately stopped on its obsolete build
when these defects were confirmed; it is not reported as a passing run.
Compact Home metric labels still use upstream-style truncation and can be very
short beside large values; that legibility tradeoff is disclosed rather than
claimed as full-label parity. Complete values remain in the semantic content.

### Corrections and re-review

Overview now opts into the existing Emerald phone table stacking and restores
all twelve cell labels. Desktop minimum widths apply only from md; tablet
inner scrolling remains supported. Node names wrap within a 14rem upper bound
without replacing desktop auto table layout. Agent column shares restore main’s
16/13/15/21/22/13 proportions, with Overview’s field order explicitly preserved.
The new test failed before this repair on phone360 and desktop1280; after it,
all five projects passed, and the new plus existing Overview suite produced
35 passes and 10 existing skips. The Spool label now also refuses to shrink
into a vertical string of letters while its value wraps; a one-line-height
geometry assertion protects that correction.

The settled capture run passed all five projects. Re-review confirmed loaded
Node detail with all six charts, disclosures and footer; complete Network
cards/actions/footer; bounded desktop Overview columns; and all fields/actions
in the restored phone priority cards. Phone Overview is necessarily long
(about 11,000px with this fixture). Its PNGs exceed the image tool’s 8192px
side limit, so lossless 2400px vertical slices were reviewed consecutively;
source images were not downscaled or cropped.

The two broad dark Validator rank bars are the existing minimal SVG chart:
comparison to main confirms unchanged rank/bar math, currentColor fill,
opacity, and title/table alternative. Sparse rank samples create broad bars;
this is not a lost chart asset. Compact metric/helper truncation, very tall
phone Overview pages, and tablet-local table scrolling remain disclosed
readability tradeoffs. A fixture can also show a receipt timestamp with null
report sequence; the existing “Never received” sequence sublabel is an
independent semantic ambiguity, not changed in this visual slice.

### Full-run failure attribution and test synchronization

The first complete corrected-build run produced **522 passed, 67 skipped,
1 failed**. Its only failure was `SCN-NODE-LAST-GOOD-REFRESH` on desktop1440
before the initial Node heading was visible. The test injected 503 after the
first request, conflating initialization requests with the intended refresh.
Ten isolated repetitions did not reproduce the timing failure, so the exact
extra-request trigger is not claimed as proven. The fixture now starts
failing only after the initial heading and Peer disclosure are ready,
immediately before its explicit invalidation event. Production refresh/cache
code and every last-good assertion are unchanged. This phase-gated scenario
passed **50 repetitions** (ten at each fixed viewport). No retries, timeout
increases, skips or assertion removals were used to make the test pass.

## 31. Final continuation acceptance

- `npm run lint` and `npm run typecheck`: pass on final source.
- `npm test`: **267 passed**, 24 files.
- Production build: pass, rebuilt by each fresh real-Server E2E harness.
- Final full five-project `CI=1 npm run test:e2e`: **523 passed, 67 skipped,
  0 failed** in 15.7 minutes. Skips are the existing viewport/explicit-evidence
  conditions, not new exclusions.
- Final explicit Overview geometry + screenshot run: **10 passed**; 30 PNGs
  refreshed. Spool’s final unbroken label was checked visually and by measured
  one-line height across all five projects.
- `git diff --check`: pass. Test Server listener released after completion.

Local logs: `/tmp/platpulse-final-unit-confirmation.log`,
`/tmp/platpulse-final-polish-evidence.log`,
`/tmp/platpulse-last-good-gated.log`, and
`/tmp/platpulse-emerald-final-acceptance.log`. The earlier failed/cancelled
runs remain attributed above rather than being relabelled as successes.

No Rust implementation, API contract, auth logic or Server trust boundary was
changed; Rust checks and OpenAPI regeneration were not rerun for this
presentation/test-only slice. Changes remain local; nothing was pushed and no PR
was created. (Committed to `main` as `a2d28ea` after this entry was written.)
