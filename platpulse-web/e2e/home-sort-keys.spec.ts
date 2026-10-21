import { expect, test, type Page } from '@playwright/test'
import type { PublicNetwork } from '../src/api/generated'
import {
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
  homeFilterEntry,
  homeFilterSurface,
  loginAs,
  openHomeSort,
} from './helpers'

/**
 * Home Disk, Uptime and Attention First stable sorts (issue #242). The three new
 * keys join the one Sort control the toolbar already holds (issue #232), so every
 * assertion here reads and changes them through that visible entry and the
 * ordinary Home URL, at all five fixed viewports and in both themes.
 *
 * Two fixtures carry the contract. The first is a projection this spec serves
 * itself, whose four Nodes rank differently under Disk, Uptime and Attention
 * First and whose Host storage percentage never agrees with the Node data ratio it
 * would otherwise be confused with; the second is the shared Server's convergence
 * Network, whose real readings prove a #242 key survives a direct URL, a refresh
 * and a Node Detail return.
 */

/** The seven public list columns, unchanged by the three new sort keys: a new
 *  sort key is not a new column. */
const COLUMN_LABELS = ['Node', 'Network', 'Health', 'Current Head', 'Peers', 'Process CPU', 'Process memory']
/** The nine sort keys, in the one Sort control's own order. */
const SORT_LABELS = [
  'Health',
  'Name',
  'Current Head',
  'Peers',
  'Process CPU',
  'Process memory',
  'Disk',
  'Uptime',
  'Attention First',
]

const CONVERGENCE_NETWORK_KEY = 'home-convergence'
/** The convergence Network's six Active Nodes, in Node-identity order. */
const CONVERGENCE_LETTERS = ['H', 'K', 'L', 'M', 'N', 'P']

/** The card grid, the one Node link inside every card, and the Compact list's
 *  own table. The slot markers are contract, not internals: webui.md §15.25 publishes
 *  `data-slot="node-link"` and webui.md §15.24's card/list acceptance already scopes by
 *  the grid it names, so the order below is read the way the delivered Home specs
 *  read it, while every value and every accessible name is read through its role. */
const nodeGrid = (page: Page) => page.locator('[data-slot="node-grid"]')
const nodeLink = (scope: ReturnType<typeof nodeGrid>) => scope.locator('a[data-slot="node-link"]')
const listTable = (page: Page) => page.getByRole('table', { name: 'Active Nodes' })
const listRows = (page: Page) => listTable(page).getByRole('row').filter({ has: page.getByRole('cell') })
const sortSelect = (page: Page) => openHomeSort(page)
const viewTab = (page: Page, name: string) => page.getByRole('tab', { name, exact: true })
const backToHome = (page: Page) => page.getByRole('link', { name: 'Back to Home', exact: true })

/** Home's cards, top to bottom, by the public name each card links with. */
const cardNames = (page: Page) =>
  nodeLink(nodeGrid(page)).evaluateAll((links) => links.map((link) => (link.getAttribute('aria-label') ?? '').trim()))

/** The Compact list's rows, top to bottom, by the same public names. */
const listedNames = (page: Page) =>
  listRows(page)
    .locator('a[data-slot="node-link"]')
    .evaluateAll((links) => links.map((link) => (link.getAttribute('aria-label') ?? '').trim()))

/** The listed Nodes' letters, e.g. "Node H — …" reads as "H". */
const listedLetters = async (page: Page): Promise<string[]> =>
  (await listedNames(page)).map((name) => /^Node ([A-Z]) /.exec(name)?.[1] ?? name)

/** One public column of the Compact list, top to bottom, as visible text. */
async function columnTexts(page: Page, column: string): Promise<string[]> {
  const index = (await listTable(page).getByRole('columnheader').allInnerTexts()).indexOf(column)
  expect(index, `the list offers a ${column} column`).toBeGreaterThanOrEqual(0)
  const rows = await listRows(page).all()
  return Promise.all(rows.map((row) => row.getByRole('cell').nth(index).innerText()))
}

/** One Node's card, and the resource region inside it, read through the roles
 *  and the accessible name the card publishes rather than through a selector. */
const cardOf = (page: Page, name: string) =>
  page.getByRole('article').filter({ has: page.locator(`a[data-slot="node-link"][aria-label="${name}"]`) })
/** What that card's resource region reads: one line for CPU, Memory and Disk. */
const resourcesOf = (page: Page, name: string) => cardOf(page, name).getByLabel('Node process and host network resources')

/** Wait for the card grid to hold exactly "names", in that order. */
async function expectCards(page: Page, names: string[]): Promise<string[]> {
  await expect.poll(() => cardNames(page), { message: `the card grid settles on ${names.join(', ')}` }).toEqual(names)
  return cardNames(page)
}

/** Wait for the Compact list to hold exactly "names", in that order. */
async function expectListed(page: Page, names: string[]): Promise<string[]> {
  await expect(listTable(page)).toBeVisible()
  await expect.poll(() => listedNames(page), { message: `the list settles on ${names.join(', ')}` }).toEqual(names)
  return listedNames(page)
}

/** Wait for the Compact list to hold exactly "letters", in that order. */
async function expectLetters(page: Page, letters: string[]): Promise<string[]> {
  await expect.poll(() => listedLetters(page), { message: `the list settles on ${letters.join(', ')}` }).toEqual(letters)
  return listedLetters(page)
}

/** The projection this spec serves: four Active Nodes whose Disk ratio, process
 *  uptime and Server health each rank them differently, and whose Host storage
 *  percentage never agrees with the Node data ratio drawn beside it. */
async function serveFourNodeProjection(page: Page) {
  await page.route('**/api/public/v1/networks*', async (route) => {
    const response = await route.fetch()
    const [network]: PublicNetwork[] = await response.json()
    const base = network.nodes[0]
    const node = (overrides: Partial<typeof base>) => ({
      ...base,
      validator: null,
      currentHead: 12_842_000,
      networkReferenceHead: null,
      networkReferenceConfidence: 'unknown',
      peers: { ...base.peers, state: 'ok', freshness: 'current', peerCount: 1 },
      ...overrides,
    })
    await route.fulfill({
      response,
      json: [
        {
          ...network,
          nodes: [
            // 90% of its Node data directory's filesystem, on a Host the Server
            // reports as 10% full.
            node({
              nodeId: 'sort-full',
              displayName: 'Directory Full',
              health: 'healthy',
              healthReason: 'RPC reachable',
              nodeDataDirectorySizeBytes: 900_000_000_000,
              nodeDataDirectoryCapacityBytes: 1_000_000_000_000,
              hostStoragePercent: 10,
              processUptimeMs: 1_000,
            }),
            // Half the directory, ten minutes of process uptime.
            node({
              nodeId: 'sort-half',
              displayName: 'Directory Half',
              health: 'healthy',
              healthReason: 'RPC reachable',
              nodeDataDirectorySizeBytes: 500,
              nodeDataDirectoryCapacityBytes: 1_000,
              hostStoragePercent: 90,
              processUptimeMs: 600_000,
            }),
            // An authoritative empty directory — a real zero, not a missing
            // reading — on the longest-lived process.
            node({
              nodeId: 'sort-zero',
              displayName: 'Directory Zero',
              health: 'unknown',
              healthReason: 'Never observed',
              nodeDataDirectorySizeBytes: 0,
              nodeDataDirectoryCapacityBytes: 1_000,
              hostStoragePercent: 95,
              processUptimeMs: 3_600_000,
            }),
            // Nothing observed at all: no ratio and no uptime, and the one Node
            // the Server reports as Unhealthy, so Attention First leads with it.
            node({
              nodeId: 'sort-never',
              displayName: 'Never Observed',
              health: 'unhealthy',
              healthReason: 'Process stopped',
              nodeDataDirectorySizeBytes: null,
              nodeDataDirectoryCapacityBytes: null,
              hostStoragePercent: null,
              processUptimeMs: null,
            }),
          ],
        },
      ],
    })
  })
}

test.describe('Public Home Disk, Uptime and Attention First sorts (issue #242)', () => {
  test('ranks the same Nodes three more ways through the one Sort control', async ({ page }, testInfo) => {
    await loginAs(page)
    await serveFourNodeProjection(page)

    // What the projection above must rank as, under each key: one Node data ratio
    // and one process uptime per Node, and the Server's own health word for each.
    const NAME = ['Directory Full', 'Directory Half', 'Directory Zero', 'Never Observed']
    const DISK = ['Directory Full', 'Directory Half', 'Directory Zero', 'Never Observed']
    const UPTIME = ['Directory Zero', 'Directory Half', 'Directory Full', 'Never Observed']
    const ATTENTION = ['Never Observed', 'Directory Zero', 'Directory Full', 'Directory Half']
    const HEALTH = ['Never Observed', 'Directory Full', 'Directory Half', 'Directory Zero']

    // Disk: the largest Node data ratio leads, a real zero stays a value, and the
    // ratio the Server never attested sorts last.
    await page.goto('/?sort=disk')
    await expectCards(page, DISK)
    await expect(resourcesOf(page, 'Directory Full')).toContainText('90.0%')
    await expect(resourcesOf(page, 'Directory Half')).toContainText('50.0%')
    await expect(resourcesOf(page, 'Directory Zero')).toContainText('0.0%')
    await expect(resourcesOf(page, 'Never Observed')).toContainText('Unknown')
    // The Disk reading is the Node data directory's own ratio: this Node's Host
    // storage percentage is a different number (10%) and never appears here.
    await expect(cardOf(page, 'Directory Full')).not.toContainText('10.0%')

    // Uptime: a different ranking over the same four Nodes — the long-lived
    // process leads, a restart is a real zero, and an unobserved process is last.
    await (await sortSelect(page)).selectOption('uptime')
    await expect.poll(() => page.url()).toContain('sort=uptime')
    await expectCards(page, UPTIME)

    // Attention First: Unhealthy, then Unknown, then Healthy — its own order over
    // the Server's health value, and the reverse grouping of the delivered Health
    // key beside it.
    await (await sortSelect(page)).selectOption('attention')
    await expect.poll(() => page.url()).toContain('sort=attention')
    await expectCards(page, ATTENTION)
    await (await sortSelect(page)).selectOption('health')
    await expectCards(page, HEALTH)

    // Both views read the same matching collection and the same order, so every
    // key is read again in the Compact list — which the three new keys add no
    // column to, and which keeps the seven public columns it already had.
    await viewTab(page, 'List').click()
    for (const [key, order] of [['disk', DISK], ['uptime', UPTIME], ['attention', ATTENTION], ['health', HEALTH]] as const) {
      await (await sortSelect(page)).selectOption(key)
      // The delivered default key is omitted from the address bar, so the control
      // itself is what says the list is sorted by it.
      await expect(await sortSelect(page), `the list is sorted by ${key}`).toHaveValue(key)
      await expectListed(page, order)
      await expect.poll(() => page.url(), { message: `the list view survives ${key}` }).toContain('view=list')
    }
    expect(await listTable(page).getByRole('columnheader').allInnerTexts()).toEqual(COLUMN_LABELS)
    await viewTab(page, 'Cards').click()
    await expectCards(page, HEALTH)

    // The new keys compose with the search and the status filters instead of
    // resetting them, and the narrowed list keeps the same ranking inside itself.
    await page.goto('/?q=Directory&health=healthy&sort=disk')
    await expectCards(page, DISK.slice(0, 2))
    await expect(await sortSelect(page)).toHaveValue('disk')
    await page.goto('/?validator=unknown&sort=uptime')
    await expectCards(page, UPTIME)
    await expect(await sortSelect(page)).toHaveValue('uptime')

    // The one Sort control offers the nine keys in their documented order, and the
    // surface invents no second direction control beside it.
    const select = await sortSelect(page)
    expect(await select.locator('option').allInnerTexts()).toEqual(SORT_LABELS)
    await expect(page.getByRole('dialog', { name: 'Sort and filters' }).getByRole('combobox')).toHaveCount(3)

    // A key this deployment never published still falls back visibly.
    await page.goto('/?sort=disk_percent')
    await expect(page.locator('[data-slot="home-filter-notice"]')).toContainText('sort "disk_percent"')
    await expect(await sortSelect(page)).toHaveValue('health')
    await expectCards(page, HEALTH)

    // Keyboard and touch reach what a pointer reaches: the entry is one stop with
    // a visible focus ring, the arrow keys move the selection inside it, and on the
    // touch projects the same 44px target opens the surface with a tap.
    const entry = await sortSelect(page)
    await entry.focus()
    expect(
      await entry.evaluate((element) => element.matches(':focus-visible')),
      'a focused Sort control must match :focus-visible in Light',
    ).toBe(true)
    await page.keyboard.press('ArrowDown')
    await expect(entry, 'the arrow keys must move the Sort selection').toHaveValue('name')
    await expectCards(page, NAME)
    await entry.selectOption('health')
    await expectCards(page, HEALTH)
    await page.keyboard.press('Escape')
    if (testInfo.project.use.hasTouch) {
      await homeFilterEntry(page).tap()
      await expect(homeFilterSurface(page)).toBeVisible()
      await page.keyboard.press('Escape')
      await expect(homeFilterSurface(page)).toBeHidden()
    }

    // Dark is the same contract read a second time: every key keeps its ranking,
    // and the entry keeps its value, its visible focus ring and its 44px target.
    await page.emulateMedia({ colorScheme: 'dark' })
    await expect.poll(() => page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(true)
    for (const [key, order] of [['disk', DISK], ['uptime', UPTIME], ['attention', ATTENTION]] as const) {
      await (await sortSelect(page)).selectOption(key)
      await expectCards(page, order)
    }
    await (await sortSelect(page)).focus()
    expect(
      await (await sortSelect(page)).evaluate((element) => element.matches(':focus-visible')),
      'a focused Sort control must match :focus-visible in Dark',
    ).toBe(true)
    await page.keyboard.press('Escape')
    await expectVisibleInteractiveTargets(page)
    await expectNoHorizontalOverflow(page)
  })

  test('keeps a #242 sort key through a direct URL, a refresh and a Node Detail return', async ({ page }) => {
    await loginAs(page)
    // The fixture this reading is measured against: a Name sort fixes the six
    // Nodes in Node-identity order (their letters follow their Node IDs), and the
    // Health column carries the Server's own word for each. Both rankings below
    // are computed from that classification instead of being restated from one of
    // the readings under test.
    await page.goto(`/?network=${CONVERGENCE_NETWORK_KEY}&view=list&sort=name`)
    await expect(listRows(page)).toHaveCount(CONVERGENCE_LETTERS.length)
    const letters = await listedLetters(page)
    const health = await columnTexts(page, 'Health')
    const healthOf = (word: string) => letters.filter((_, index) => health[index] === word)
    const unhealthy = healthOf('Unhealthy')
    const unknown = healthOf('Unknown')
    const healthy = healthOf('Healthy')
    expect(
      [...unhealthy, ...unknown, ...healthy].sort(),
      'every Node of the reading carries one of the three health words',
    ).toEqual([...CONVERGENCE_LETTERS].sort())
    expect(unknown.length, 'the seam needs a health the Server has not confirmed').toBeGreaterThan(0)
    expect(healthy.length, 'the seam needs a confirmed Healthy Node').toBeGreaterThan(0)

    // The delivered Health key ranks the set Unhealthy → Healthy → Unknown, and
    // Attention First ranks the same set Unhealthy → Unknown → Healthy: two
    // readings of one Server value, neither rewriting the other.
    await page.goto(`/?network=${CONVERGENCE_NETWORK_KEY}&view=list&sort=health`)
    const healthOrder = await expectLetters(page, [...unhealthy, ...healthy, ...unknown])

    const url = `/?network=${CONVERGENCE_NETWORK_KEY}&view=list&sort=attention`
    await page.goto(url)
    await expect(await sortSelect(page)).toHaveValue('attention')
    const attentionOrder = await expectLetters(page, [...unhealthy, ...unknown, ...healthy])
    expect(attentionOrder, 'the two keys rank this reading differently').not.toEqual(healthOrder)
    const attentionNames = await listedNames(page)

    // A direct URL and an ordinary refresh both restore the #242 key, and the page
    // never rewrites the link it was given.
    await page.goto(url)
    await expect(await sortSelect(page)).toHaveValue('attention')
    await expectLetters(page, attentionOrder)
    await page.reload()
    await expect(await sortSelect(page)).toHaveValue('attention')
    await expectLetters(page, attentionOrder)
    await expect(page).toHaveURL(/[?]network=home-convergence&view=list&sort=attention$/)

    // Back and Forward walk a #242 key exactly like a delivered one: changing the
    // Sort control is its own history entry, and stepping through that history
    // restores the ranking each entry was read in.
    await (await sortSelect(page)).selectOption('disk')
    await expect.poll(() => page.url()).toContain('sort=disk')
    await page.goBack()
    await expect(await sortSelect(page)).toHaveValue('attention')
    await expectLetters(page, attentionOrder)
    await page.goForward()
    await expect(await sortSelect(page)).toHaveValue('disk')
    await page.goto(url)
    await expectLetters(page, attentionOrder)

    // Back through Node Detail: the reading the reader left comes back with the
    // sort key it was left in, at the Node they opened.
    const departed = (await listedNames(page))[3]!
    await listRows(page)
      .filter({ has: page.locator(`a[aria-label="${departed}"]`) })
      .locator('a[data-slot="node-link"]')
      .click()
    await expect(page.getByText('Node detail')).toBeVisible()
    await backToHome(page).click()
    await expect(page).toHaveURL(/[?]network=home-convergence&sort=attention&view=list$/)
    await expectListed(page, attentionNames)
    // Read before the Sort entry is opened: that entry moves the keyboard into
    // its own surface, and the reader's place is the row, not the toolbar.
    await expect(page.getByRole('link', { name: departed })).toBeFocused()
    await expect(await sortSelect(page)).toHaveValue('attention')
    await expectNoHorizontalOverflow(page)
  })
})
