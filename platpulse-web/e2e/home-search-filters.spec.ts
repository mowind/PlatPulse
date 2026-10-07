import { expect, test, type Page } from '@playwright/test'
import {
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  homeFilterEntry,
  homeFilterSurface,
  homeSearchEntry,
  homeSearchSurface,
  loginAs,
  openHomeHealth,
  openHomeSearch,
  openHomeSort,
  openHomeValidator,
} from './helpers'

/**
 * Public Home search and filter URLs (issue #222). The shared Server is seeded
 * by e2e/start-server.sh with two public Networks: "PlatON E2E Network" and the
 * convergence Network, whose six Active Nodes carry the display names this spec
 * searches for. Every value searched here is public (Node display name, Node ID,
 * Network display name); the spec is read-only and runs on every fixed viewport
 * project under the repository's single-worker convention.
 *
 * Assertions cross the routed Public Home seam only: accessible roles and names,
 * visible text, the address bar, and externally observable layout geometry. They
 * never reach into component state.
 */

const CONVERGENCE_NETWORK_NAME = 'Home Convergence Network With An Extremely Long Display Name'
const CONVERGENCE_NODE_LETTERS = ['H', 'K', 'L', 'M', 'N', 'P']
const SOLO_NODE_ID = '0195f2a1-0060-4060-8060-000000000060'

const home = (page: Page) => page.getByRole('region', { name: 'Home' })
const summary = (page: Page) => home(page).locator('[aria-label="Home summary"]')
const nodeGrid = (page: Page) => page.locator('[data-slot="node-grid"]')
const resultCount = (page: Page) => page.locator('[data-slot="home-result-count"]')
const filterNotice = (page: Page) => page.locator('[data-slot="home-filter-notice"]')
// Issue #232: the search field and the three select controls live behind their
// own toolbar entries, so every read or change opens the surface that holds the
// control first. Opening is idempotent: an already open surface stays open.
const searchBox = (page: Page) => openHomeSearch(page)
const healthFilter = (page: Page) => openHomeHealth(page)
const validatorFilter = (page: Page) => openHomeValidator(page)
const sortSelect = (page: Page) => openHomeSort(page)
// The entries themselves, and the same controls read from a surface the test
// has already opened (a plain locator, so the test never toggles it shut).
const filterEntry = (page: Page) => homeFilterEntry(page)
const searchField = (page: Page) =>
  homeSearchSurface(page).getByRole('searchbox', { name: 'Search Active Nodes' })
const sortField = (page: Page) =>
  homeFilterSurface(page).getByRole('combobox', { name: 'Sort', exact: true })
const healthField = (page: Page) =>
  homeFilterSurface(page).getByRole('combobox', { name: 'Health filter' })
const validatorField = (page: Page) =>
  homeFilterSurface(page).getByRole('combobox', { name: 'Validator status filter' })
const networkControl = (page: Page) => page.locator('[aria-label="Network filter"]')
/** One control tab by its accessible name. The Network pills and the card/list
 *  View control share the `tab` role, so the name carries the choice. */
const tab = (page: Page, name: string) => page.getByRole('tab', { name, exact: true })

/** The listed Node cards, by the public name each card links with. */
async function listedNodes(page: Page): Promise<string[]> {
  return nodeGrid(page)
    .locator('a[href^="/nodes/"]')
    .evaluateAll((links) => links.map((link) => (link.getAttribute('aria-label') ?? '').trim()))
}

/** Read one Home summary statistic as a number. */
async function summaryValue(page: Page, label: string): Promise<number> {
  const card = summary(page).getByRole('article', { name: label, exact: true })
  const text = await card.locator('[data-slot="summary-value"]').innerText()
  const digits = text.replace(/[^0-9]/g, '')
  expect(digits, `the ${label} statistic must be a number, never Unknown`).not.toBe('')
  return Number(digits)
}

/** Parse the list's own "Showing X of Y Active Nodes" line. */
async function listCounts(page: Page): Promise<{ matching: number; scoped: number }> {
  const text = await resultCount(page).innerText()
  const match = /Showing ([\d,]+) of ([\d,]+) Active Nodes/.exec(text)
  if (!match) throw new Error(`unexpected Home result count: ${text}`)
  return { matching: Number(match[1].replace(/,/g, '')), scoped: Number(match[2].replace(/,/g, '')) }
}

/** Wait for the list to hold exactly `count` Node cards, then read their names.
 *  Every read of the list is gated on the settled card count: a filter change
 *  commits through the URL, so React paints the new list one frame later. */
async function expectListedNodes(page: Page, count: number): Promise<string[]> {
  await expect(nodeGrid(page).locator('a[href^="/nodes/"]')).toHaveCount(count)
  return listedNodes(page)
}

/** Wait for one Home summary statistic to settle on `value`. */
async function expectSummaryValue(page: Page, label: string, value: number) {
  await expect
    .poll(() => summaryValue(page, label), { message: `the ${label} statistic settles on ${value}` })
    .toBe(value)
}

/** The list's in-scope total is the same Network scope the summary counts, and
 *  the grid shows every matching Node, so the three can never look
 *  contradictory. Polls until one consistent reading exists, then returns it. */
async function expectListAgreesWithSummary(page: Page) {
  const reading = async () => {
    const counts = await listCounts(page)
    const activeNodes = await summaryValue(page, 'Active Nodes')
    const cards = await nodeGrid(page).locator('a[href^="/nodes/"]').count()
    return { ...counts, cards, activeNodes, agrees: counts.scoped === activeNodes && counts.matching === cards }
  }
  await expect
    .poll(() => reading().then((settled) => settled.agrees), {
      message: 'the list scope must equal the summary Active Nodes count and list every matching card',
    })
    .toBe(true)
  return reading()
}

/** Wait for the convergence Network's six Active Nodes to be the listed set. */
async function expectConvergenceList(page: Page): Promise<string[]> {
  const names = await expectListedNodes(page, CONVERGENCE_NODE_LETTERS.length)
  for (const letter of CONVERGENCE_NODE_LETTERS) {
    expect(
      names.filter((name) => name.startsWith(`Node ${letter} `)),
      `Node ${letter} is a public Node of the searched Network`,
    ).toHaveLength(1)
  }
  return names
}

test.describe('Public Home search and filter URLs (issue #222)', () => {
  test('narrows the Node list without moving the Network overview', async ({ page }) => {
    await loginAs(page)
    await expect(home(page)).toBeVisible()
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toBeVisible({ timeout: 15_000 })

    const activeNodes = await summaryValue(page, 'Active Nodes')
    const networks = await summaryValue(page, 'Networks')
    const unfiltered = await expectListAgreesWithSummary(page)
    expect(unfiltered.matching, 'the unfiltered list holds the whole Network selection').toBe(activeNodes)
    expect(unfiltered.cards).toBe(activeNodes)
    await expect.poll(async () => (await listedNodes(page)).some((name) => name.includes('Node H'))).toBe(true)

    await (await searchBox(page)).fill('node h')

    await expectListedNodes(page, 1)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toHaveCount(1)
    expect(await listedNodes(page)).toEqual([expect.stringContaining('Node H')])

    // The search owns the list alone: every overview statistic still counts the
    // whole Network selection.
    await expectSummaryValue(page, 'Active Nodes', activeNodes)
    await expectSummaryValue(page, 'Networks', networks)
    const counts = await expectListAgreesWithSummary(page)
    expect(counts.matching).toBe(1)
    expect(counts.scoped).toBe(activeNodes)
    await expect(resultCount(page)).toContainText(`Showing 1 of ${activeNodes} Active Nodes`)
  })

  test('matches the public Node name, Node ID, and Network name', async ({ page }) => {
    await loginAs(page)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toBeVisible({ timeout: 15_000 })
    const activeNodes = await summaryValue(page, 'Active Nodes')

    // A Node ID is a public identifier, so it finds exactly its own Node.
    await (await searchBox(page)).fill(SOLO_NODE_ID)
    await expectListedNodes(page, 1)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toHaveCount(1)

    // A Network name finds that Network's Nodes, and still counts every Network.
    await (await searchBox(page)).fill(CONVERGENCE_NETWORK_NAME)
    await expectConvergenceList(page)
    const counts = await expectListAgreesWithSummary(page)
    expect(counts.matching).toBe(CONVERGENCE_NODE_LETTERS.length)
    expect(counts.scoped).toBe(activeNodes)
    await expectSummaryValue(page, 'Active Nodes', activeNodes)

    // Clearing the search is an ordinary link again: the whole list returns.
    await (await searchBox(page)).fill('')
    await expectListedNodes(page, activeNodes)
    await expectSummaryValue(page, 'Active Nodes', activeNodes)
  })

  test('combines the search with the health filter and explains an empty list', async ({ page }) => {
    await loginAs(page)
    await expect(nodeGrid(page).locator('a[href^="/nodes/"]').first()).toBeVisible({ timeout: 15_000 })
    const activeNodes = await summaryValue(page, 'Active Nodes')

    // Node H is Healthy, so the health filter alone drops it; the Unknown Node
    // states the Server cannot attest are what remains.
    await (await healthFilter(page)).selectOption('unknown')
    const filtered = await expectListAgreesWithSummary(page)
    expect(filtered.matching, 'the Unknown Node states the Server cannot attest remain').toBeGreaterThan(0)
    expect(filtered.scoped).toBe(activeNodes)
    const unknowns = await expectListedNodes(page, filtered.matching)
    expect(unknowns.filter((name) => name.startsWith('Node H '))).toHaveLength(0)

    // Search and health combine with AND, so the two together match nothing,
    // and the list states that plainly instead of showing an empty grid.
    await (await searchBox(page)).fill('Node H')
    await expect(nodeGrid(page)).toHaveCount(0)
    await expect(page.getByText('No Active Nodes match these filters.')).toBeVisible()
    await expect(resultCount(page)).toContainText(`Showing 0 of ${activeNodes} Active Nodes`)
    await expect(page.getByText(/widen the filters/)).toBeVisible()
    // The empty list is a statement about the list, never about the overview.
    await expectSummaryValue(page, 'Active Nodes', activeNodes)
  })

  test('moves the overview with the Network selection while the filters stay in the list', async ({ page }) => {
    await loginAs(page)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toBeVisible({ timeout: 15_000 })
    const activeNodes = await summaryValue(page, 'Active Nodes')

    await (await searchBox(page)).fill('Node H')
    await expectListedNodes(page, 1)
    await expectSummaryValue(page, 'Active Nodes', activeNodes)

    await tab(page, CONVERGENCE_NETWORK_NAME).click()

    // The Network selection is what moves the overview, the map, and the
    // in-scope total; the search keeps narrowing only the list inside it.
    await expect(tab(page, CONVERGENCE_NETWORK_NAME)).toHaveAttribute('aria-selected', 'true')
    const scoped = CONVERGENCE_NODE_LETTERS.length
    expect(scoped).toBeLessThan(activeNodes)
    await expectSummaryValue(page, 'Active Nodes', scoped)
    await expectSummaryValue(page, 'Networks', 1)
    const counts = await expectListAgreesWithSummary(page)
    expect(counts.scoped).toBe(scoped)
    expect(counts.matching).toBe(1)
    expect(await listedNodes(page)).toEqual([expect.stringContaining('Node H')])
    await expect(resultCount(page)).toContainText(CONVERGENCE_NETWORK_NAME)
  })

  test('restores the filters, sorting, and Network selection from an ordinary Home URL', async ({ page }) => {
    await loginAs(page)
    const url = `/?network=home-convergence&q=Node+H&sort=name`
    await page.goto(url)

    await expect(home(page)).toBeVisible()
    await expect(tab(page, CONVERGENCE_NETWORK_NAME)).toHaveAttribute('aria-selected', 'true')
    await expect(await searchBox(page)).toHaveValue('Node H')
    await expect(await sortSelect(page)).toHaveValue('name')
    await expect(nodeGrid(page).locator('a[href^="/nodes/"]')).toHaveCount(1)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toHaveCount(1)
    await expect(filterNotice(page)).toHaveCount(0)
    expect((await expectListAgreesWithSummary(page)).scoped).toBe(CONVERGENCE_NODE_LETTERS.length)

    // An ordinary refresh restores the same view, and so does reaching the
    // address directly, without the page rewriting what it was given.
    await page.reload()
    await expect(await searchBox(page)).toHaveValue('Node H')
    await expect(await sortSelect(page)).toHaveValue('name')
    await expect(nodeGrid(page).locator('a[href^="/nodes/"]')).toHaveCount(1)

    await page.goto(url)
    await expect(await sortSelect(page)).toHaveValue('name')
    await expect(tab(page, CONVERGENCE_NETWORK_NAME)).toHaveAttribute('aria-selected', 'true')
    await expect(page).toHaveURL(/[?]network=home-convergence&q=Node[+]H&sort=name$/)
  })

  test('falls back visibly for the values this deployment cannot honour', async ({ page }) => {
    await loginAs(page)
    await page.goto('/?network=gone&health=critical&sort=size')

    const notice = filterNotice(page)
    await expect(notice).toBeVisible()
    await expect(notice).toContainText('Network "gone"')
    await expect(notice).toContainText('health "critical"')
    await expect(notice).toContainText('sort "size"')
    await expect(notice).toContainText('fell back to their defaults')
    await expect(notice).toContainText('The rest of the link was applied unchanged')

    // The refused values fell back to their defaults in place, the link the
    // reader followed is untouched, and the rest of it still took effect.
    await expect(tab(page, 'All Networks')).toHaveAttribute('aria-selected', 'true')
    await expect(await healthFilter(page)).toHaveValue('all')
    await expect(await sortSelect(page)).toHaveValue('health')
    await expect(page).toHaveURL(/[?]network=gone&health=critical&sort=size$/)
    await expectListAgreesWithSummary(page)

    // Acting on a control is what rewrites the address, without the refused
    // values it could not honour.
    await (await sortSelect(page)).selectOption('name')
    await expect(filterNotice(page)).toHaveCount(0)
    await expect(await sortSelect(page)).toHaveValue('name')
    await expect(page).toHaveURL(/[?]sort=name$/)
  })

  test('keeps the filter controls usable, focus-visible, and 44px at every viewport', async ({ page }) => {
    await loginAs(page)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)

    // The Network filter is a single tab stop: the arrow keys, not Tab, move
    // between the pills, and the pill they select is the ordinary Network filter
    // (so the default selection leaves the address bar bare again).
    const selectedPill = () =>
      page.evaluate(
        () =>
          document
            .querySelector('[aria-label="Network filter"] [role="tab"][aria-selected="true"]')
            ?.textContent?.trim() ?? '',
      )
    const plainHome = page.url()
    const initialPill = await selectedPill()
    expect(initialPill, 'a Network pill must be selected').not.toBe('')
    expect(
      await page.evaluate(() => {
        // The whole Network filter is one tab stop: the control itself holds the
        // stop and its pills stay out of the Tab order (the trigger tabindex is
        // -1 in both states, and the arrow keys move between the pills).
        // The card/list control is a second, independent tab stop; only the
        // Network control and its own pills are counted here.
        const control = document.querySelector('[aria-label="Network filter"]')
        const candidates = [control, ...Array.from(control?.querySelectorAll('[role="tab"]') ?? [])].filter(
          (element): element is HTMLElement => element instanceof HTMLElement,
        )
        return candidates.filter((element) => element.tabIndex >= 0).length
      }),
      'the Network filter must be one tab stop',
    ).toBe(1)
    await networkControl(page).locator('[role="tab"][aria-selected="true"]').focus()
    await page.keyboard.press('ArrowRight')
    // The selected pill follows the address bar, which is painted a frame later,
    // so the new selection is awaited instead of read straight after the key.
    await expect
      .poll(selectedPill, { message: 'the arrow keys must move the Network selection' })
      .not.toBe(initialPill)
    await expect(page).toHaveURL(/[?&]network=/)
    await page.keyboard.press('ArrowLeft')
    await expect.poll(selectedPill).toBe(initialPill)
    expect(page.url(), 'the default Network selection is omitted from the URL').toBe(plainHome)

    // The toolbar row keeps one stop per entry — the Network filter, the Sort &
    // filter entry, the card/list choice, and the search entry — and the
    // controls behind an entry leave the walk with it while its surface is
    // closed (issue #232). The keyboard therefore reaches an entry, opens it,
    // and only then meets the field it holds.
    await networkControl(page).locator('[role="tab"][aria-selected="true"]').focus()
    await page.keyboard.press('Tab')
    await expect(filterEntry(page)).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(tab(page, 'Cards')).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(homeSearchEntry(page)).toBeFocused()

    // Opening the search entry from the keyboard hands the field inside it the
    // focus, and the field paints its own visible focus indicator (the shared
    // field uses a ring rather than an outline).
    await page.keyboard.press('Enter')
    await expect(homeSearchSurface(page)).toBeVisible()
    await expect(searchField(page)).toBeFocused()
    const focus = await page.evaluate(() => {
      const element = document.activeElement
      if (!(element instanceof HTMLElement)) return null
      const style = getComputedStyle(element)
      return {
        focusVisible: element.matches(':focus-visible'),
        outlineWidth: Number.parseFloat(style.outlineWidth),
        ring: style.boxShadow,
      }
    })
    expect(focus, 'the search box must take focus').not.toBeNull()
    expect(focus!.focusVisible, 'a focused filter must match :focus-visible').toBe(true)
    expect(
      focus!.outlineWidth > 0 || focus!.ring !== 'none',
      'a focused filter must show a visible focus indicator',
    ).toBe(true)

    await page.keyboard.type('Node H')
    await expect(nodeGrid(page).locator('a[href^="/nodes/"]')).toHaveCount(1)

    // Tab from the field walks the surface it opened, then the rest of the row in
    // reading order, and hands the keyboard on to the filtered card.
    await page.keyboard.press('Tab')
    await expect(page.getByRole('button', { name: 'Clear search' })).toBeFocused()
    await page.keyboard.press('Shift+Tab')
    await expect(searchField(page)).toBeFocused()

    // Escape closes the surface, takes its field out of the walk with it, and
    // hands the keyboard back to the entry that opened the surface.
    await page.keyboard.press('Escape')
    await expect(homeSearchSurface(page)).toHaveCount(0)
    await expect(searchField(page)).toHaveCount(0)
    await expect(homeSearchEntry(page)).toBeFocused()
    await page.keyboard.press('Tab')
    expect(await page.evaluate(() => document.activeElement?.getAttribute('href') ?? '')).toBe(
      `/nodes/${SOLO_NODE_ID}`,
    )

    // Read backwards, the same row crosses the entries in reverse order, so
    // nothing in the toolbar is a keyboard dead end.
    await page.keyboard.press('Shift+Tab')
    await expect(homeSearchEntry(page)).toBeFocused()
    await page.keyboard.press('Shift+Tab')
    await expect(tab(page, 'Cards')).toBeFocused()
    await page.keyboard.press('Shift+Tab')
    await expect(filterEntry(page)).toBeFocused()
    await page.keyboard.press('Shift+Tab')
    expect(
      await page.evaluate(() => document.activeElement?.closest('[aria-label="Network filter"]') !== null),
      'the Network filter sits before the Sort & filter entry in the reading order',
    ).toBe(true)

    // The Sort & filter entry opens the same way from the keyboard, and the three
    // controls it holds join the walk between the entry and the card/list choice.
    await page.keyboard.press('Tab')
    await expect(filterEntry(page)).toBeFocused()
    await page.keyboard.press('Enter')
    await expect(homeFilterSurface(page)).toBeVisible()
    await page.keyboard.press('Tab')
    await expect(sortField(page)).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(healthField(page)).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(validatorField(page)).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(tab(page, 'Cards')).toBeFocused()
    await page.keyboard.press('Shift+Tab')
    await expect(validatorField(page)).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(homeFilterSurface(page)).toHaveCount(0)
    await expect(filterEntry(page)).toBeFocused()

    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)
  })

  test('restores the search box when the browser goes back while it keeps focus', async ({ page }) => {
    await loginAs(page)
    // An unscoped Home first, so the entries the reader will walk back through
    // are ordinary Home URLs and nothing else.
    const initial = await expectListAgreesWithSummary(page)
    const allNodes = await listedNodes(page)
    const stepsBefore = await page.evaluate(() => history.length)

    await tab(page, CONVERGENCE_NETWORK_NAME).click()
    await expectListAgreesWithSummary(page)
    // One reader action is one history step. A Radix pill reports its value on
    // focus and again on press, so a second write of the same URL would leave a
    // Back step that appears to do nothing.
    expect(
      await page.evaluate(() => history.length),
      'selecting a Network must add one history step',
    ).toBe(stepsBefore + 1)

    await (await searchBox(page)).click()
    await page.keyboard.type('Node H')
    await expectListedNodes(page, 1)
    expect(await (await searchBox(page)).inputValue()).toBe('Node H')

    // Back restores the URL the reader actually reached, so the box must show
    // that text again rather than the entry it was holding.
    await page.goBack()
    expect(
      await (await searchBox(page)).evaluate((element) => element === document.activeElement),
      'the browser must not steal focus from the search box',
    ).toBe(true)
    await expect(await searchBox(page)).toHaveValue('')
    const restored = await expectListAgreesWithSummary(page)
    expect(restored.matching).toBe(initial.matching)
    expect(await listedNodes(page)).toEqual(allNodes)
  })

  test('narrows the list by Validator status and stays operable in Dark', async ({ page }) => {
    await loginAs(page)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toBeVisible({ timeout: 15_000 })
    const activeNodes = await summaryValue(page, 'Active Nodes')

    // Validator status is the Server's own Linked-Validator answer, so the Nodes
    // it links leave the list while the overview stays where it was.
    await (await validatorFilter(page)).selectOption('not_validator')
    await expect(page).toHaveURL(/[?&]validator=not_validator$/)
    const notValidator = await expectListAgreesWithSummary(page)
    const notValidatorNames = await listedNodes(page)
    expect(notValidator.matching, 'the Validator status filter narrows the list').toBeLessThan(activeNodes)
    expect(notValidator.scoped, 'the Validator status filter does not change the scope').toBe(activeNodes)
    await expectSummaryValue(page, 'Active Nodes', activeNodes)
    await expect(filterNotice(page)).toHaveCount(0)

    // Unknown is its own answer, never folded into Not a Validator, and the two
    // cannot overlap because each Node carries exactly one status.
    await (await validatorFilter(page)).selectOption('unknown')
    const unknown = await expectListAgreesWithSummary(page)
    const unknownNames = await listedNodes(page)
    expect(unknown.matching, 'an Unlinked Validator status is listed as Unknown').toBeGreaterThan(0)
    expect(unknown.scoped).toBe(activeNodes)
    expect(unknownNames.filter((name) => notValidatorNames.includes(name))).toEqual([])

    // The three statuses partition the Active Node set: nothing is left outside
    // the vocabulary the Server publishes.
    await (await validatorFilter(page)).selectOption('validator')
    const validators = await expectListAgreesWithSummary(page)
    expect(validators.matching + unknown.matching + notValidator.matching).toBe(activeNodes)
    const validatorNames = await expectListedNodes(page, validators.matching)
    expect(
      validatorNames.filter((name) => unknownNames.includes(name) || notValidatorNames.includes(name)),
      'a Node carries exactly one Validator status',
    ).toEqual([])

    // Dark is the same contract read a second time: the control keeps its name,
    // its value, its 44px target, its focus indicator, and the list it narrowed.
    await page.emulateMedia({ colorScheme: 'dark' })
    await expect
      .poll(() => page.evaluate(() => document.documentElement.classList.contains('dark')))
      .toBe(true)
    await expect(await validatorFilter(page)).toHaveValue('validator')
    const dark = await expectListAgreesWithSummary(page)
    expect(dark.matching, 'Dark does not change the filtered list').toBe(validators.matching)
    expect(dark.scoped).toBe(activeNodes)
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
    await (await validatorFilter(page)).focus()
    expect(
      await (await validatorFilter(page)).evaluate((element) => element.matches(':focus-visible')),
      'a focused Validator status filter must match :focus-visible in Dark',
    ).toBe(true)
  })

  test('scrolls the Network pills inside their own control and selects one by touch', async ({ page }) => {
    await loginAs(page)
    await expect(nodeGrid(page).locator(`a[href="/nodes/${SOLO_NODE_ID}"]`)).toBeVisible({ timeout: 15_000 })
    const activeNodes = await summaryValue(page, 'Active Nodes')
    const plainHome = page.url()
    // The pill row is the only scroller the toolbar adds: the pills move inside
    // it while the page itself never pans (design §9, §11.1).
    const pillRow = page.locator('div.overflow-x-auto').filter({ has: page.locator('[aria-label="Network filter"]') })

    if ((await pillRow.evaluate((row) => row.scrollWidth - row.clientWidth)) > 0) {
      await pillRow.evaluate((row) => {
        row.scrollLeft = row.scrollWidth
      })
      await expect
        .poll(() => pillRow.evaluate((row) => row.scrollLeft >= row.scrollWidth - row.clientWidth - 1))
        .toBe(true)
      expect(
        await page.evaluate(() => document.scrollingElement?.scrollLeft ?? 0),
        'a local pill scroll must not pan the page',
      ).toBe(0)
      await pillRow.evaluate((row) => {
        row.scrollLeft = 0
      })
      await expect.poll(() => pillRow.evaluate((row) => row.scrollLeft)).toBe(0)
    }

    // Where the project carries a touch screen, selecting a Network is a real
    // tap on the pill — and the tap point stays inside the visible part of the
    // row, because a long Network name can be wider than the scroller.
    if (await page.evaluate(() => navigator.maxTouchPoints > 0)) {
      const tapPill = async (name: string) => {
        // Committing a Network selection grows the overview band above the
        // toolbar, which can push the row past the bottom of the 360x800
        // phone: a real touch dispatched outside the viewport is dropped
        // before it reaches the page. Bring the row into the viewport first,
        // the way a reader scrolls to it. The row itself is never scrolled
        // here, so the pill still has to be reachable in the part of the row
        // its own scroller shows.
        await pillRow.evaluate((row) => row.scrollIntoView({ block: 'nearest', inline: 'nearest' }))
        const point = await tab(page, name).evaluate((pill) => {
          const rect = pill.getBoundingClientRect()
          const row = pill.closest('div.overflow-x-auto')?.getBoundingClientRect()
          if (!row) return null
          const left = Math.max(rect.left, row.left)
          const right = Math.min(rect.right, row.right)
          if (right - left < 8) return null
          return { x: (left + right) / 2, y: rect.top + rect.height / 2, viewportHeight: window.innerHeight }
        })
        expect(point, `the ${name} pill must be reachable inside the pill row`).not.toBeNull()
        expect(
          point!.y,
          `the ${name} tap point must stay inside the viewport: a touch outside it never reaches the pill`,
        ).toBeLessThan(point!.viewportHeight)
        await page.touchscreen.tap(point!.x, point!.y)
      }

      const secondPill = await networkControl(page)
        .locator('[role="tab"][aria-selected="false"]')
        .first()
        .textContent()
      await tapPill(secondPill!.trim())

      // The tap is the Network selection: the pills, the URL, the overview and
      // the Node list all follow it.
      await expect(tab(page, secondPill!.trim())).toHaveAttribute('aria-selected', 'true')
      await expect(page).toHaveURL(/[?&]network=/)
      await expect.poll(() => summaryValue(page, 'Active Nodes')).not.toBe(activeNodes)

      await tapPill('All Networks')
      await expect(tab(page, 'All Networks')).toHaveAttribute('aria-selected', 'true')
      await expectSummaryValue(page, 'Active Nodes', activeNodes)
      expect(page.url(), 'the default Network selection is omitted from the URL again').toBe(plainHome)
    }
  })
})
