# ADR 0003: Adaptive Admin workspace within Emerald

Status: Accepted

## Context

ADR 0002 migrated both surfaces to Emerald. The current implementation and
shell ultrawide assertion apply a centered 1280px column to Admin, both in the
shell and again in several pages. The header centers against the viewport while
the body centers against the space remaining after navigation. This was a prior
visual migration choice, not a business invariant.

## Decision

Supersede the Admin-only centered 1280px rule. Keep Emerald tokens, typography,
icons, BackgroundDecoration, shared controls and realtime vocabulary unchanged.
Public Home, map, Node cards and public content widths are not affected.

AdminLayout owns sidebar reservation, available body width and page padding.
Use one 13.5rem sidebar variable in header and navigation, with 24px desktop
body/header-workspace padding (16px narrow). The header brand occupies the
sidebar column; status/actions occupy the remaining workspace. Pages fill that
workspace without repeating centering or page-level maximum widths. Forms may
retain local reading-width limits beneath normally aligned headings.

Retain mobile drawer behavior, min-width: 0 shrink boundaries, local table
scrolling and data-stack transformations. Do not hide overflow to obtain alignment.
Overview keeps its module order, query isolation, server-owned status/times,
summary limits, diagnostics disclosure and keyboard/focus behavior.

## Verification

Replace the shell's old ultrawide cap/centering assertion with remaining-width
and header/body alignment assertions at 1280, 1440 and 1920px. Preserve behavioral
tests. Add a refresh test where Overview completes while another query remains
pending. Capture matched theme/viewport/data evidence before and after changes.

Theme tests that still expected zero Admin decoration are aligned with the
explicit retained-background contract: assert one aria-hidden decoration and
no public Geo chart in Admin. This preserves the checks rather than deleting
failing assertions or removing the existing background to satisfy an old rule.
