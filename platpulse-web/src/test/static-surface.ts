import { expect } from 'vitest'

/**
 * Assert a read-only working surface does not advertise hover interactivity.
 *
 * Read-only Admin containers use SURFACE_CARD_STATIC (bg-background/50) and must
 * gain no hover glow, opacity change, or lift; interactive recipes keep those
 * affordances. Living here keeps the shared contract in one place (issue #199).
 */
export function expectStaticReadOnlySurface(element: Element): void {
  expect(element.className).not.toContain('hover:shadow')
  expect(element.className).not.toContain('hover:bg-background')
  expect(element.className).not.toContain('hover:-translate-y')
  expect(element.className).toContain('bg-background/50')
}
