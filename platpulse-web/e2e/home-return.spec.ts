import { expect, test, type Locator, type Page } from '@playwright/test'
import { readFileSync } from 'node:fs'

import { loginToDisposableServer } from './admin-flow'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets, loginAs } from './helpers'
import { startDisposableServer } from './server-harness'

/**
 * Same-tab return from Node Detail to Home (issue #224). The convergence
 * Network's six Active Nodes make the whole journey observable: the reader
 * leaves a reading they can see and comes back to the same Node in that same
 * reading, even though the fresh projection has since ranked it differently or
 * dropped it altogether. Every reorder, rename, and disappearance here is a real
 * Admin request against the running Server, so what the reader meets on the way
 * back is the Public surface a second reader would see, never a rendering of
 * private state this spec wrote by hand. Each write is undone before its test
 * ends, because every project in this run shares one temp SQLite Server.
 */

const CONVERGENCE_NETWORK_KEY = 'home-convergence'
/** The reading the reader leaves from: the Network's Active Nodes ranked by
 *  Peers, so the returned Node has to be found below five others. */
const DEPARTURE_LIST_URL = `/?network=${CONVERGENCE_NETWORK_KEY}&view=list&sort=peers`
/** The same reading as Node Detail rebuilds it, in Home's own parameter order. */
const RETURNED_LIST_SEARCH = `?network=${CONVERGENCE_NETWORK_KEY}&sort=peers&view=list`
/** The same reading in the default card view. */
const DEPARTURE_CARD_URL = `/?network=${CONVERGENCE_NETWORK_KEY}&sort=peers`
const RETURNED_CARD_SEARCH = `?network=${CONVERGENCE_NETWORK_KEY}&sort=peers`
/** The same Active Nodes by name, where the six stand in alphabetical order. */
const DEPARTURE_NAME_URL = `/?network=${CONVERGENCE_NETWORK_KEY}&view=list&sort=name`
const RETURNED_NAME_SEARCH = `?network=${CONVERGENCE_NETWORK_KEY}&sort=name&view=list`
/** The reader's own search: the reading a word of two names narrows to those two. */
const STALE_DEPARTURE_URL = `/?network=${CONVERGENCE_NETWORK_KEY}&q=Stale&view=list&sort=peers`
const RETURNED_STALE_SEARCH = `?network=${CONVERGENCE_NETWORK_KEY}&q=Stale&sort=peers&view=list`
/** A Network this deployment does not serve, and a word only two Node names
 *  carry: the reading widens to every Node this deployment does serve, Home says
 *  so on the way in, and the reader's own search answers exactly two of them. */
const REFUSED_STALE_URL = '/?network=gone&q=Stale&view=list&sort=peers'
const RETURNED_REFUSED_STALE_SEARCH = '?network=gone&q=Stale&sort=peers&view=list'
/** The Node the reader opens in most of these journeys: fourth of six by Peers,
 *  so a place of 1 names another Node and only the ID can be right. */
const DEPARTED_LETTER = 'N'
const NODE_K_ID = '0195f2a1-0061-4061-8061-000000000061'
const NODE_L_ID = '0195f2a1-0062-4062-8062-000000000062'
const NODE_K_NAME = 'Node K — Missing Current Head Block Summary'
const NODE_L_NAME = 'Node L — Stale Consensus Membership'
/** A name that sorts after every other Node of this Network, so the Node holding
 *  it moves to the end of the Name reading while the reader is away. */
const RENAMED_K_NAME = 'Node Zeta — Renamed While The Reader Was Away'
/** A name that no longer carries the word the reader's own search is looking for. */
const RENAMED_L_NAME = 'Node L — Consensus Membership'

const listTable = (page: Page) => page.getByRole('table', { name: 'Active Nodes' })
const listRows = (page: Page) => listTable(page).getByRole('row').filter({ has: page.getByRole('cell') })
const nodeGrid = (page: Page) => page.locator('[data-slot="node-grid"]')
const nodeLink = (scope: Locator) => scope.locator('a[data-slot="node-link"]')
/** One Node's own row, and one Node's own card, addressed by the name its link
 *  already announces. */
const rowsOf = (page: Page, letter: string) =>
  listRows(page).filter({ has: page.locator(`a[aria-label^="Node ${letter} "]`) })
const cardsOf = (page: Page, letter: string) =>
  nodeGrid(page).locator('[data-slot="node-card-frame"]').filter({ has: page.locator(`a[aria-label^="Node ${letter} "]`) })
const returnNotice = (page: Page) => page.locator('[data-slot="home-return-notice"]')
const filterNotice = (page: Page) => page.locator('[data-slot="home-filter-notice"]')
const backToHome = (page: Page) => page.getByRole('link', { name: 'Back to Home', exact: true })

/** The address bar's own search, which is where a reading may live and a
 *  position may never live. */
const returnedSearch = (page: Page) => new URL(page.url()).search

/** The zero-based place a Node's own name holds in the reading on screen, which
 *  is the place that Node's link records as it is opened. */
const rowIndexOf = (page: Page, name: string) =>
  listRows(page).evaluateAll((rows: Element[], wanted: string) => rows.findIndex((row) => (row.textContent ?? '').includes(wanted)), name)

/** The Public projection Home itself is built from, as the Server answers it: a
 *  change is waited for here, so the journey back starts from a deployment that
 *  has really moved rather than from whenever a subscription happens to arrive. */
const publicProjection = (page: Page) =>
  page.evaluate(async () => {
    const response = await fetch('/api/public/v1/networks')
    return await response.text()
  })

/** One Admin write against the running Server, sent from inside the page so the
 *  browser's own same-origin `Origin` header accompanies the CSRF token the
 *  Server's mutation guard requires beside it (design §13.3). */
async function adminWrite(page: Page, method: 'PUT' | 'POST', path: string, body: unknown) {
  const result = await page.evaluate(
    async (request: { method: 'PUT' | 'POST'; path: string; body: unknown }) => {
      const session = await fetch('/api/public/v1/session')
      const { csrfToken } = (await session.json()) as { csrfToken: string }
      const response = await fetch(request.path, {
        method: request.method,
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrfToken },
        body: JSON.stringify(request.body),
      })
      return { status: response.status, body: await response.text() }
    },
    { method, path, body },
  )
  expect(result.status, `${path} answered ${result.status}: ${result.body}`).toBe(200)
}

const renameNode = (page: Page, nodeId: string, displayName: string) =>
  adminWrite(page, 'PUT', `/api/admin/v1/nodes/${nodeId}/metadata`, { displayName })

/**
 * A Purge is the one Admin write that removes a Node outright: its row, its
 * metrics, and its identity all go, and the Server publishes the Public reset
 * Home revalidates from. The body confirms the Node by identity, as the route
 * insists, so a mis-aimed call cannot delete a scope nobody chose. It is
 * irreversible, which is why the journey that uses it runs on a Server of its
 * own instead of the fixture every other project is reading.
 */
const purgeNode = (page: Page, nodeId: string) =>
  adminWrite(page, 'POST', `/api/admin/v1/nodes/${nodeId}/purge`, { confirmNodeId: nodeId })

/** The reader's journey into one Node, taken through the link their reading
 *  already offers. */
async function openNode(page: Page, link: Locator, letter: string) {
  await link.click()
  await expect(page.getByRole('heading', { level: 1, name: new RegExp(`^Node ${letter} `) })).toBeVisible({ timeout: 15_000 })
}

test.describe('Public Home same-tab return (issue #224)', () => {
  test('returns to the Node the reader left from, in the same reading, with no place in the URL', async ({ page }) => {
    await loginAs(page)
    await page.goto(DEPARTURE_LIST_URL)
    await expect(listRows(page)).toHaveCount(6)

    await openNode(page, nodeLink(rowsOf(page, DEPARTED_LETTER)), DEPARTED_LETTER)
    await backToHome(page).click()

    // The reading comes back, and the address bar carries that reading and
    // nothing else: no place, no offset, nothing a copied link could inherit.
    await expect(listTable(page)).toBeVisible({ timeout: 15_000 })
    await expect.poll(() => returnedSearch(page)).toBe(RETURNED_LIST_SEARCH)
    await expect(listRows(page)).toHaveCount(6)
    // The reader is put back on the Node they left from, wherever the fresh
    // ranking has since put it, and their keyboard continues from that row.
    await expect(nodeLink(rowsOf(page, DEPARTED_LETTER))).toBeInViewport()
    await expect(nodeLink(rowsOf(page, DEPARTED_LETTER))).toBeFocused()
    await expect(returnNotice(page)).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
    // The reveal scrolls the reader onto the Node they left from, which leaves the
    // toolbar above the fold on a phone: it is brought back into the middle of the
    // viewport before the touch targets are measured, because a compact tab's touch
    // expansion reaches above the pill itself, and a toolbar flush with the
    // viewport's top edge cannot be hit tested where that expansion lies (a point
    // above the viewport hits nothing), which would report an honest 44px target as
    // its unexpanded 26px box.
    await page.getByLabel('Node filters and sorting').scrollIntoViewIfNeeded()
    await page.getByLabel('Node filters and sorting').evaluate((element) => element.scrollIntoView({ block: 'center' }))
    await expectVisibleInteractiveTargets(page)
  })

  test('follows the Node ID when a rename reorders the reading before the reader is back', async ({ page }) => {
    await loginAs(page)
    await page.goto(DEPARTURE_NAME_URL)
    await expect(listRows(page)).toHaveCount(6)

    // The third Node of the Name reading: Node L, with Node K right above it.
    await openNode(page, nodeLink(rowsOf(page, 'L')), 'L')

    try {
      // While the reader is away, another Owner gives Node K a name that sorts
      // last. The reading has really moved, so the place the departure recorded
      // now names a different Node.
      await renameNode(page, NODE_K_ID, RENAMED_K_NAME)
      await expect.poll(() => publicProjection(page), { timeout: 15_000 }).toContain(RENAMED_K_NAME)
      await backToHome(page).click()

      // The reading comes back with the fresh order in it...
      await expect.poll(() => returnedSearch(page)).toBe(RETURNED_NAME_SEARCH)
      await expect(listRows(page)).toHaveCount(6)
      await expect(listRows(page).last(), 'the renamed Node now ranks last').toContainText(RENAMED_K_NAME, { timeout: 15_000 })
      await expect(listRows(page).nth(2), 'the recorded place now names another Node').toContainText('Node M')
      // ...and the reader is on the Node they opened, not on the place they held.
      await expect(nodeLink(rowsOf(page, 'L'))).toBeInViewport()
      await expect(nodeLink(rowsOf(page, 'L'))).toBeFocused()
      await expect(nodeLink(rowsOf(page, 'M'))).not.toBeFocused()
      await expect(returnNotice(page)).toHaveCount(0)
      await expectNoHorizontalOverflow(page)
    } finally {
      await renameNode(page, NODE_K_ID, NODE_K_NAME)
    }
  })

  test('reports the Node a restored filter now hides, under the name it now has', async ({ page }) => {
    await loginAs(page)
    // The reader's own search, which only the two Nodes with this word in their
    // names answer.
    await page.goto(STALE_DEPARTURE_URL)
    await expect(listRows(page)).toHaveCount(2)

    await openNode(page, nodeLink(rowsOf(page, 'L')), 'L')

    try {
      // While the reader is away the Node is renamed out of the search that found
      // it: the reading they left still exists, but it no longer holds that Node.
      await renameNode(page, NODE_L_ID, RENAMED_L_NAME)
      await expect.poll(() => publicProjection(page), { timeout: 15_000 }).toContain(RENAMED_L_NAME)
      await backToHome(page).click()

      await expect.poll(() => returnedSearch(page)).toBe(RETURNED_STALE_SEARCH)
      await expect(listRows(page)).toHaveCount(1)
      // Home names the Node as it is called now, and points at the nearest result
      // of the reading the reader asked for.
      await expect(returnNotice(page)).toContainText(RENAMED_L_NAME, { timeout: 15_000 })
      await expect(returnNotice(page)).toContainText('is no longer shown by these filters.')
      await expect(returnNotice(page)).toContainText('Showing the nearest matching Active Node instead.')
      await expect(nodeLink(rowsOf(page, DEPARTED_LETTER))).toBeFocused()
      await expectNoHorizontalOverflow(page)
    } finally {
      await renameNode(page, NODE_L_ID, NODE_L_NAME)
    }
  })

  test('returns to the Node the reader left from in the card view, by keyboard', async ({ page }) => {
    await loginAs(page)
    await page.goto(DEPARTURE_CARD_URL)
    await expect(nodeGrid(page)).toBeVisible()

    await nodeLink(cardsOf(page, DEPARTED_LETTER)).focus()
    await page.keyboard.press('Enter')
    await expect(page.getByRole('heading', { level: 1, name: new RegExp(`^Node ${DEPARTED_LETTER} `) })).toBeVisible({ timeout: 15_000 })
    await backToHome(page).focus()
    await page.keyboard.press('Enter')

    await expect(nodeGrid(page)).toBeVisible({ timeout: 15_000 })
    await expect.poll(() => returnedSearch(page)).toBe(RETURNED_CARD_SEARCH)
    await expect(nodeLink(cardsOf(page, DEPARTED_LETTER))).toBeInViewport()
    await expect(nodeLink(cardsOf(page, DEPARTED_LETTER))).toBeFocused()
    await expect(listTable(page)).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('opens the Node and returns to it with a touch tap', async ({ page }, testInfo) => {
    test.skip(!testInfo.project.name.endsWith('-touch'), 'the tap path belongs to the touch projects')
    await loginAs(page)
    await page.goto(DEPARTURE_LIST_URL)
    await expect(listRows(page)).toHaveCount(6)

    await nodeLink(rowsOf(page, DEPARTED_LETTER)).tap()
    await expect(page.getByRole('heading', { level: 1, name: new RegExp(`^Node ${DEPARTED_LETTER} `) })).toBeVisible({ timeout: 15_000 })
    await backToHome(page).tap()

    await expect(nodeLink(rowsOf(page, DEPARTED_LETTER))).toBeInViewport()
    await expect(nodeLink(rowsOf(page, DEPARTED_LETTER))).toBeFocused()
    await expect.poll(() => returnedSearch(page)).toBe(RETURNED_LIST_SEARCH)
    await expect(returnNotice(page)).toHaveCount(0)
  })

  test('hands a reloaded Node page no reading to come back to', async ({ page }) => {
    await loginAs(page)
    await page.goto(DEPARTURE_LIST_URL)
    await expect(listRows(page)).toHaveCount(6)
    await openNode(page, nodeLink(rowsOf(page, DEPARTED_LETTER)), DEPARTED_LETTER)

    // A reload is another document, and a departure belongs to the document that
    // wrote it: the reading the reader left with is not this one's to hand back,
    // so the way home is an ordinary Home and not the reading they left.
    await page.reload()
    await expect(page.getByRole('heading', { level: 1, name: new RegExp(`^Node ${DEPARTED_LETTER} `) })).toBeVisible({ timeout: 15_000 })
    await expect(backToHome(page)).toHaveAttribute('href', '/')
    await backToHome(page).click()

    // An ordinary Home: the default card reading, nothing in the address bar,
    // no notice, and no Node for the keyboard to continue from.
    await expect(nodeGrid(page)).toBeVisible({ timeout: 15_000 })
    await expect(listTable(page)).toHaveCount(0)
    expect(returnedSearch(page)).toBe('')
    await expect(returnNotice(page)).toHaveCount(0)
    expect(await page.evaluate(() => document.activeElement?.getAttribute('href')?.startsWith('/nodes/') ?? false)).toBe(false)
  })

  test('a fresh Home inherits no reading position', async ({ page, context }) => {
    await loginAs(page)
    await page.goto(DEPARTURE_LIST_URL)
    await openNode(page, nodeLink(rowsOf(page, DEPARTED_LETTER)), DEPARTED_LETTER)
    await backToHome(page).click()
    await expect(nodeLink(rowsOf(page, DEPARTED_LETTER))).toBeFocused()

    // A direct entry in another tab: the departure lived in the first tab's own
    // history entry, so there is nothing here to inherit.
    const fresh = await context.newPage()
    await fresh.goto('/' + RETURNED_LIST_SEARCH)
    if (new URL(fresh.url()).pathname === '/login') {
      // The Public shell asks a new tab for its own session before Home renders.
      await loginAs(fresh)
      await fresh.goto('/' + RETURNED_LIST_SEARCH)
    }
    await expect(listTable(fresh)).toBeVisible({ timeout: 15_000 })
    await expect(returnNotice(fresh)).toHaveCount(0)
    expect(await fresh.evaluate(() => window.scrollY)).toBe(0)
    expect(await fresh.evaluate(() => document.activeElement?.getAttribute('href')?.startsWith('/nodes/') ?? false)).toBe(false)
    await fresh.close()
  })

  test('a fresh arrival at Home starts at the top of the page', async ({ page }) => {
    await loginAs(page)
    await page.goto(DEPARTURE_LIST_URL)
    await expect(listRows(page)).toHaveCount(6)
    await openNode(page, nodeLink(rowsOf(page, DEPARTED_LETTER)), DEPARTED_LETTER)

    // The reader scrolls down the Node they are reading and then leaves through
    // the shell's own brand, which asks for Home and nothing else: a fresh
    // arrival is the top of Home, never the offset the Node page was left at.
    await page.evaluate(() => document.scrollingElement?.scrollTo({ top: 600 }))
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(0)
    await page.locator('[data-slot="app-brand"]').click()

    await expect(nodeGrid(page)).toBeVisible({ timeout: 15_000 })
    await expect(listTable(page)).toHaveCount(0)
    expect(returnedSearch(page)).toBe('')
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0)
  })

  test('returns, and reports a refused reading and a Node the restored filter hides, on the dark surface', async ({ page }) => {
    await loginAs(page)
    await page.emulateMedia({ colorScheme: 'dark' })
    // A Network this deployment does not serve, and a word only two Node names
    // carry: the reading widens to every Node it does serve, Home says so on the
    // way in, and the reader's own search still answers exactly two of them.
    await page.goto(REFUSED_STALE_URL)
    await expect(page.locator('html')).toHaveClass(/dark/)
    await expect(filterNotice(page)).toBeVisible()
    await expect(listRows(page)).toHaveCount(2)
    const place = await rowIndexOf(page, 'Node L —')
    expect(place).toBeGreaterThanOrEqual(0)

    await openNode(page, nodeLink(rowsOf(page, 'L')), 'L')

    try {
      // While the reader is away the Node loses the word the restored search is
      // looking for: the reading they left still exists, and no longer holds the
      // Node. The rename republishes the Public cache for the same reader, and
      // that reader's own departure survives the republish.
      await renameNode(page, NODE_L_ID, RENAMED_L_NAME)
      await expect.poll(async () => (await publicProjection(page)).includes(RENAMED_L_NAME), { timeout: 15_000 }).toBe(true)
      await backToHome(page).click()

      // Both refusals meet the reader on the way back, on the dark surface, in
      // one paint: the Network the address bar names, and the Node the restored
      // search no longer shows.
      await expect.poll(() => returnedSearch(page)).toBe(RETURNED_REFUSED_STALE_SEARCH)
      await expect(filterNotice(page)).toBeVisible({ timeout: 15_000 })
      await expect(returnNotice(page)).toBeVisible({ timeout: 15_000 })
      await expect(returnNotice(page)).toContainText(RENAMED_L_NAME)
      await expect(returnNotice(page)).toContainText('is no longer shown by these filters.')
      await expect(listRows(page)).toHaveCount(1)
      // The return notice is stated in the Home filter notice's own audited tone,
      // so the dark surface reads it the way it reads every other notice.
      const tones = await page.evaluate(() => {
        const tone = (slot: string) => {
          const element = document.querySelector('[data-slot="' + slot + '"]')
          return element === null ? null : getComputedStyle(element).color
        }
        return { returned: tone('home-return-notice'), filtered: tone('home-filter-notice') }
      })
      expect(tones.returned).not.toBeNull()
      expect(tones.returned).toBe(tones.filtered)
      // The nearest result of the reading that actually arrived, clamped to the
      // list Home has now: where the reader's keyboard continues.
      const nearest = Math.min(place, (await listRows(page).count()) - 1)
      await expect(nodeLink(listRows(page).nth(nearest))).toBeFocused()
      await expect(nodeLink(listRows(page).nth(nearest))).toBeInViewport()
      await expectNoHorizontalOverflow(page)
      await page.getByLabel('Node filters and sorting').scrollIntoViewIfNeeded()
      await page.getByLabel('Node filters and sorting').evaluate((element) => element.scrollIntoView({ block: 'center' }))
      await expectVisibleInteractiveTargets(page)
    } finally {
      await renameNode(page, NODE_L_ID, NODE_L_NAME)
    }
  })
})

/**
 * The disappearance case needs a Server of its own. The one honest Admin write
 * that makes a reading stop serving a Node is a Purge: hiding a Node governs the
 * anonymous surface, and a transfer never moves a Node out of its Network. A
 * Purge removes every row the Node owns and publishes the Public reset Home
 * revalidates from — and it is irreversible, so it cannot be aimed at the shared
 * fixture every other project is reading. This journey therefore drives the same
 * production WebUI over the same real HTTP surface, against a temp SQLite Server
 * that is disposed when it ends.
 */
const DISPOSABLE_READING = '/?view=list&sort=name'
const RETURNED_DISPOSABLE_SEARCH = '?sort=name&view=list'
const REGISTERED_GENESIS = '0x' + '0'.repeat(63) + '1'
const REPORT_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_minimal.json'

/** The three Nodes this spec reports, and the one the reader loses: second by
 *  name, so the purged Node's recorded place names another Node and lands on the
 *  last row the reading has left. */
const REPORTED_NODES = [
  { nodeId: '0195f2a1-0014-4014-8014-000000000014', name: 'Node Q — Oldest Agent Inventory', port: 6790 },
  { nodeId: '0195f2a1-0015-4015-8015-000000000015', name: 'Node R — Purged While The Reader Was Away', port: 6791 },
  { nodeId: '0195f2a1-0016-4016-8016-000000000016', name: 'Node S — Newest Agent Inventory', port: 6792 },
]
const PURGED_NODE = REPORTED_NODES[1]

/** The parts of one Report Node section this spec rewrites. Everything else the
 *  fixture states is kept exactly as it is. */
type FixtureNode = {
  node_id: string
  chain: {
    network_identity: { attempted_at: string; latest_observed_at: string; latest: Record<string, unknown> }
    static_metadata: { attempted_at: string; latest_observed_at: string; latest: Record<string, unknown> }
  }
}

/** Instants whole seconds before one fixed now, so nothing this spec states
 *  drifts with the millisecond a call happens to land on. */
function clock(): (secondsAgo: number) => string {
  const now = Math.floor(Date.now() / 1000)
  return (secondsAgo: number): string => new Date((now - secondsAgo) * 1000).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/** One Report whose Inventory and Node sections declare the Nodes above on the
 *  Network Identity the disposable Server registered for platon-mainnet. */
function reportOf(nodes: typeof REPORTED_NODES, observedAt: string): (report: Record<string, unknown>) => void {
  const fixture = JSON.parse(readFileSync(REPORT_FIXTURE, 'utf8')) as { nodes: FixtureNode[] }
  const template = fixture.nodes[0]
  return (report) => {
    report.generated_at = observedAt
    report.report_sequence = 1
    report.inventory = {
      revision: 1,
      nodes: nodes.map((node) => ({ node_id: node.nodeId, network_key: 'platon-mainnet', rpc_endpoint: 'ws://127.0.0.1:' + node.port })),
    }
    report.nodes = nodes.map((node, index) => {
      const section = JSON.parse(JSON.stringify(template)) as FixtureNode
      section.node_id = node.nodeId
      const identity = section.chain.network_identity
      identity.attempted_at = observedAt
      identity.latest_observed_at = observedAt
      identity.latest.genesis_hash = REGISTERED_GENESIS
      const metadata = section.chain.static_metadata
      metadata.attempted_at = observedAt
      metadata.latest_observed_at = observedAt
      metadata.latest.node_key_fingerprint = '0x' + String(index + 4).repeat(40)
      delete metadata.latest.enode
      return section
    })
  }
}

test.describe('Public Home same-tab return after a Node is purged (issue #224)', () => {
  test('reports the Node the fresh projection no longer serves, and lands beside it', async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the Purge journey boots a disposable Server, which runs once per suite')
    test.setTimeout(300_000)

    const server = await startDisposableServer()
    try {
      const agent = await server.enrollAgent()
      await server.submitReport(agent.agentId, agent.credential, reportOf(REPORTED_NODES, clock()(5)))
      await loginToDisposableServer(page, server.baseUrl)
      for (const node of REPORTED_NODES) await renameNode(page, node.nodeId, node.name)

      await page.goto(server.baseUrl + DISPOSABLE_READING)
      await expect(listRows(page)).toHaveCount(3)
      const place = await rowIndexOf(page, PURGED_NODE.name)
      expect(place).toBe(1)

      await openNode(page, nodeLink(rowsOf(page, 'R')), 'R')

      // While the reader is away the Node is purged: the reading really stops
      // serving it, and the Server publishes the Public reset this reader
      // revalidates from. The Node page loses the Node too, which is how this
      // spec knows the surface in front of it has really moved.
      await purgeNode(page, PURGED_NODE.nodeId)
      await expect.poll(async () => (await publicProjection(page)).includes(PURGED_NODE.name), { timeout: 15_000 }).toBe(false)
      await expect(backToHome(page)).toBeVisible({ timeout: 30_000 })
      await expect(page.getByRole('heading', { level: 1 })).toHaveCount(0, { timeout: 30_000 })

      // The way home is still the reading the reader left — and the Node it no
      // longer holds is named, never silently skipped, with somewhere to land.
      await backToHome(page).click()
      await expect.poll(() => returnedSearch(page)).toBe(RETURNED_DISPOSABLE_SEARCH)
      await expect(listRows(page)).toHaveCount(2, { timeout: 15_000 })
      await expect(returnNotice(page)).toContainText(PURGED_NODE.nodeId, { timeout: 15_000 })
      await expect(returnNotice(page)).toContainText('is no longer in this view.')
      await expect(returnNotice(page)).toContainText('Showing the nearest Active Node instead.')
      const nearest = Math.min(place, (await listRows(page).count()) - 1)
      expect(nearest).toBe(1)
      await expect(nodeLink(listRows(page).nth(nearest))).toBeFocused()
      await expect(nodeLink(listRows(page).nth(nearest))).toBeInViewport()
      await expectNoHorizontalOverflow(page)
    } finally {
      await server.dispose()
    }
  })
})
