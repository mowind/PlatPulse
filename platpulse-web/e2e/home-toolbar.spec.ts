import { expect, test, type Locator, type Page } from '@playwright/test'
import {
  closeHomeSurface,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  homeFilterEntry,
  homeFilterSurface,
  homeSearchEntry,
  homeSearchSurface,
  loginAs,
  openHomeFilters,
  openHomeHealth,
  openHomeSearch,
  openHomeSort,
  openHomeValidator,
} from './helpers'

/**
 * The public Home toolbar (issue #232). The row keeps one entry per concern —
 * the Network pills, the Sort & filter entry, the card/list choice, and the
 * search entry — and the controls behind the two entries are not in the row and
 * not in the tab walk until the reader opens the surface that holds them. Every
 * assertion crosses the routed Public Home seam only: accessible roles and
 * names, the address bar, and externally observable layout geometry.
 *
 * This spec runs on every fixed viewport project, so the same contract is read
 * on both phones, the tablet, and both desktops.
 */
test.describe('Public Home toolbar entries (issue #232)', () => {
  const badge = (page: Page) => page.locator('[data-slot="home-filter-count"]')
  const filterDescription = (page: Page) => page.locator('#home-status-filter-count')
  const clearFilters = (page: Page) => page.getByRole('button', { name: 'Clear filters' })
  const clearSearch = (page: Page) => page.getByRole('button', { name: 'Clear search' })
  const row = (page: Page) => page.locator('[aria-label="Node filters and sorting"]')
  const selectedPill = (page: Page) => page.locator('[aria-label="Network filter"] [role="tab"][aria-selected="true"]')

  /** The geometry of the four controls the row itself shows, so a surface that
   *  shifted its siblings is caught rather than described. They are read by name
   *  because the surfaces live inside the same row element: "every button in the
   *  subtree" would let an open surface's own Clear button join the list and hide
   *  a real shift behind an expected addition. */
  const rowControls = (page: Page) => [
    page.locator('[role="tablist"][aria-label="Network filter"]'),
    homeFilterEntry(page),
    page.locator('[role="tablist"][aria-label="View"]'),
    homeSearchEntry(page),
  ]

  /** Document coordinates, not viewport ones: focusing the search field scrolls
   *  the page the few pixels needed to reveal it on a phone-height viewport (the
   *  document height does not change, so the row itself does not reflow), and a
   *  viewport-relative y would report that legitimate scroll as a shifted row. */
  const rowGeometry = (page: Page) =>
    Promise.all(
      rowControls(page).map((control) =>
        control.evaluate((element) => {
          const box = element.getBoundingClientRect()
          return {
            left: Math.round(box.left + window.scrollX),
            top: Math.round(box.top + window.scrollY),
            width: Math.round(box.width),
          }
        }),
      ),
    )

  /** A surface opens below the entry that owns it and never crosses a viewport
   *  edge, at every fixed width. */
  const expectAnchoredInside = async (page: Page, entry: Locator, surface: Locator, what: string) => {
    const entryBox = (await entry.boundingBox())!
    const box = (await surface.boundingBox())!
    expect(box.y, what + ' opens below its entry, not over the row').toBeGreaterThanOrEqual(
      entryBox.y + entryBox.height - 1,
    )
    expect(box.x, what + ' stays inside the left edge').toBeGreaterThanOrEqual(0)
    expect(box.x + box.width, what + ' stays inside the right edge').toBeLessThanOrEqual(
      page.viewportSize()!.width + 0.5,
    )
  }

  /** An open surface reads as an elevated tier, never as a transparent box over
   *  the list behind it. */
  const expectElevated = async (surface: Locator, where: string) => {
    expect(
      await surface.evaluate((element) => getComputedStyle(element).backgroundColor),
      'the open surface paints its own background in ' + where,
    ).not.toBe('rgba(0, 0, 0, 0)')
  }

  /** A bounded interaction-occlusion check, not a paint-order proof: it samples
   *  in-viewport points across the surface and rejects foreign hit-testable
   *  elements covering them, so a card chip or info button that still receives
   *  the pointer at the surface's own pixels fails here rather than only by a
   *  reader looking at the screen. It does not see overlays the hit test is
   *  blind to (`pointer-events: none` paint, as platpulse-web/src/components/ui/data-tooltip.tsx
   *  uses) and overlaps between two samples are missed. */
  const expectUncovered = async (surface: Locator, where: string) => {
    const covered = await surface.evaluate((element) => {
      const box = element.getBoundingClientRect()
      const hits: string[] = []
      for (let column = 1; column < 10; column += 1) {
        for (let row = 1; row < 10; row += 1) {
          const x = box.left + (box.width * column) / 10
          const y = box.top + (box.height * row) / 10
          // Points the surface scrolls past the viewport edges have no hit test.
          if (x < 0 || y < 0 || x > window.innerWidth - 1 || y > window.innerHeight - 1) continue
          const hit = document.elementFromPoint(x, y)
          if (hit && !element.contains(hit)) {
            const slot = hit.getAttribute('data-slot')
            hits.push(`${Math.round(x)},${Math.round(y)} -> ${hit.tagName.toLowerCase()}${slot ? `[${slot}]` : ''}`)
          }
        }
      }
      return hits
    })
    expect(covered, 'nothing outside the surface may cover it in ' + where).toEqual([])
  }

  test('opens each surface below its own entry without leaving the viewport', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })

    // A closed entry holds nothing at all: no select, no field, no surface.
    await expect(page.getByRole('combobox')).toHaveCount(0)
    await expect(page.getByRole('searchbox')).toHaveCount(0)
    await expect(page.getByRole('dialog')).toHaveCount(0)
    await expect(homeFilterEntry(page)).toBeVisible()
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-expanded', 'false')
    await expect(homeSearchEntry(page)).toBeVisible()
    await expect(homeSearchEntry(page)).toHaveAttribute('aria-expanded', 'false')
    await expect(badge(page)).toHaveText('0')

    const before = await rowGeometry(page)

    // The Sort & filter surface is a popover below its own entry, inside the
    // viewport at every width, and it moves no other toolbar control.
    const filterSurface = await openHomeFilters(page)
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-expanded', 'true')
    expect(await rowGeometry(page), 'an open surface must not shift the toolbar row').toEqual(before)
    await expectAnchoredInside(page, homeFilterEntry(page), filterSurface, 'the Sort & filter surface')
    await expect(clearFilters(page)).toBeVisible()
    await expect(await openHomeSort(page)).toBeVisible()
    await expect(await openHomeHealth(page)).toBeVisible()
    await expect(await openHomeValidator(page)).toBeVisible()
    await expectUncovered(filterSurface, 'the open Sort & filter surface')
    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)

    // The search entry opens its own surface the same way, and its field takes
    // the focus a reader needs while the surface is open.
    await closeHomeSurface(page)
    await expect(homeFilterSurface(page)).toHaveCount(0)
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-expanded', 'false')
    const field = await openHomeSearch(page)
    await expect(field).toBeVisible()
    await expect(field).toBeFocused()
    await expect(clearSearch(page)).toBeVisible()
    await expect(clearSearch(page)).toBeDisabled()
    await expectAnchoredInside(page, homeSearchEntry(page), homeSearchSurface(page), 'the Search surface')
    await expectUncovered(homeSearchSurface(page), 'the open Search surface')
    expect(await rowGeometry(page), 'the open search surface must not shift the toolbar row').toEqual(before)
    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)

    // Escape closes the surface and hands the keyboard back to its entry, so the
    // row a reader leaves is the row they started from.
    await page.keyboard.press('Escape')
    await expect(homeSearchSurface(page)).toHaveCount(0)
    await expect(homeSearchEntry(page)).toBeFocused()
    expect(await rowGeometry(page)).toEqual(before)
  })

  test('counts the status filters on the entry and clears only those', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })
    // The count is stated whether or not any filter is on.
    await expect(badge(page)).toHaveText('0')
    await expect(filterDescription(page)).toHaveText('0 of 2 status filters on')
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-describedby', 'home-status-filter-count')

    await openHomeFilters(page)
    // Nothing to clear while both status filters sit on their default.
    await expect(clearFilters(page)).toBeDisabled()

    await (await openHomeSort(page)).selectOption('name')
    await (await openHomeHealth(page)).selectOption('healthy')
    await expect(badge(page)).toHaveText('1')
    await expect(filterDescription(page)).toHaveText('1 of 2 status filters on')
    // The entry still reads "Sort & filter" to a screen reader: the count reaches
    // it as a description instead of a changed name.
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-describedby', 'home-status-filter-count')
    await expect(filterDescription(page)).toHaveText('1 of 2 status filters on')

    await (await openHomeValidator(page)).selectOption('unknown')
    await expect(badge(page)).toHaveText('2')
    // Choosing a value never closes the surface the reader is working in.
    await expect(homeFilterSurface(page)).toBeVisible()

    await expect(clearFilters(page)).toBeEnabled()
    await clearFilters(page).click()

    // Clear filters resets the two status filters and nothing else: the sort
    // order and the Network selection stay where the reader put them.
    await expect(badge(page)).toHaveText('0')
    await expect(filterDescription(page)).toHaveText('0 of 2 status filters on')
    await expect(await openHomeHealth(page)).toHaveValue('all')
    await expect(await openHomeValidator(page)).toHaveValue('all')
    await expect(await openHomeSort(page)).toHaveValue('name')
    await expect(page).toHaveURL(/[?&]sort=name$/)
  })

  test('keeps the search entry marked while a search is in force', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })
    await expect(homeSearchEntry(page)).toHaveAttribute('data-search-state', 'off')
    await expect(page.locator('[data-slot="home-search-active"]')).toHaveCount(0)
    await expect(homeSearchEntry(page)).not.toHaveAttribute('aria-describedby')

    const field = await openHomeSearch(page)
    await field.fill('Node')
    await expect(page).toHaveURL(/[?&]q=Node/)
    // The marker is on the entry itself, so a narrowed list is never mistaken for
    // an empty deployment, and it stays on while the field is out of sight.
    await expect(homeSearchEntry(page)).toHaveAttribute('data-search-state', 'on')
    // The condition is carried by a drawn marker as well as the accent colour,
    // and named for a reader who sees neither.
    await expect(page.locator('[data-slot="home-search-active"]')).toHaveCount(1)
    await expect(homeSearchEntry(page)).toHaveAttribute('aria-describedby', 'home-search-state')
    await expect(page.locator('#home-search-state')).toHaveText('Search is on: Node')
    await page.keyboard.press('Escape')
    await expect(homeSearchSurface(page)).toHaveCount(0)
    await expect(homeSearchEntry(page)).toHaveAttribute('data-search-state', 'on')
    await expect(page.locator('[data-slot="home-search-active"]')).toHaveCount(1)
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-expanded', 'false')

    // Reopening hands the reader back the text they had, and the field's own
    // clear action drops only the query.
    const reopened = await openHomeSearch(page)
    await expect(reopened).toHaveValue('Node')
    await expect(clearSearch(page)).toBeEnabled()
    await clearSearch(page).click()
    await expect(reopened).toHaveValue('')
    await expect(homeSearchEntry(page)).toHaveAttribute('data-search-state', 'off')
    await expect(page.locator('[data-slot="home-search-active"]')).toHaveCount(0)
    await expect(homeSearchEntry(page)).not.toHaveAttribute('aria-describedby')
    await expect(page).not.toHaveURL(/[?&]q=/)
  })

  test('leaves the hidden controls out of the tab walk', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })

    await expect(row(page).getByRole('combobox')).toHaveCount(0)
    await expect(row(page).getByRole('searchbox')).toHaveCount(0)
    await expect(row(page).getByRole('textbox')).toHaveCount(0)

    // From the selected Network pill, Tab meets the entries in reading order and
    // nothing else: the controls behind them are not in the walk.
    await selectedPill(page).focus()
    await page.keyboard.press('Tab')
    await expect(homeFilterEntry(page)).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('tab', { name: 'Cards', exact: true })).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(homeSearchEntry(page)).toBeFocused()

    // Opening an entry puts its own controls into the walk between the entry and
    // the rest of the row.
    await page.keyboard.press('Enter')
    await expect(homeSearchSurface(page)).toBeVisible()
    await expect(await openHomeSearch(page)).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(homeSearchSurface(page)).toHaveCount(0)
    await expect(homeSearchEntry(page)).toBeFocused()
  })

  test('keeps every surface readable and touch-sized in Light and Dark', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })

    for (const scheme of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme: scheme })
      await expect
        .poll(() => page.evaluate(() => document.documentElement.classList.contains('dark')))
        .toBe(scheme === 'dark')

      const surface = await openHomeFilters(page)
      await expectElevated(surface, 'the Sort & filter surface in ' + scheme)
      await expectVisibleInteractiveTargets(page)
      await expectNoHorizontalOverflow(page)
      await closeHomeSurface(page)
      await expect(homeFilterSurface(page)).toHaveCount(0)

      // The search surface answers the same contract in both themes.
      const field = await openHomeSearch(page)
      await expect(field).toBeFocused()
      await expectElevated(homeSearchSurface(page), 'the Search surface in ' + scheme)
      await expectVisibleInteractiveTargets(page)
      await expectNoHorizontalOverflow(page)
      await closeHomeSurface(page)
      await expect(homeSearchSurface(page)).toHaveCount(0)
    }
  })
})
