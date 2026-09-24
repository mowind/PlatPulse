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
 */
export const SURFACE_CARD_STATIC = 'bg-background/60'
export const SURFACE_CARD_INTERACTIVE = 'bg-background/60 hover:bg-background'
// Legacy recipe retained for pages outside the Node-detail migration.
export const SURFACE_CARD = SURFACE_CARD_INTERACTIVE
export const SURFACE_TOOLBAR = 'bg-background/60'
