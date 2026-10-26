/**
 * Staggered entrance animation parameters for node cards on Home.
 *
 * Visual authority: komari-theme-emerald HomeView.vue (.node-card-switch-enter-active)
 * Registered deviation: Emerald deviation 43 (docs/visual-migration/emerald/README.md)
 * Design authority: docs/design/webui.md §15.39
 * Issue #278: Reference Emerald to implement staggered micro-entrance animation for node cards.
 */

export const NODE_CARD_STAGGER_STEP_MS = 25
export const NODE_CARD_MAX_DELAY_MS = 250

/**
 * Calculates the staggered delay in milliseconds for a node card at `index`.
 * The delay increases linearly by 25ms per card and is capped at 250ms (200~300ms window)
 * to avoid excessive delays on large lists.
 */
export function nodeCardStaggerDelay(index: number): number {
  if (index <= 0) return 0
  return Math.min(index * NODE_CARD_STAGGER_STEP_MS, NODE_CARD_MAX_DELAY_MS)
}
