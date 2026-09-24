/**
 * Surface recipe, ported from komari-theme-emerald @ c2c5e88
 * src/composables/useBackgroundSurface.ts.
 *
 * Upstream switches between two variants depending on whether the Owner has
 * configured a custom background image/video: the plain recipe uses
 * bg-background/60, the custom one drops to /50 and adds a backdrop blur
 * because a photograph sits behind the card. PlatPulse has no custom-background
 * setting — the Emerald gradient is the default surface — so the plain recipe
 * is the one that matches upstream's rendered default.
 *
 * Node Detail splits that one recipe into the three reading tiers the page
 * needs. Each value already exists upstream or in this tree — /60 is the plain
 * recipe, /50 is upstream's custom-background value, /40 is the existing
 * nested-note surface — so the hierarchy adds no new colour, only ordering:
 *
 *   summary cards  /60  the page's most important read
 *   info / charts  /50  the working surface
 *   disclosures    /40  collapsed, dimmest, least authoritative
 *
 * The separation is deliberately only five points of background opacity per
 * step; it must read as depth, not as differently coloured blocks.
 */
export const SURFACE_CARD_SUMMARY = 'bg-background/60'
export const SURFACE_CARD_STATIC = 'bg-background/50'
export const SURFACE_CARD_DISCLOSURE = 'bg-background/40'
export const SURFACE_CARD_INTERACTIVE = 'bg-background/60 hover:bg-background'
// Legacy recipe retained for pages outside the Node-detail migration.
export const SURFACE_CARD = SURFACE_CARD_INTERACTIVE
export const SURFACE_TOOLBAR = 'bg-background/60'
