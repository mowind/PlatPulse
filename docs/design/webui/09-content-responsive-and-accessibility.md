Part of the [PlatPulse WebUI design](../webui.md): §9–§10. Sections keep their numbers; cite them as `webui.md §X.Y`.

## 9. Content, privacy, and redaction

The WebUI must not display or store in browser state:

- Agent credentials or any one-time provisioning material;
- session tokens or CSRF values in URLs;
- passwords, TLS private keys, or pepper values;
- raw Peer addresses or identity lists. Peer Snapshots and Presence exist behind Server projections, but Public/Admin WebUI surfaces show only redacted current/aggregate data;
- complete RPC Endpoints, complete request bodies, stack traces, or internal paths.

Errors name the failed user task and next safe action. They do not expose stack traces, secret contents, or internal paths. Confirmation copy states what will and will not change — in particular for the history window: shortening deletes data; lengthening cannot recover it.

Relative times can expand to absolute UTC or selected timezone. Server timestamps remain authoritative.

## 10. Responsive and accessibility contract

### 10.1 Navigation

- desktop: persistent Admin sidebar and context;
- tablet: collapsible navigation with context preserved;
- mobile: accessible drawer or equivalent navigation;
- drawer opening moves focus inside, traps Tab focus, closes on Escape, restores focus to opener, and locks body scroll (restoring the earlier inline overflow when it closes);
- the drawer exists only below the sidebar breakpoint (Tailwind `lg`, 64rem — `ADMIN_SIDEBAR_QUERY`, the same query the `lg:` classes use): crossing into the sidebar layout while it is open ends it without moving focus to the then-hidden Menu button, releases the scroll lock, and does not reopen it when the width shrinks again; the desktop sidebar never traps Tab or intercepts Escape;
- no critical action depends on hover;
- the theme control's accessible name states the current mode and the next action, and it is at least a 44×44 target in every shell;
- browser back/forward preserves URL filters and detail context;
- Admin page-group links carry one leading decorative glyph that inherits the label colour. The glyph is `aria-hidden`, so the link's accessible name remains the visible page-group label; it never carries status meaning (§10.3) and never replaces the label. The retained MVP mapping is:

| Page group | Glyph | Code point |
|---|---|---|
| Overview | ▦ | U+25A6 |
| Agents | ◈ | U+25C8 |
| Nodes | ◉ | U+25C9 |
| Networks | ⬡ | U+2B21 |
| Settings | ⚙ | U+2699 |
| Sessions | ◫ | U+25EB |
| Audit | ☷ | U+2637 |

### 10.2 Layout

- forms are single-column at narrow widths;
- 44×44 CSS pixel target is the baseline for touch controls;
- tables become priority cards/rows on narrow screens;
- critical fields remain visible: status, subject/Node, head/sync, freshness, primary reason, next action;
- detail/evidence uses expansion or a dedicated route;
- sticky action areas never cover errors, fields, or keyboard focus;
- 200% zoom, landscape, and portrait remain functional.

### 10.3 Semantics

- one logical `h1` per page and ordered headings;
- semantic `form`, `label`, `table`, and `caption` where applicable;
- visible focus on keyboard navigation;
- status has text plus icon/equivalent, never color only;
- validation has field-level messages and page summary;
- live regions are polite and limited to meaningful transitions;
- reduced motion removes non-essential animation;
- touch tooltips have a non-hover alternative.
