import { expect, test, type Locator, type Page } from '@playwright/test'
import {
  closeHomeSurface,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  homeFilterEntry,
  homeFilterSurface,
  homeSearchEntry,
  HOME_SEARCH_TRACK,
  homeSearchField,
  homeSearchInline,
  homeSearchSlot,
  homeSearchSurface,
  homeToolbarShowsField,
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
 * on both phones, the tablet, and both desktops — including the band the sticky
 * header keeps for itself when the row is scrolled up into it.
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
   *  a real shift behind an expected addition. The search control is read in the
   *  shape the toolbar shows it (#241): the row's own field, or the entry that
   *  opens that field. */
  const searchControl = async (page: Page) =>
    (await homeToolbarShowsField(page)) ? homeSearchInline(page) : homeSearchEntry(page)
  const rowControls = async (page: Page) => [
    page.locator('[role="tablist"][aria-label="Network filter"]'),
    homeFilterEntry(page),
    page.locator('[role="tablist"][aria-label="View"]'),
    await searchControl(page),
  ]

  /** Document coordinates, not viewport ones: focusing the search field scrolls
   *  the page the few pixels needed to reveal it on a phone-height viewport (the
   *  document height does not change, so the row itself does not reflow), and a
   *  viewport-relative y would report that legitimate scroll as a shifted row. */
  const rowGeometry = async (page: Page) =>
    Promise.all(
      (await rowControls(page)).map((control) =>
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

  /** The room the row has left for its search control, worked out from the row's
   *  own geometry rather than read back from the implementation: the row's own
   *  width, minus the Network pills at their content width, minus the two other
   *  controls at the widths they are drawn, minus the 8px gaps between them all.
   *  That is the number the stylesheet's own 16rem asks about. */
  const searchRoom = (page: Page) =>
    page.evaluate(() => {
      const row = document.querySelector('[data-slot="home-toolbar"]') as HTMLElement
      const slot = row.querySelector('[data-slot="home-search-slot"]')
      const used = Array.from(row.children)
        .filter((child) => child !== slot)
        .reduce((total, child) => {
          const element = child as HTMLElement
          return total + (element.getAttribute('aria-label') === 'Network filter'
            ? element.scrollWidth
            : element.getBoundingClientRect().width)
        }, 0)
      return row.clientWidth - used - 8 * (row.children.length - 1)
    })

  /** A surface opens below the entry that owns it and stays inside the viewport's
   *  own width at every fixed viewport. The vertical extent is deliberately not
   *  clamped here: a surface is anchored below its entry and is never re-placed
   *  vertically, so on a phone-height viewport it is taller than the room the
   *  entry leaves under itself and the reader scrolls the page to its tail
   *  (expectReachable reads that half of the contract). */
  const expectAnchoredInside = async (page: Page, entry: Locator, surface: Locator, what: string) => {
    const entryBox = (await entry.boundingBox())!
    const box = (await surface.boundingBox())!
    const viewport = page.viewportSize()!
    expect(box.y, what + ' opens below its entry, not over the row').toBeGreaterThanOrEqual(
      entryBox.y + entryBox.height - 1,
    )
    expect(box.x, what + ' stays inside the left edge').toBeGreaterThanOrEqual(0)
    expect(box.x + box.width, what + ' stays inside the right edge').toBeLessThanOrEqual(viewport.width + 0.5)
  }

  /** The tail of a surface taller than the room under its entry is still reachable:
   *  the reader scrolls the page to it, and there it is inside the viewport and is
   *  still the thing the pointer meets — so a surface that hangs past the fold on
   *  a phone is reachable rather than merely tall, and its last control is not
   *  covered by the list it hangs over. */
  const expectReachable = async (page: Page, control: Locator, surface: Locator, what: string) => {
    // The excursion scrolls the page, and the geometry checks around this helper
    // read the toolbar at rest, so the resting scroll position is restored.
    const resting = await page.evaluate(() => window.scrollY)
    await control.scrollIntoViewIfNeeded()
    const box = (await control.boundingBox())!
    const viewport = page.viewportSize()!
    expect(box.y, what + ' is inside the viewport once the page is scrolled to it').toBeGreaterThanOrEqual(0)
    expect(box.y + box.height, what + ' stays inside the bottom edge once scrolled to').toBeLessThanOrEqual(
      viewport.height + 0.5,
    )
    const reaches = await surface.evaluate(
      (element, point) => {
        const found = document.elementFromPoint(point.x, point.y)
        return found !== null && element.contains(found)
      },
      { x: box.x + box.width / 2, y: box.y + box.height / 2 },
    )
    expect(reaches, 'the pointer still meets the surface at ' + what).toBe(true)
    await page.evaluate((top) => window.scrollTo({ top }), resting)
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(resting)
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

  test('opens each surface below its own entry without leaving the viewport', async ({ page }, testInfo) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })

    // A closed Sort & filter entry holds nothing at all: no select, no surface.
    // The search control is the row's own field where the row has the room for
    // one, and the entry that opens that very field where it does not — one of
    // the two, never both (#241).
    const rowShowsSearchField = await homeToolbarShowsField(page)
    await expect(page.getByRole('combobox')).toHaveCount(0)
    await expect(page.getByRole('dialog')).toHaveCount(0)
    await expect(homeFilterEntry(page)).toBeVisible()
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-expanded', 'false')
    if (rowShowsSearchField) {
      await expect(homeSearchInline(page)).toBeVisible()
      await expect(homeSearchInline(page)).toHaveAttribute('data-search-state', 'off')
      await expect(homeSearchEntry(page)).toBeHidden()
    } else {
      await expect(homeSearchEntry(page)).toBeVisible()
      await expect(homeSearchEntry(page)).toHaveAttribute('aria-expanded', 'false')
      await expect(homeSearchInline(page)).toBeHidden()
    }
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
    // The surface is taller than the room under its entry on a phone-height
    // viewport, and nothing re-places it once it is open: its tail is reached by
    // scrolling the page to it.
    await expectReachable(page, clearFilters(page), filterSurface, 'Clear filters of the open Sort & filter surface')

    // The two search shapes answer the same contract. The row's own field is
    // there to be typed in — no entry to open, no surface, nothing between the
    // reader and the field — while the entry opens that same field below itself
    // and hands it the focus a reader needs while the surface is open.
    await closeHomeSurface(page)
    await expect(homeFilterSurface(page)).toHaveCount(0)
    await expect(homeFilterEntry(page)).toHaveAttribute('aria-expanded', 'false')
    if (rowShowsSearchField) {
      const field = homeSearchInline(page)
      await field.focus()
      await expect(field).toBeFocused()
      await expect(page.getByRole('dialog')).toHaveCount(0)
      await expect(clearSearch(page)).toBeVisible()
      await expect(clearSearch(page)).toBeDisabled()
      expect(await rowGeometry(page), 'focusing the row field must not shift the row').toEqual(before)
      await expectNoHorizontalOverflow(page)
      await expectVisibleInteractiveTargets(page)
    } else {
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
      await expectReachable(
        page,
        clearSearch(page),
        homeSearchSurface(page),
        'Clear search of the open Search surface',
      )

      // Escape closes the surface and hands the keyboard back to its entry, so
      // the row a reader leaves is the row they started from.
      await page.keyboard.press('Escape')
      await expect(homeSearchSurface(page)).toHaveCount(0)
      await expect(homeSearchEntry(page)).toBeFocused()

      if (testInfo.project.use.hasTouch) {
        // The same surface from a finger, where a phone actually lives: opened by
        // a real tap on the entry, dismissed by a real tap outside it, with the
        // word, the entry's marker and the row itself untouched by the round trip.
        await homeSearchEntry(page).tap()
        await expect(homeSearchSurface(page)).toBeVisible()
        await field.fill('Node')
        await expect(page).toHaveURL(/[?&]q=Node/)
        // The tap lands in the row's own gap between the card/list choice and the
        // search control: on no control at all, outside the entry that owns the
        // surface, and at a real finger's coordinates rather than a synthetic
        // event — which is the only way the dismissal a phone uses is read here.
        const viewBox = (await page.locator('[role="tablist"][aria-label="View"]').boundingBox())!
        const slotBox = (await homeSearchSlot(page).boundingBox())!
        await page.touchscreen.tap(
          (viewBox.x + viewBox.width + slotBox.x) / 2,
          viewBox.y + viewBox.height / 2,
        )
        await expect(homeSearchSurface(page)).toHaveCount(0)
        await expect(homeSearchEntry(page)).toHaveAttribute('data-search-state', 'on')
        await expect(page.locator('[data-slot="home-search-active"]')).toBeVisible()
        await expect(page).toHaveURL(/[?&]q=Node/)

        // Re-opened by another tap, the field still holds the reader's word, and
        // the field's own Clear search action is what drops it — leaving the row
        // this test started from.
        await homeSearchEntry(page).tap()
        await expect(page.getByRole('searchbox')).toHaveValue('Node')
        await clearSearch(page).tap()
        await expect(page).not.toHaveURL(/[?&]q=/)
        await closeHomeSurface(page)
        await expect(homeSearchSurface(page)).toHaveCount(0)
      }
    }
    expect(await rowGeometry(page)).toEqual(before)
  })

  /** Issue #241: the search control is the row's own field where the row has the
   *  room for one, and the accessibly named entry that opens that same field where
   *  it does not. One shape at a time, the field wide enough to read, and no other
   *  control of the row pressed for the room it takes. */
  test('gives the row field where the toolbar has room, and the entry that opens it where it does not', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })
    const showsField = await homeToolbarShowsField(page)
    const room = await searchRoom(page)

    // Which shape a row has is a decision about the room the row has left — the
    // Network pills, the Sort & filter entry and the card/list choice at their
    // own widths first — and not about the project's viewport: the row displays
    // its own field exactly while that room holds one, so a narrow desktop with
    // lightly named Networks still keeps the field, and a wide one with a long
    // Network name still keeps the entry.
    expect(
      showsField,
      `the row displays its own field exactly while the room left in it holds one (${Math.round(room)}px)`,
    ).toBe(room >= HOME_SEARCH_TRACK)

    // Both shapes exist; exactly one is displayed, and exactly one field is
    // exposed — never two search controls a reader could reach and neither of
    // them left in the tab walk while it is out of sight.
    expect(await homeSearchInline(page).count(), 'the row field is in the DOM once').toBe(1)
    expect(
      await page.locator('[data-slot="home-search-trigger"]').count(),
      'the entry is in the DOM once',
    ).toBe(1)
    expect(await homeSearchSlot(page).count(), 'the row keeps one slot for that control').toBe(1)
    if (showsField) {
      await expect(homeSearchInline(page)).toBeVisible()
      await expect(homeSearchEntry(page)).toBeHidden()
      await expect(page.getByRole('dialog')).toHaveCount(0)
      expect(await page.getByRole('searchbox').count(), 'exactly one field is exposed').toBe(1)
    } else {
      await expect(homeSearchEntry(page)).toBeVisible()
      await expect(homeSearchInline(page)).toBeHidden()
      expect(await row(page).getByRole('searchbox').count(), 'a closed entry holds no field').toBe(0)
      await expect(await openHomeSearch(page)).toBeVisible()
      expect(await page.getByRole('searchbox').count(), 'exactly one field is exposed').toBe(1)
    }

    // One control either way: what one shape holds, the other reads, and neither
    // loses the word on the way between them.
    const field = await homeSearchField(page)
    await field.fill('Node')
    await expect(page).toHaveURL(/[?&]q=Node/)
    expect(await page.getByRole('searchbox').inputValue()).toBe('Node')

    // The switch is not paid for out of the field's own readability or out of the
    // controls beside it: the field in force keeps the 13rem the design gives it
    // and the theme's own text size, and where it is displayed the Network pills
    // were not the ones that made the room.
    const box = (await page.getByRole('searchbox').boundingBox())!
    expect(box.width, 'the field keeps the readable 13rem the design gives it').toBeGreaterThanOrEqual(13 * 16 - 1)
    const fontSize = await page
      .getByRole('searchbox')
      .evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
    expect(fontSize, 'the field keeps the theme\'s own text size').toBeGreaterThanOrEqual(12)
    if (showsField) {
      expect(
        await row(page).locator('[aria-label="Network filter"]').evaluate((element) => element.scrollWidth > element.clientWidth + 1),
        'the Network pills are not pressed for the room the field takes',
      ).toBe(false)
    }
    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)
  })

  /** Issue #241: the switch is about the room the row has, so the one thing that
   *  changes that room under a reader — a resize — is exercised in both
   *  directions here. A shape change is read at the seams a reader reads: which
   *  shape is displayed, where the keyboard is, and that the word, the filter and
   *  the marker come through it untouched while no surface is left open behind a
   *  shape that is no longer displayed. */
  test('crosses between its own field and the entry without losing the reader', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })
    const narrow = { width: 390, height: 844 }
    const wide = { width: 1280, height: 800 }
    await page.setViewportSize(narrow)
    await expect(homeSearchEntry(page)).toBeVisible()
    await expect(homeSearchInline(page)).toBeHidden()

    // The reader opens the entry and types in the field it opens.
    const field = await homeSearchField(page)
    await field.fill('Node')
    await expect(page).toHaveURL(/[?&]q=Node/)
    await expect(field).toBeFocused()

    // The row grows the room for its own field while the reader is in that
    // surface: the field takes over the reader's place with the word still in it,
    // keeps the described-by sentence naming that word, and the surface is not
    // left open behind a shape that is no longer displayed.
    await page.setViewportSize(wide)
    await expect(homeSearchInline(page)).toBeVisible()
    await expect(homeSearchEntry(page)).toBeHidden()
    await expect(homeSearchInline(page)).toBeFocused()
    await expect(homeSearchInline(page)).toHaveValue('Node')
    await expect(page.locator('[data-slot="home-search-active"]')).toBeHidden()
    await expect(page.locator('#home-search-state')).toHaveText('Search is on: Node')
    await expect(page.locator('[data-slot="toolbar-popover"]')).toHaveCount(0)
    await expect(page).toHaveURL(/[?&]q=Node/)

    // Back where the room is not: the entry speaks for the field that went out of
    // sight, holds the reader's place, and is marked as the narrowed list the
    // reading still is. The word is not touched, and neither is the filter.
    await page.setViewportSize(narrow)
    await expect(homeSearchEntry(page)).toBeVisible()
    await expect(homeSearchInline(page)).toBeHidden()
    await expect(homeSearchEntry(page)).toBeFocused()
    await expect(homeSearchEntry(page)).toHaveAttribute('data-search-state', 'on')
    await expect(page.locator('[data-slot="home-search-active"]')).toBeVisible()
    await expect(page.locator('[data-slot="toolbar-popover"]')).toHaveCount(0)
    await expect(page).toHaveURL(/[?&]q=Node/)
    await expect(await homeSearchField(page)).toHaveValue('Node')
    await expect(page.getByRole('searchbox')).toHaveCount(1)
  })

  /** The sticky header keeps its own pixels over anything Home paints below it. An
   *  open toolbar surface lifts its own wrapper to `md:z-30` (design §15.27), so
   *  the header has to outrank that tier: a row — or the surface hanging off its
   *  entry — that is scrolled into the header band must pass under the brand, the
   *  theme toggle and the Admin link rather than paint over them. */
  test('keeps the sticky header above a row scrolled into it', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })
    const header = page.locator('[data-slot="app-header"]')

    // Open the surface, then scroll the page so that the surface's own top edge
    // sits inside the header band and the row above it crosses the band as well:
    // one position that covers both the entry and the surface it opened.
    const filterSurface = await openHomeFilters(page)
    const surfaceDocTop = await filterSurface.evaluate(
      (element) => element.getBoundingClientRect().top + window.scrollY,
    )
    await page.evaluate((top) => window.scrollTo({ top }), surfaceDocTop - 20)
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(0)

    const headerBox = (await header.boundingBox())!
    const bandBottom = headerBox.y + headerBox.height
    expect((await filterSurface.boundingBox())!.y, 'the open surface reaches into the header band').toBeLessThan(
      bandBottom,
    )
    expect((await row(page).boundingBox())!.y, 'the scrolled row reaches into the header band').toBeLessThan(
      bandBottom,
    )

    // Every pixel of the band still belongs to the header, whatever the scrolled
    // row and its open surface paint behind it.
    const covered = await header.evaluate((element) => {
      const box = element.getBoundingClientRect()
      const hits: string[] = []
      for (let column = 1; column < 10; column += 1) {
        for (let line = 1; line < 10; line += 1) {
          const x = box.left + (box.width * column) / 10
          const y = box.top + (box.height * line) / 10
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
    expect(covered, 'nothing Home paints below the header may cover the header band').toEqual([])
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

  test('keeps the search control marked while a search is in force', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })
    const rowShowsSearchField = await homeToolbarShowsField(page)
    // Nothing is in force yet, in whichever shape this toolbar shows: the state
    // and the described-by sentence are read from the shape in force, and the
    // drawn marker is on show in neither.
    await expect(await searchControl(page)).toHaveAttribute('data-search-state', 'off')
    await expect(await searchControl(page)).not.toHaveAttribute('aria-describedby')
    await expect(page.locator('[data-slot="home-search-active"]')).toBeHidden()

    const field = await homeSearchField(page)
    await field.fill('Node')
    await expect(page).toHaveURL(/[?&]q=Node/)

    // The flag in force is stated on whichever shape the reader has: the state and
    // a sentence naming the text, for a reader who sees neither the field nor a
    // hue — and, where the field sits behind the entry, the drawn marker beside
    // that entry's icon as well as the accent colour.
    await expect(await searchControl(page)).toHaveAttribute('data-search-state', 'on')
    await expect(await searchControl(page)).toHaveAttribute('aria-describedby', 'home-search-state')
    await expect(page.locator('#home-search-state')).toHaveText('Search is on: Node')
    if (rowShowsSearchField) {
      await expect(field).toHaveValue('Node')
      // Escape takes nothing from this reader: there is no surface to close and
      // no entry to hand the keyboard back to, so the word, the filter and the
      // field all stay where they were.
      await page.keyboard.press('Escape')
      await expect(field).toBeFocused()
      await expect(field).toHaveValue('Node')
      await expect(page).toHaveURL(/[?&]q=Node/)
    } else {
      await expect(page.locator('[data-slot="home-search-active"]')).toBeVisible()
      // The marker stays on while the field is out of sight, so a narrowed list is
      // never mistaken for an empty deployment.
      await page.keyboard.press('Escape')
      await expect(homeSearchSurface(page)).toHaveCount(0)
      await expect(homeSearchEntry(page)).toHaveAttribute('data-search-state', 'on')
      await expect(page.locator('[data-slot="home-search-active"]')).toBeVisible()
      await expect(homeFilterEntry(page)).toHaveAttribute('aria-expanded', 'false')
      await expect(await openHomeSearch(page)).toHaveValue('Node')
    }

    // The field's own clear action drops only the query, in both shapes.
    await expect(clearSearch(page)).toBeEnabled()
    await clearSearch(page).click()
    await expect(page.getByRole('searchbox')).toHaveValue('')
    await expect(page).not.toHaveURL(/[?&]q=/)
    await expect(await searchControl(page)).toHaveAttribute('data-search-state', 'off')
    await expect(await searchControl(page)).not.toHaveAttribute('aria-describedby')
    await expect(page.locator('[data-slot="home-search-active"]')).toBeHidden()
  })

  test('leaves the hidden controls out of the tab walk', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })

    const rowShowsSearchField = await homeToolbarShowsField(page)
    await expect(row(page).getByRole('combobox')).toHaveCount(0)
    await expect(row(page).getByRole('textbox')).toHaveCount(0)
    if (rowShowsSearchField) {
      // The row's own field is a stop of the row; the entry that could open the
      // same field is not drawn at all here.
      expect(await row(page).getByRole('searchbox').count(), 'the row field is the row\'s own stop').toBe(1)
      await expect(homeSearchEntry(page)).toBeHidden()
    } else {
      // A field behind a closed entry is in no walk at all.
      await expect(row(page).getByRole('searchbox')).toHaveCount(0)
    }

    // From the selected Network pill, Tab meets the row's own controls in
    // reading order and nothing else: the controls behind an entry are not in
    // the walk until that entry is opened.
    await selectedPill(page).focus()
    await page.keyboard.press('Tab')
    await expect(homeFilterEntry(page)).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(page.getByRole('tab', { name: 'Cards', exact: true })).toBeFocused()
    await page.keyboard.press('Tab')
    if (rowShowsSearchField) {
      // The field is the row's own stop, and with nothing to clear its Clear
      // search action is disabled — which is itself a stop the walk skips — so
      // the reading goes straight on to the card rather than through an entry.
      await expect(homeSearchInline(page)).toBeFocused()
      await expect(clearSearch(page)).toBeDisabled()
      await page.keyboard.press('Tab')
      expect(await page.evaluate(() => document.activeElement?.getAttribute('href') ?? '')).toContain('/nodes/')

      // Once there is a word to clear the action joins the walk beside the field
      // and hands it straight back.
      await homeSearchInline(page).fill('Node')
      await expect(clearSearch(page)).toBeEnabled()
      await page.keyboard.press('Tab')
      await expect(clearSearch(page)).toBeFocused()
      await page.keyboard.press('Shift+Tab')
      await expect(homeSearchInline(page)).toBeFocused()
    } else {
      await expect(homeSearchEntry(page)).toBeFocused()

      // Opening the entry puts the field it holds into the walk between the entry
      // and the rest of the row, and Escape hands the keyboard back to the entry.
      await page.keyboard.press('Enter')
      await expect(homeSearchSurface(page)).toBeVisible()
      await expect(await openHomeSearch(page)).toBeFocused()
      await page.keyboard.press('Escape')
      await expect(homeSearchSurface(page)).toHaveCount(0)
      await expect(homeSearchEntry(page)).toBeFocused()
    }
  })

  test('keeps every surface readable and touch-sized in Light and Dark', async ({ page }) => {
    await loginAs(page)
    await expect(page.locator('[data-slot="node-card"]').first()).toBeVisible({ timeout: 15_000 })
    const rowShowsSearchField = await homeToolbarShowsField(page)

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

      // The search control answers the same contract in both themes, in
      // whichever of its two shapes this toolbar shows.
      if (rowShowsSearchField) {
        const field = homeSearchInline(page)
        await field.focus()
        await expect(field).toBeFocused()
        await expect(field).toBeVisible()
        await expect(clearSearch(page)).toBeVisible()
        await expectVisibleInteractiveTargets(page)
        await expectNoHorizontalOverflow(page)
      } else {
        const field = await openHomeSearch(page)
        await expect(field).toBeFocused()
        await expectElevated(homeSearchSurface(page), 'the Search surface in ' + scheme)
        await expectVisibleInteractiveTargets(page)
        await expectNoHorizontalOverflow(page)
        await closeHomeSurface(page)
        await expect(homeSearchSurface(page)).toHaveCount(0)
      }
    }
  })
})
