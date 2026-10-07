import { expect, test, type Locator, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets, loginAs, openHomeSort } from './helpers'

/**
 * Public Home card and list views (issue #223). The shared Server is seeded by
 * e2e/start-server.sh with the convergence Network, whose six Active Nodes make
 * every interesting list state observable: a full Node (H), an authoritative
 * empty peer set (K), a stale Node (L), a Node with a retained Provider error
 * (N), and a Node that was never observed at all (P). Every assertion here
 * crosses the routed Public Home seam only - accessible roles and names, visible
 * text, the address bar, and externally observable layout geometry.
 */

const CONVERGENCE_NETWORK_KEY = 'home-convergence'

/** The convergence Network's six Active Nodes in Node-identity order. */
const NODE_LETTERS = ['H', 'K', 'L', 'M', 'N', 'P']
/** Their seeded peer counts are 3, 0, 1, 2, 1 and "never observed", so the
 *  descending Peers ranking puts the real zero above the Unknown value. */
const PEERS_ORDER = ['H', 'M', 'L', 'N', 'K', 'P']
/** The list's seven public columns, in order. */
const COLUMN_LABELS = ['Node', 'Network', 'Health', 'Current Head', 'Peers', 'Process CPU', 'Process memory']

/** The delivered default view of one Network, sorted by Node Health. */
const DEFAULT_LIST_URL = `/?network=${CONVERGENCE_NETWORK_KEY}&view=list`
/** The same list under the stable name sort, so its order is Node identity. */
const LIST_URL = `${DEFAULT_LIST_URL}&sort=name`

const nodeGrid = (page: Page) => page.locator('[data-slot="node-grid"]')
/** The list is read through the accessible structure it already publishes. Only
 *  the scroller, which carries no role of its own, is reached by its shell
 *  marker, and only for geometry. */
const listTable = (page: Page) => page.getByRole('table', { name: 'Active Nodes' })
const listScroller = (page: Page) => page.locator('[data-slot="node-list-scroll"]')
const listRows = (page: Page) => listTable(page).getByRole('row').filter({ has: page.getByRole('cell') })
/** The Sort order, read from the surface the toolbar entry holds (issue #232). */
const sortSelect = async (page: Page) => openHomeSort(page)
const viewTab = (page: Page, name: string) => page.getByRole('tab', { name, exact: true })

/** The list's column labels, left to right. */
const columnLabels = (page: Page) => listTable(page).getByRole('columnheader').allInnerTexts()

/** The zero-based index of one column, taken from its own label. */
async function columnIndex(page: Page, column: string): Promise<number> {
  const index = (await columnLabels(page)).indexOf(column)
  expect(index, `the list offers a ${column} column`).toBeGreaterThanOrEqual(0)
  return index
}

/** The listed rows, by the public Node name each row links with. */
async function listedRows(page: Page): Promise<string[]> {
  return listRows(page)
    .locator('a[href^="/nodes/"]')
    .evaluateAll((links) => links.map((link) => (link.getAttribute('aria-label') ?? '').trim()))
}

/** The listed Node letters, e.g. "Node H - ..." reads as "H". */
async function listedLetters(page: Page): Promise<string[]> {
  return (await listedRows(page)).map((name) => /^Node ([A-Z]) /.exec(name)?.[1] ?? name)
}

/** One column, top to bottom, as visible text. */
async function columnTexts(page: Page, column: string): Promise<string[]> {
  const index = await columnIndex(page, column)
  const rows = await listRows(page).all()
  return Promise.all(rows.map((row) => row.getByRole('cell').nth(index).innerText()))
}

/** One Node's cell in one column, as visible text. */
async function nodeCell(page: Page, letter: string, column: string): Promise<string> {
  const index = await columnIndex(page, column)
  return listRows(page)
    .filter({ has: page.locator(`a[aria-label^="Node ${letter} "]`) })
    .getByRole('cell')
    .nth(index)
    .innerText()
}

/** A metric column as attested numbers and never-observed values, in order. */
async function metricColumn(page: Page, column: string): Promise<(number | null)[]> {
  const texts = await columnTexts(page, column)
  // A retained value keeps its number and adds the snapshot qualifier on the
  // line below it, so the value is the cell's first line.
  return texts.map((text) => {
    const value = text.trim().split('\n')[0]!.trim()
    return value === 'Unknown' ? null : Number(value.replace(/[^0-9.]/g, ''))
  })
}

/** Scroll the list's own surface to an absolute offset, and report where it landed. */
async function scrollList(page: Page, offset: number): Promise<number> {
  return listScroller(page).evaluate((element, left) => {
    element.scrollLeft = left
    return element.scrollLeft
  }, offset)
}

/** Wait for the list to settle on exactly "letters", then read the rows again. */
async function expectList(page: Page, letters: string[]): Promise<string[]> {
  await expect(listTable(page)).toBeVisible()
  await expect(listRows(page)).toHaveCount(letters.length)
  const listed = await listedLetters(page)
  expect(listed).toEqual(letters)
  return listed
}

/** Every attested value ranks above every never-observed value, and the
 *  attested values descend: the one rule all four metric sorts share. */
function expectUnknownLast(values: (number | null)[]) {
  const attested = values.filter((value): value is number => value !== null)
  const unknown = values.filter((value) => value === null)
  expect(values.slice(0, attested.length), 'every attested value comes first').toEqual(attested)
  expect(unknown.length + attested.length).toBe(values.length)
  expect([...attested].sort((left, right) => right - left), 'the values descend').toEqual(attested)
}

test.describe('Public Home card and list views (issue #223)', () => {
  test('offers the seven public columns and carries the view in the ordinary Home URL', async ({ page }) => {
    await loginAs(page)
    await expect(nodeGrid(page).locator('a[href^="/nodes/"]').first()).toBeVisible({ timeout: 15_000 })
    expect(page.url(), 'the default Home view is the card grid').not.toContain('view=')

    await viewTab(page, 'List').click()
    await expect(listTable(page)).toBeVisible()
    await expect(nodeGrid(page)).toHaveCount(0)
    await expect.poll(() => page.url()).toContain('view=list')
    expect(await columnLabels(page)).toEqual(COLUMN_LABELS)
    // QC, Locked and Committed are card-only statements, so the list omits them
    // rather than inventing a column the Server never attests.
    expect(await listTable(page).innerText()).not.toContain('Locked')

    // The view is an ordinary Home parameter: a link to it restores the list.
    await page.goto(LIST_URL)
    await expectList(page, NODE_LETTERS)

    // Switching back is an ordinary link too, and it leaves no stale parameter.
    await viewTab(page, 'Cards').click()
    await expect(nodeGrid(page)).toBeVisible()
    await expect(listTable(page)).toHaveCount(0)
    await expect.poll(() => page.url()).not.toContain('view=')

    // A view this deployment does not offer is reported, never silently ignored.
    await page.goto('/?view=grid')
    await expect(nodeGrid(page)).toBeVisible()
    const notice = page.locator('[data-slot="home-filter-notice"]')
    await expect(notice).toContainText('view "grid"')
    await expect(notice).toContainText('fell back to its default')
  })

  test('sorts by all six keys, keeps Unknown last, and never reads a real zero as Unknown', async ({ page }) => {
    await loginAs(page)
    await page.goto(DEFAULT_LIST_URL)
    await expect(listTable(page)).toBeVisible()
    await expect(listRows(page)).toHaveCount(NODE_LETTERS.length)

    expect(await (await sortSelect(page)).locator('option').allInnerTexts()).toEqual([
      'Health',
      'Name',
      'Current Head',
      'Peers',
      'Process CPU',
      'Process memory',
    ])

    // The default Health sort keeps the same economy as the four metric keys: a
    // health the Server never confirmed sorts last, and Node identity keeps one
    // predictable order inside each group.
    const listed = await listedLetters(page)
    expect([...listed].sort(), 'the health sort lists the same six Nodes').toEqual([...NODE_LETTERS].sort())
    const health = await columnTexts(page, 'Health')
    const healthy = listed.filter((_, index) => health[index] === 'Healthy')
    const unconfirmed = listed.filter((_, index) => health[index] !== 'Healthy')
    expect(healthy.length, 'the seam needs a confirmed health').toBeGreaterThan(0)
    expect(unconfirmed.length, 'the seam needs an unconfirmed health').toBeGreaterThan(0)
    expect(health, 'an unconfirmed health reads as Unknown, never as Healthy').toEqual([
      ...healthy.map(() => 'Healthy'),
      ...unconfirmed.map(() => 'Unknown'),
    ])
    expect(healthy, 'the Healthy Nodes keep the stable Node-identity order').toEqual([...healthy].sort())
    expect(unconfirmed, 'the unconfirmed Nodes keep the stable Node-identity order').toEqual([...unconfirmed].sort())

    // The name and Current Head sorts are ordinary public rankings, and the
    // never-observed Node's Head is Unknown, never zero.
    await (await sortSelect(page)).selectOption('name')
    await expect.poll(() => page.url()).toContain('sort=name')
    await expectList(page, NODE_LETTERS)

    await (await sortSelect(page)).selectOption('head')
    await expect.poll(() => page.url()).toContain('sort=head')
    await expectList(page, NODE_LETTERS)
    const heads = await metricColumn(page, 'Current Head')
    expectUnknownLast(heads)
    expect(heads.slice(0, 5), 'the five attested Heads descend').toEqual([12842025, 12842024, 12842023, 12842022, 12842021])
    expect(heads.at(-1), 'the never-observed Node reports Unknown, not zero').toBeNull()
    expect(await nodeCell(page, 'P', 'Current Head')).toBe('Unknown')

    // Peers: the authoritative empty peer set is a real zero that ranks last
    // among the attested values, and only the never-observed Node is Unknown.
    await (await sortSelect(page)).selectOption('peers')
    await expect.poll(() => page.url()).toContain('sort=peers')
    await expectList(page, PEERS_ORDER)
    const peers = await metricColumn(page, 'Peers')
    expectUnknownLast(peers)
    expect(peers, 'a real peer count of zero stays a zero').toEqual([3, 2, 1, 1, 0, null])
    expect(await nodeCell(page, 'K', 'Peers'), 'the authoritative empty peer set is zero, not Unknown').toBe('0')
    expect(await nodeCell(page, 'P', 'Peers'), 'a Node that was never observed is Unknown, not zero').toBe('Unknown')
    // A count that was observed once and then retained keeps its value and says
    // whose snapshot it is, so the list never reads a retained count as current.
    const retained = await nodeCell(page, 'L', 'Peers')
    expect(retained, 'a retained count keeps its value').toContain('1')
    expect(retained, 'a retained count says it is the last good snapshot').toContain('last good')
    expect(retained, 'a retained count names the dimension that is not current').toContain('freshness stale')

    // Both process metrics are Unknown for every Node of this Network, so the
    // stable Node-identity tie-break keeps one predictable order.
    for (const [sort, column] of [['process_cpu', 'Process CPU'], ['process_memory', 'Process memory']] as const) {
      await (await sortSelect(page)).selectOption(sort)
      await expect.poll(() => page.url()).toContain(`sort=${sort}`)
      await expectList(page, NODE_LETTERS)
      expect(await metricColumn(page, column)).toEqual([null, null, null, null, null, null])
    }
  })

  test('keeps the Node name and its health readable while the narrow list scrolls sideways', async ({ page }) => {
    await loginAs(page)
    await page.goto(LIST_URL)
    await expectList(page, NODE_LETTERS)

    // The phone Home is taller than the viewport and the toolbar sits just past
    // the fold, so the toolbar is brought into view before the touch targets are
    // measured: a control the fold cuts is hit tested on the part that happens to
    // be on screen, which is not what a 44px target means.
    await page.getByLabel('Node filters and sorting').scrollIntoViewIfNeeded()

    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)

    const width = await listScroller(page).evaluate((element) => element.clientWidth)
    const content = await listScroller(page).evaluate((element) => element.scrollWidth)
    if (content <= width) {
      // A wide viewport fits all seven columns, so there is nothing to pin.
      expect(content).toBeLessThanOrEqual(width)
      return
    }

    // The surface is only a local scroller to a reader if a finger can pan it: a
    // programmatic scrollLeft would move an overflow-x: hidden box too, so the
    // panning capability itself is asserted rather than assumed.
    const panning = await listScroller(page).evaluate((element) => {
      const style = getComputedStyle(element)
      return { overflowX: style.overflowX, touchAction: style.touchAction }
    })
    expect(['auto', 'scroll'], 'a reader can pan the list surface').toContain(panning.overflowX)
    // touch-action: pan-y would keep the overflow scrollable while blocking the
    // sideways finger pan a phone reader needs, so the axis matters.
    expect(panning.touchAction, 'the surface accepts a horizontal finger pan').not.toMatch(
      /^(none|pan-y)( pinch-zoom)?$/,
    )

    const nameColumn = listTable(page).getByRole('columnheader', { name: 'Node', exact: true })
    const networkColumn = listTable(page).getByRole('columnheader', { name: 'Network', exact: true })
    const firstRow = listRows(page).first()
    const nameCell = firstRow.getByRole('cell').nth(await columnIndex(page, 'Node'))
    const healthCell = firstRow.getByRole('cell').nth(await columnIndex(page, 'Health'))
    const cellX = async (locator: Locator) => (await locator.boundingBox())!.x
    const atRest = {
      name: await cellX(nameColumn),
      network: await cellX(networkColumn),
      rowName: await cellX(nameCell),
      health: await cellX(healthCell),
    }
    // At rest - before any horizontal scrolling - the Node name, its Network and
    // its Health are all inside the visible band of the list surface, so a reader
    // on a phone learns the unit's health without scrolling the list at all.
    const scrollport = (await listScroller(page).boundingBox())!
    const atRestBoxes: [string, { x: number; width: number }][] = [
      ['Node', { x: atRest.name, width: (await nameColumn.boundingBox())!.width }],
      ['Network', { x: atRest.network, width: (await networkColumn.boundingBox())!.width }],
      ['Health', { x: atRest.health, width: (await healthCell.boundingBox())!.width }],
    ]
    for (const [column, box] of atRestBoxes) {
      expect(box.x, `the ${column} column starts inside the list surface`).toBeGreaterThanOrEqual(
        scrollport.x - 1,
      )
      expect(box.x + box.width, `the ${column} column ends inside the list surface`).toBeLessThanOrEqual(
        scrollport.x + scrollport.width + 1,
      )
    }

    // Every column but the Node name travels with the surface. The Health
    // column parks against the name column's right edge, so the pair that must
    // stay readable is readable at the far end without any page-level scroll.
    const parkedAt = await scrollList(page, content - width - 20)
    expect(parkedAt).toBeGreaterThan(0)
    const parked = {
      name: await cellX(nameColumn),
      network: await cellX(networkColumn),
      rowName: await cellX(nameCell),
      health: await cellX(healthCell),
    }
    expect(Math.abs(parked.name - atRest.name), 'the Node name column stays pinned').toBeLessThan(1)
    expect(Math.abs(parked.rowName - atRest.rowName), 'the Node name cell stays pinned').toBeLessThan(1)
    const nameWidth = (await nameColumn.boundingBox())!.width
    expect(
      Math.abs(parked.health - (atRest.name + nameWidth)),
      'the Health column parks beside the pinned Node name',
    ).toBeLessThan(1)
    expect(parked.network, 'the remaining columns slide under the pinned pair').toBeLessThan(atRest.network - 1)

    // Parking is pinning and not coincidence: the rest of the way to the end
    // moves the remaining columns, while the pair does not move at all.
    await scrollList(page, content - width)
    expect(Math.abs((await cellX(nameColumn)) - parked.name)).toBeLessThan(1)
    expect(Math.abs((await cellX(healthCell)) - parked.health)).toBeLessThan(1)
    expect(await cellX(networkColumn)).toBeLessThan(parked.network - 1)

    // Both parked columns sit inside the visible band of the list surface.
    const healthBox = (await healthCell.boundingBox())!
    expect(healthBox.x).toBeGreaterThanOrEqual(scrollport.x - 1)
    expect(healthBox.x + healthBox.width).toBeLessThanOrEqual(scrollport.x + scrollport.width + 1)
    await expect(nameCell.locator('a[href^="/nodes/"]')).toBeVisible()
    await expect(healthCell).toHaveText(/Healthy|Unhealthy|Unknown/)

    // The list scrolls inside its own surface: the page itself never moves.
    expect(await page.evaluate(() => document.scrollingElement?.scrollLeft ?? 0)).toBe(0)
    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)
  })

  test('keeps the pinned columns on the dark surface instead of a light band', async ({ page }) => {
    await loginAs(page)
    await page.emulateMedia({ colorScheme: 'dark' })
    await page.goto(LIST_URL)
    await expect(page.locator('html')).toHaveClass(/dark/)
    await expectList(page, NODE_LETTERS)

    // The name and health columns paint their own background so the columns
    // sliding underneath never show through. That background has to follow the
    // theme: a fixed light surface would leave a bright band over a dark list.
    const surfaces = await listTable(page).evaluate((table) => {
      // The theme states its surfaces in oklch(), so the painted colour is read
      // back through a real canvas instead of string-splitting the computed value.
      const canvas = document.createElement('canvas')
      canvas.width = 1
      canvas.height = 1
      const context = canvas.getContext('2d')!
      const read = (element: Element | null) => {
        const value = getComputedStyle(element!).backgroundColor
        context.clearRect(0, 0, 1, 1)
        context.fillStyle = value
        context.fillRect(0, 0, 1, 1)
        const [r, g, b, alpha] = context.getImageData(0, 0, 1, 1).data
        return { value, r, g, b, alpha: alpha / 255 }
      }
      return {
        header: read(table.querySelector('thead th[data-column="name"]')),
        cell: read(table.querySelector('tbody td')),
      }
    })
    for (const [part, surface] of Object.entries(surfaces)) {
      expect(surface.alpha, `the pinned ${part} background is opaque (${surface.value})`).toBe(1)
      expect(
        Math.max(surface.r, surface.g, surface.b),
        `the pinned ${part} background follows the dark theme (${surface.value})`,
      ).toBeLessThan(90)
    }
  })
})
