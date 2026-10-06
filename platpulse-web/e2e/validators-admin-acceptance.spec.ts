import { expect, test, type Locator, type Page } from '@playwright/test'
import { readFileSync } from 'node:fs'

import type {
  AdminNodeDetail,
  AdminNodeValidatorIdentity,
  Validator,
  ValidatorDetail,
} from '../src/api/generated'
import {
  VIEWPORTS,
  expectLocalTableScroll,
  expectResolvedTheme,
  focusByKeyboard,
  gotoAuthenticated,
  loginToDisposableServer,
} from './admin-flow'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
} from './helpers'
import { startDisposableServer, type DisposableServer } from './server-harness'

/**
 * The automatic Validator identity of a Node (issue #218, design §15.8).
 *
 * Identity on this surface is a chain correspondence the Server *resolves from
 * evidence*: the Network Identity and the full P2P public key an Agent observed,
 * compared with the Network the Operator registered. The whole acceptance of the
 * surface is that it never softens that evidence into a claim nobody made — a
 * Node whose observed identity contradicts the Registry is a mismatch rather
 * than an absence, a matching identity with no full key is Unknown rather than
 * "not a Validator", and a Provider that cannot be read leaves the verdict
 * unestablished with no age instead of a fresh zero.
 *
 * The spec states those outcomes through one real Report against a disposable
 * Server, asks the Server for its own answer first so every panel is judged
 * against the record rather than against a value this spec picked, and then
 * reads the Admin surfaces across all five viewports of the Admin matrix in both
 * resolved themes. A second test contradicts a Node's previously established
 * identity and checks that the association interval ends while the Validator and
 * its Node history stay readable. A third retires an identified Node and checks
 * that its retained identity and still-open interval stop claiming a Public
 * association, because the association is the Server's own fact and not a
 * reading of the identity state label. A fourth leaves the coverage page open,
 * states one Report, and waits for the Server's own discovery pass to publish the
 * association, so the page is proven to refresh from the Server's realtime signal
 * rather than from the navigation or restart the other three take.
 */

const REPORT_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_minimal.json'

/** The fixture Node is one of the three this spec seeds; its siblings only
 *  differ in the last four characters of their identifier. */
const NODE_A_ID = fixtureNodeId()
const NODE_B_ID = NODE_A_ID.slice(0, -4) + '0015'
const NODE_C_ID = NODE_A_ID.slice(0, -4) + '0016'

/** The full 32-byte P2P public key an Agent observed for Node A, and the enode
 *  URI that carries it. Only a full key can identify a Validator. */
const OBSERVED_P2P_KEY = '0x' + '3f'.repeat(64)
const OBSERVED_ENODE = 'enode://' + OBSERVED_P2P_KEY.slice(2) + '@10.0.0.1:30303'

/** The Network Identity the disposable Server registered for platon-mainnet,
 *  and the one the fixture declares. The fixture value is the registered value
 *  with its first digit changed, so a Node that keeps the fixture identity is
 *  observed to be on another Network without any invention by this spec. */
const REGISTERED_GENESIS = '0x' + '0'.repeat(63) + '1'
const CONTRADICTING_GENESIS = '0x' + 'a'.repeat(64)

/** The three surfaces this spec reads, named by their datum so no other card,
 *  table, or page region is ever matched by accident. */
const IDENTITY_TABLE = '[data-slot="validator-identity-table"]'
const VALIDATOR_TABLE = '[data-slot="validator-table"]'
const LINKS_TABLE = '[data-slot="validator-links-table"]'

/**
 * A clock whose instants are whole seconds a chosen age before one fixed now.
 * The Server's freshness arithmetic is then judged against an instant this spec
 * states, instead of drifting with the millisecond a call happens to land on.
 */
function clock(): (secondsAgo: number) => string {
  const now = Math.floor(Date.now() / 1000)
  return (secondsAgo: number): string =>
    new Date((now - secondsAgo) * 1000).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

function fixtureNodeId(): string {
  const report = JSON.parse(readFileSync(REPORT_FIXTURE, 'utf8')) as {
    nodes: { node_id: string }[]
  }
  return report.nodes[0].node_id
}

/** The parts of one Report Node section this spec rewrites. Everything else the
 *  fixture states — probe outcomes, revisions, the whole host section — is kept
 *  exactly as it is, so only the evidence under test changes. */
type FixtureNode = {
  node_id: string
  chain: {
    network_identity: {
      attempted_at: string
      latest_observed_at: string
      latest: Record<string, unknown>
    }
    static_metadata: {
      attempted_at: string
      latest_observed_at: string
      latest: Record<string, unknown>
    }
  }
}

/** The observed identity of one Node the Agent reports. */
type SeededNode = {
  nodeId: string
  /** The genesis hash the Agent observed for the Node's Network Identity. */
  genesisHash: string
  /** The enode URI carrying the full P2P public key, absent when the Agent
   *  observed no key at all — an absence of evidence, not a negative verdict. */
  enode?: string
  /** The Node key fingerprint, distinct per Node so the observations differ. */
  fingerprint: string
  /** The Node RPC endpoint port, distinct per Node. */
  port: number
}

/** One Node's Report section: the fixture's own observation with its Network
 *  Identity and observed key rewritten. */
function nodeSection(template: FixtureNode, node: SeededNode, observedAt: string): FixtureNode {
  const section = JSON.parse(JSON.stringify(template)) as FixtureNode
  section.node_id = node.nodeId
  const identity = section.chain.network_identity
  identity.attempted_at = observedAt
  identity.latest_observed_at = observedAt
  identity.latest.genesis_hash = node.genesisHash
  const metadata = section.chain.static_metadata
  metadata.attempted_at = observedAt
  metadata.latest_observed_at = observedAt
  metadata.latest.node_key_fingerprint = node.fingerprint
  if (node.enode === undefined) {
    delete metadata.latest.enode
  } else {
    metadata.latest.enode = node.enode
  }
  return section
}

/** One Report mutator that states the observed identity of every Node. */
function seedReport(
  nodes: SeededNode[],
  options: {
    generatedAt: string
    observedAt: string
    sequence?: number
    reportId?: string
    /** The Inventory revision the Agent declares. A changed Inventory must state a
     *  higher revision than the accepted one: re-declaring the accepted revision
     *  with different content is refused as an Inventory revision conflict
     *  (crates/platpulse-server/src/http/report_ingestion.rs:2202-2221), and the
     *  Server stores that refusal instead of retiring anything. */
    revision?: number
  },
): (report: Record<string, unknown>) => void {
  return (report) => {
    const template = (report.nodes as FixtureNode[])[0]
    report.generated_at = options.generatedAt
    report.report_sequence = options.sequence ?? 1
    // One Report id names one immutable body: a later Report from the same Agent
    // states its own id instead of editing the fixture's, which the Server
    // answers with report_identity_conflict (409).
    if (options.reportId !== undefined) {
      report.report_id = options.reportId
    }
    report.inventory = {
      revision: options.revision ?? 1,
      nodes: nodes.map((node) => ({
        node_id: node.nodeId,
        network_key: 'platon-mainnet',
        rpc_endpoint: 'ws://127.0.0.1:' + node.port,
      })),
    }
    report.nodes = nodes.map((node) => nodeSection(template, node, options.observedAt))
  }
}

/** The three identity outcomes this spec accepts, stated as evidence: a Node on
 *  the registered Network with its full key, a Node on another Network, and a
 *  Node on the registered Network whose full key was never observed. */
const SEEDED_NODES: SeededNode[] = [
  {
    nodeId: NODE_A_ID,
    genesisHash: REGISTERED_GENESIS,
    enode: OBSERVED_ENODE,
    fingerprint: '0x' + 'd'.repeat(40),
    port: 6790,
  },
  {
    nodeId: NODE_B_ID,
    genesisHash: CONTRADICTING_GENESIS,
    fingerprint: '0x' + 'e'.repeat(40),
    port: 6791,
  },
  {
    nodeId: NODE_C_ID,
    genesisHash: REGISTERED_GENESIS,
    fingerprint: '0x' + 'f'.repeat(40),
    port: 6792,
  },
]

/** The Node that only ever observed the registered Network Identity: the one
 *  the second test contradicts later. */
const SEEDED_NODE_A: SeededNode[] = [SEEDED_NODES[0]]

/** The Report that states the contradiction of that Node's identity. A Report id
 *  names one immutable body, so the contradiction is not an edit of the first. */
const SECOND_REPORT_ID = '0195f2a1-0018-4018-8018-000000000018'

/** The Report whose Inventory no longer declares the identified Node, which the
 *  Server answers by retiring it. A Report id names one immutable body. */
const RETIRE_REPORT_ID = '0195f2a1-001a-401a-801a-00000000001a'

/** A GET whose status this spec states: a surface that 500s is a failure here,
 *  not a rendered empty state the browser assertions could mistake for one. */
async function adminJson<T>(server: DisposableServer, path: string): Promise<T> {
  const response = await server.adminGet(path)
  expect(response.status, 'GET ' + path + ' answers 200').toBe(200)
  return response.body as T
}

/**
 * The identity coverage the Server resolved, awaited rather than raced. Discovery
 * runs from the Server's own startup pass, and readiness does not wait for it, so
 * a spec that read the surface immediately could judge an unfinished pass.
 */
async function waitForIdentities(
  server: DisposableServer,
  predicate: (rows: AdminNodeValidatorIdentity[]) => boolean,
  expected: string,
): Promise<AdminNodeValidatorIdentity[]> {
  const deadline = Date.now() + 60_000
  for (;;) {
    const rows = await adminJson<AdminNodeValidatorIdentity[]>(
      server,
      '/api/admin/v1/validator-identities',
    )
    if (predicate(rows)) return rows
    if (Date.now() > deadline) {
      throw new Error('the Server never reported ' + expected + ': ' + JSON.stringify(rows))
    }
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
}

/** The coverage row of one Node, found by the link its own Node cell carries. */
function identityRow(page: Page, nodeId: string): Locator {
  return page
    .locator(IDENTITY_TABLE + ' tbody tr')
    .filter({ has: page.locator('a[href="/admin/nodes/' + nodeId + '"]') })
}

/** The registry row of one Validator. */
function validatorRow(page: Page, validatorId: string): Locator {
  return page
    .locator(VALIDATOR_TABLE + ' tbody tr')
    .filter({ has: page.locator('a[href="/admin/validators/' + validatorId + '"]') })
}

/** One labeled value out of a detail grid, read by the label it belongs to. */
function detailValue(page: Page, label: string): Locator {
  return page.locator('dl > div:has(> dt:text-is("' + label + '")) > dd')
}

/** The identity state the Node page's own panel states as its `State` value.
 *  The label is also carried by two badges above it, so the value is read by the
 *  label it belongs to rather than by the text alone. */
function panelIdentityState(panel: Locator): Locator {
  return panel.locator('dl > div:has(> dt:text-is("State")) > dd')
}

/** The Validator identity panel of the Admin Node page. It is declared as its
 *  own card, so the innermost matching card is the panel and an outer card that
 *  merely contains it is never read as one. */
function nodeIdentityPanel(page: Page): Locator {
  return page
    .locator('[data-slot="card-x"]')
    .filter({ has: page.getByRole('heading', { level: 2, name: 'Validator identity' }) })
    .last()
}

/** Every claim the list surface makes about the three seeded Nodes. */
async function expectListSurface(page: Page, validatorId: string): Promise<void> {
  await expect(page.getByRole('heading', { level: 1, name: 'Validators' })).toBeVisible()
  // Two Nodes were not identified, and the coverage is counted per Node.
  await expect(page.getByText('2 unresolved', { exact: true })).toBeVisible()
  await expect(page.getByText('3 Nodes · 3 evaluated · 1 identified', { exact: true })).toBeVisible()

  const resolved = identityRow(page, NODE_A_ID)
  await expect(resolved).toHaveCount(1)
  await expect(resolved.getByText('Identified', { exact: true })).toBeVisible()
  await expect(resolved).toContainText(
    'An automatic Link is open and the Public projection shows this association.',
  )
  await expect(resolved.getByText('Shown', { exact: true })).toBeVisible()
  await expect(resolved).toContainText('Active Node with an open Link')
  // The observed key is shown in full to an Owner through its title, and the
  // citation is the key the Server resolved, not a shortened invention.
  await expect(resolved.locator('code')).toHaveAttribute('title', OBSERVED_P2P_KEY)
  await expect(resolved.locator('code')).toContainText(OBSERVED_P2P_KEY.slice(0, 8))
  await expect(resolved.locator('a[href="/admin/validators/' + validatorId + '"]')).toBeVisible()

  const mismatch = identityRow(page, NODE_B_ID)
  await expect(mismatch).toHaveCount(1)
  await expect(mismatch.getByText('Network identity mismatch', { exact: true })).toBeVisible()
  await expect(mismatch).toContainText('does not match this Node')
  await expect(mismatch.getByText('Not established', { exact: true })).toBeVisible()
  await expect(mismatch).toContainText('No Link to project')
  await expect(mismatch.getByText('Shown', { exact: true })).toHaveCount(0)
  // A contradicted identity observed no key: the cell says Unknown and carries
  // no tooltip, instead of keeping a key from a claim the Server rejected.
  await expect(mismatch.locator('code')).toHaveText('Unknown')
  expect(
    await mismatch.locator('code').getAttribute('title'),
    'a Node with no observed key names no key',
  ).toBeNull()

  const missingKey = identityRow(page, NODE_C_ID)
  await expect(missingKey).toHaveCount(1)
  await expect(missingKey.getByText('P2P public key missing', { exact: true })).toBeVisible()
  await expect(missingKey).toContainText('No full P2P public key has been observed')
  await expect(missingKey.getByText('Not established', { exact: true })).toBeVisible()
  await expect(missingKey).toContainText('No Link to project')

  // The registry keeps exactly the identity resolved from evidence. With no
  // Provider configured nothing about staking is readable, so the verdict is
  // unestablished with no age and no metric — Unknown, never a fresh zero.
  const registry = validatorRow(page, validatorId)
  await expect(registry).toHaveCount(1)
  await expect(registry.getByText('Validator status unknown', { exact: true })).toBeVisible()
  await expect(registry.locator('[data-label="Current status"]')).toContainText('Not established')
  await expect(registry.locator('[data-label="Current status"]')).toContainText(
    'Activity Observing · Not established',
  )
  await expect(registry.locator('[data-label="Last confirmed at"]')).toHaveText(
    'No confirmed verdict',
  )
  await expect(registry.locator('[data-label="Rank"]')).toContainText('Unknown')
  await expect(registry.locator('[data-label="Rank"]')).toContainText('Rank not established')
  await expect(registry.locator('[data-label="Stake"]')).toHaveText('Unknown')
  await expect(registry.locator('[data-label="Delegators"]')).toHaveText('Unknown')
  await expect(registry.locator('[data-label="Nodes"]')).toHaveText('1')

  await expect(page.locator(IDENTITY_TABLE + ' tbody tr')).toHaveCount(3)
  await expectLocalTableScroll(page, 'validator-identity-table')
  await expectLocalTableScroll(page, 'validator-table')
  await expectNoHorizontalOverflow(page)
}

/** Every claim the Validator detail surface makes about the one identity. */
async function expectDetailSurface(page: Page): Promise<void> {
  await expect(page.getByRole('heading', { level: 1 })).toContainText(OBSERVED_P2P_KEY.slice(0, 8))

  await expect(page.getByRole('heading', { level: 2, name: 'Current Validator status' })).toBeVisible()
  await expect(detailValue(page, 'Evidence')).toHaveText('Not established')
  await expect(detailValue(page, 'Last confirmed at')).toHaveText('No confirmed verdict')
  await expect(detailValue(page, 'Activity')).toHaveText('Observing · Not established')
  await expect(detailValue(page, 'Validator key')).toHaveText(OBSERVED_P2P_KEY)
  await expect(
    page.getByText('Current staking validity is not established; this is not a negative conclusion.'),
  ).toBeVisible()

  await expect(page.getByRole('heading', { level: 2, name: 'Provider evidence' })).toBeVisible()
  await expect(detailValue(page, 'Source')).toHaveText('Not configured')
  await expect(detailValue(page, 'Freshness')).toHaveText('Unknown')
  await expect(detailValue(page, 'Outcome')).toHaveText('not_configured')
  // The Server synthesizes a counter state for a Provider it never read; the
  // surface must not print that canonical marker as evidence.
  await expect(detailValue(page, 'Counter state')).toHaveText('Not observed')
  await expect(detailValue(page, 'Observed at')).toHaveText('Never observed')
  await expect(detailValue(page, 'Provider timestamp')).toHaveText('Not reported')
  await expect(detailValue(page, 'Attempted at')).toHaveText('Not attempted')
  await expect(detailValue(page, 'Diagnostic')).toHaveText('None reported')

  await expect(page.getByRole('heading', { level: 2, name: 'Staking metrics' })).toBeVisible()
  for (const label of [
    'Rank',
    'Stake',
    'Delegators',
    'Blocks produced',
    'Reward',
    'Reward rate',
    'Epoch',
  ]) {
    await expect(detailValue(page, label)).toHaveText('Unknown')
  }
  await expect(detailValue(page, 'Display name')).toHaveText('Not set')

  await expect(page.getByRole('heading', { level: 2, name: 'Node associations' })).toBeVisible()
  const links = page.locator(LINKS_TABLE + ' tbody tr')
  await expect(links).toHaveCount(1)
  await expect(links.locator('a[href="/admin/nodes/' + NODE_A_ID + '"]')).toBeVisible()
  await expect(links.getByText('Open', { exact: true })).toBeVisible()
  await expect(links).toContainText('Automatic identity, no manual role')
  await expectLocalTableScroll(page, 'validator-links-table')
  await expectNoHorizontalOverflow(page)
}

test('the automatic Validator identity the Server resolved reaches every Admin surface unsoftened', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix flow runs once per suite')
  test.setTimeout(600_000)

  const server = await startDisposableServer()
  try {
    const agent = await server.enrollAgent()
    const stamp = clock()
    await server.submitReport(
      agent.agentId,
      agent.credential,
      seedReport(SEEDED_NODES, { generatedAt: stamp(0), observedAt: stamp(5) }),
    )
    // Discovery runs from the Server's own startup pass, so the same state
    // directory is served again instead of reaching behind a running Server.
    await server.restart()

    const identities = await waitForIdentities(
      server,
      (rows) => rows.length === 3 && rows.every((row) => row.evaluatedAt !== null),
      'three evaluated Nodes',
    )
    const nodeA = identities.filter((row) => row.nodeId === NODE_A_ID)[0]
    const nodeB = identities.filter((row) => row.nodeId === NODE_B_ID)[0]
    const nodeC = identities.filter((row) => row.nodeId === NODE_C_ID)[0]

    // Node A: the observed identity matched the registered Network and the
    // observed full key became the identity the Server associated with it.
    expect(nodeA.state).toBe('identified')
    expect(nodeA.reason, 'a resolved identity needs no explanation').toBeNull()
    expect(nodeA.observedValidatorNodeKey).toBe(OBSERVED_P2P_KEY)
    expect(nodeA.validatorNodeKey).toBe(OBSERVED_P2P_KEY)
    expect(nodeA.lifecycle).toBe('active')
    expect(nodeA.associationEffective, 'an active Node with an open Link is projected').toBe(true)
    const validatorId = nodeA.validatorId
    expect(validatorId).not.toBeNull()

    // Node B: the identity was observed and contradicts the registered Network.
    expect(nodeB.state).toBe('network_identity_mismatch')
    expect(nodeB.reason).toContain('does not match this Node')
    expect(nodeB.observedValidatorNodeKey).toBeNull()
    expect(nodeB.validatorId).toBeNull()
    expect(nodeB.associationEffective).toBe(false)

    // Node C: the identity matched but no full key was ever observed, which is
    // an absence of evidence rather than evidence of absence.
    expect(nodeC.state).toBe('missing_public_key')
    expect(nodeC.reason).toContain('No full P2P public key has been observed')
    expect(nodeC.observedValidatorNodeKey).toBeNull()
    expect(nodeC.validatorId).toBeNull()

    const validators = await adminJson<Validator[]>(server, '/api/admin/v1/validators')
    expect(validators).toHaveLength(1)
    expect(validators[0].validatorId).toBe(validatorId)
    expect(validators[0].validatorNodeId).toBe(OBSERVED_P2P_KEY)
    expect(validators[0].linkCount, 'one Node carries this identity').toBe(1)
    // No Provider is configured on this Server, which is unreadable evidence:
    // the insight says so instead of claiming the keys do not stake.
    expect(validators[0].insight?.state).toBe('not_configured')
    expect(validators[0].insight?.source).toBe('disabled')
    expect(validators[0].insight?.currentValidatorStatus).toBe('unknown')
    expect(validators[0].insight?.lastGoodAgeSeconds ?? null).toBeNull()

    const detail = await adminJson<ValidatorDetail>(
      server,
      '/api/admin/v1/validators/' + String(validatorId),
    )
    expect(detail.links).toHaveLength(1)
    expect(detail.links[0].nodeId).toBe(NODE_A_ID)
    expect(detail.links[0].role ?? null).toBeNull()
    expect(detail.links[0].validUntil ?? null).toBeNull()

    // The Admin Node projection carries the same identity for this Node.
    const nodeDetail = await adminJson<AdminNodeDetail>(
      server,
      '/api/admin/v1/nodes/' + NODE_A_ID,
    )
    expect(nodeDetail.validator_identity?.state).toBe('identified')
    expect(nodeDetail.validator_identity?.validatorNodeKey).toBe(OBSERVED_P2P_KEY)

    const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
    try {
      const page = await context.newPage()
      await loginToDisposableServer(page, server.baseUrl)

      for (const viewport of VIEWPORTS) {
        for (const colorScheme of ['light', 'dark'] as const) {
          const where = viewport.width + 'x' + viewport.height + ' ' + colorScheme
          await page.setViewportSize(viewport)
          await page.emulateMedia({ colorScheme })
          testInfo.annotations.push({ type: 'surface', description: where })

          await gotoAuthenticated(page, server.baseUrl, '/admin/validators')
          await expectResolvedTheme(page, colorScheme)
          await expectListSurface(page, String(validatorId))

          if (viewport.width <= 768) {
            // The narrow Admin layout is driven by touch and by the keyboard:
            // every control stays a usable target, the search filter is
            // reachable by Tab with a visible focus ring, and filtering keeps
            // the same evidence rather than an empty list.
            await expectVisibleInteractiveTargets(page)
            const search = await focusByKeyboard(
              page,
              page.getByLabel('Search Validators and Nodes'),
            )
            await expect(search).toBeFocused()
            await expectFocusedElementHasVisibleFocus(page)
            await search.tap()
            await search.fill(NODE_C_ID)
            await expect(identityRow(page, NODE_A_ID)).toHaveCount(0)
            await expect(identityRow(page, NODE_C_ID)).toHaveCount(1)
            await expect(page.getByText('P2P public key missing', { exact: true })).toBeVisible()
            await expect(page.getByText('No Validator matches this search.', { exact: true })).toBeVisible()
            await search.fill('')
            await expect(identityRow(page, NODE_A_ID)).toHaveCount(1)
          }
          testInfo.annotations.pop()

          await gotoAuthenticated(page, server.baseUrl, '/admin/validators/' + String(validatorId))
          await expectResolvedTheme(page, colorScheme)
          await expectDetailSurface(page)
        }
      }

      // The Node entry of the same identity, read once at the desktop viewport
      // the Admin matrix fixes: the panel is the same distinct fact on the page
      // the Operator reaches from the coverage row.
      await page.setViewportSize({ width: 1280, height: 800 })
      await page.emulateMedia({ colorScheme: 'light' })

      await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + NODE_A_ID)
      const resolvedPanel = nodeIdentityPanel(page)
      await expect(resolvedPanel).toHaveCount(1)
      await expect(resolvedPanel).toContainText(
        'An automatic Link is open and the Public projection shows this association.',
      )
      await expect(panelIdentityState(resolvedPanel)).toHaveText('Identified')
      await expect(resolvedPanel.locator('code')).toHaveAttribute('title', OBSERVED_P2P_KEY)
      await expect(
        resolvedPanel.locator('a[href="/admin/validators/' + String(validatorId) + '"]'),
      ).toBeVisible()
      await expect(resolvedPanel).toContainText('Open, and shown in the Public projection')
      await expect(resolvedPanel).toContainText('Active')
      await expect(resolvedPanel).not.toContainText('Never evaluated')

      await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + NODE_B_ID)
      const mismatchPanel = nodeIdentityPanel(page)
      await expect(mismatchPanel).toHaveCount(1)
      await expect(panelIdentityState(mismatchPanel)).toHaveText('Network identity mismatch')
      await expect(mismatchPanel).toContainText('does not match this Node')
      await expect(mismatchPanel).toContainText('Not established')
      await expect(mismatchPanel).toContainText('No automatic Link to project')
      await expect(mismatchPanel).not.toContainText('Not a Validator')

      await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + NODE_C_ID)
      const missingKeyPanel = nodeIdentityPanel(page)
      await expect(missingKeyPanel).toHaveCount(1)
      await expect(panelIdentityState(missingKeyPanel)).toHaveText('P2P public key missing')
      await expect(missingKeyPanel).toContainText('No full P2P public key has been observed')
      await expect(missingKeyPanel).toContainText('No automatic Link to project')
      await expect(missingKeyPanel).not.toContainText('Not a Validator')
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

test('a contradicted identity ends its association interval but keeps the Validator history', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the lifecycle flow runs once per suite')
  test.setTimeout(300_000)

  const server = await startDisposableServer()
  try {
    const agent = await server.enrollAgent()
    const first = clock()
    await server.submitReport(
      agent.agentId,
      agent.credential,
      seedReport(SEEDED_NODE_A, { generatedAt: first(0), observedAt: first(5) }),
    )
    await server.restart()

    const identified = await waitForIdentities(
      server,
      (rows) => rows.length === 1 && rows[0].state === 'identified',
      'an identified Node',
    )
    const validatorId = identified[0].validatorId
    expect(validatorId).not.toBeNull()
    expect(identified[0].associationEffective).toBe(true)

    // The same Agent now observes this Node's identity on another Network: the
    // projection must stop, and the record must stay.
    const second = clock()
    await server.submitReport(
      agent.agentId,
      agent.credential,
      seedReport([{ ...SEEDED_NODES[0], genesisHash: CONTRADICTING_GENESIS }], {
        generatedAt: second(0),
        observedAt: second(5),
        sequence: 2,
        reportId: SECOND_REPORT_ID,
      }),
    )
    await server.restart()

    const contradicted = await waitForIdentities(
      server,
      (rows) => rows.length === 1 && rows[0].state === 'network_identity_mismatch',
      'a contradicted Node',
    )
    expect(contradicted[0].reason).toContain('does not match this Node')
    expect(contradicted[0].observedValidatorNodeKey).toBeNull()
    expect(contradicted[0].associationEffective).toBe(false)

    // The interval ended rather than disappearing, and the Validator that was
    // once resolved from evidence is still readable with its Node history.
    const validators = await adminJson<Validator[]>(server, '/api/admin/v1/validators')
    expect(validators).toHaveLength(1)
    expect(validators[0].linkCount).toBe(1)
    const detail = await adminJson<ValidatorDetail>(
      server,
      '/api/admin/v1/validators/' + String(validatorId),
    )
    expect(detail.links).toHaveLength(1)
    expect(detail.links[0].nodeId).toBe(NODE_A_ID)
    expect(detail.links[0].validUntil ?? null).not.toBeNull()

    const context = await browser.newContext({
      hasTouch: true,
      colorScheme: 'light',
      viewport: { width: 1280, height: 800 },
    })
    try {
      const page = await context.newPage()
      await loginToDisposableServer(page, server.baseUrl)

      await gotoAuthenticated(page, server.baseUrl, '/admin/validators')
      await expect(page.getByText('1 unresolved', { exact: true })).toBeVisible()
      await expect(page.getByText('1 Node · 1 evaluated · 0 identified', { exact: true })).toBeVisible()
      const contradictedRow = identityRow(page, NODE_A_ID)
      await expect(contradictedRow).toHaveCount(1)
      await expect(contradictedRow.getByText('Network identity mismatch', { exact: true })).toBeVisible()
      await expect(contradictedRow).toContainText('No Link to project')
      // The Validator survives the contradiction: an Operator can still reach
      // the identity and the Nodes it was seen on.
      const registry = validatorRow(page, String(validatorId))
      await expect(registry).toHaveCount(1)
      await expect(registry.locator('[data-label="Nodes"]')).toHaveText('1')

      await gotoAuthenticated(page, server.baseUrl, '/admin/validators/' + String(validatorId))
      const links = page.locator(LINKS_TABLE + ' tbody tr')
      await expect(links).toHaveCount(1)
      await expect(links.locator('a[href="/admin/nodes/' + NODE_A_ID + '"]')).toBeVisible()
      await expect(links.getByText('Ended', { exact: true })).toBeVisible()
      await expect(links).not.toContainText('Open')
      await expect(links).toContainText('Automatic identity, no manual role')

      await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + NODE_A_ID)
      const panel = nodeIdentityPanel(page)
      await expect(panel).toHaveCount(1)
      await expect(panelIdentityState(panel)).toHaveText('Network identity mismatch')
      await expect(panel).toContainText('No automatic Link to project')
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})
test('a retired Node keeps its retained identity without claiming a Public association', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the lifecycle flow runs once per suite')
  test.setTimeout(300_000)

  const server = await startDisposableServer()
  try {
    const agent = await server.enrollAgent()
    const first = clock()
    await server.submitReport(
      agent.agentId,
      agent.credential,
      seedReport(SEEDED_NODE_A, { generatedAt: first(0), observedAt: first(5) }),
    )
    await server.restart()

    const identified = await waitForIdentities(
      server,
      (rows) => rows.length === 1 && rows[0].state === 'identified',
      'an identified Node',
    )
    const validatorId = identified[0].validatorId
    expect(validatorId).not.toBeNull()
    expect(identified[0].associationEffective).toBe(true)

    // The latest Agent Inventory no longer declares this Node, so the Server
    // retires it. Discovery only examines Active Nodes, so the retained record
    // and the open interval stay exactly as they were: the association is the
    // Server own fact, while the Public projection stops with the lifecycle.
    const retired = clock()
    await server.submitReport(
      agent.agentId,
      agent.credential,
      seedReport([SEEDED_NODES[2]], {
        generatedAt: retired(0),
        observedAt: retired(5),
        sequence: 2,
        reportId: RETIRE_REPORT_ID,
        revision: 2,
      }),
    )
    await server.restart()

    const coverage = await waitForIdentities(
      server,
      (rows) => rows.some((row) => row.nodeId === NODE_A_ID && row.lifecycle === 'retired'),
      'a retired Node',
    )
    const retained = coverage.filter((row) => row.nodeId === NODE_A_ID)[0]
    expect(retained.state).toBe('identified')
    expect(retained.validatorId).toBe(validatorId)
    expect(retained.associationEffective).toBe(false)
    const detail = await adminJson<ValidatorDetail>(
      server,
      '/api/admin/v1/validators/' + String(validatorId),
    )
    expect(detail.links).toHaveLength(1)
    expect(detail.links[0].nodeId).toBe(NODE_A_ID)
    expect(detail.links[0].validUntil ?? null).toBeNull()

    const context = await browser.newContext({
      hasTouch: true,
      colorScheme: 'light',
      viewport: { width: 1280, height: 800 },
    })
    try {
      const page = await context.newPage()
      await loginToDisposableServer(page, server.baseUrl)

      await gotoAuthenticated(page, server.baseUrl, '/admin/validators')
      const row = identityRow(page, NODE_A_ID)
      await expect(row).toHaveCount(1)
      await expect(row).toContainText('Identified')
      // Not an expired claim and not an absent one: the interval is still open
      // and only its Public projection stops, which is what the row says.
      await expect(row).toContainText('Open Link, Node not Active')
      await expect(row).not.toContainText('No Link to project')
      await expect(row.getByText('Shown', { exact: true })).toHaveCount(0)

      await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + NODE_A_ID)
      const panel = nodeIdentityPanel(page)
      await expect(panel).toHaveCount(1)
      await expect(panelIdentityState(panel)).toHaveText('Identified')
      await expect(panel).toContainText(
        'Open, but the Node is not Active so Public shows no association',
      )
      await expect(panel).toContainText('Retired')
      // A retired Node is not a purged one: its record stays reachable from the
      // Validator it was identified as.
      await expect(
        panel.locator('a[href="/admin/validators/' + String(validatorId) + '"]'),
      ).toBeVisible()
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})

/**
 * The live path: the coverage page is already open when the evidence arrives, and
 * the Server's own discovery pass is the only thing that can tell it so.
 *
 * Discovery runs from the Server's startup pass and then on its own cadence, and
 * the Report's own invalidation lands before that pass has run — so this page can
 * only become correct if the pass publishes an invalidation of its own. Nothing
 * here restarts the Server, reloads the page, or navigates, and the navigation
 * listener states that: the state change came from the realtime signal.
 */
test('an already-open Validators page learns about a discovery pass', async ({ browser }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the live flow runs once per suite')
  test.setTimeout(300_000)

  const server = await startDisposableServer()
  try {
    const agent = await server.enrollAgent()
    const context = await browser.newContext({
      hasTouch: true,
      colorScheme: 'light',
      viewport: { width: 1280, height: 800 },
    })
    try {
      const page = await context.newPage()
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, '/admin/validators')
      // No Node has reported yet, so there is nothing the Server could evaluate,
      // and the card says exactly that instead of certifying the silence.
      await expect(page.getByText('Nothing evaluated', { exact: true })).toBeVisible()
      await expect(
        page.getByText('No Node has been evaluated for an automatic Validator identity yet.'),
      ).toBeVisible()

      const navigations: string[] = []
      page.on('framenavigated', (frame) => {
        if (frame === page.mainFrame()) navigations.push(frame.url())
      })

      const live = clock()
      await server.submitReport(
        agent.agentId,
        agent.credential,
        seedReport(SEEDED_NODE_A, { generatedAt: live(0), observedAt: live(5) }),
      )

      // The Report's own invalidation refetches this page before discovery runs,
      // so the row becomes Identified only when the pass publishes its change.
      const row = identityRow(page, NODE_A_ID)
      await expect(row).toHaveCount(1, { timeout: 30_000 })
      await expect(row).toContainText('Identified', { timeout: 150_000 })
      await expect(page.getByText('1 Node · 1 evaluated · 1 identified')).toBeVisible()
      await expect(page.getByText('Resolved', { exact: true })).toBeVisible()
      await expect(page.locator(VALIDATOR_TABLE + ' tbody tr')).toHaveCount(1)
      expect(navigations, 'the page refreshed from the realtime signal').toHaveLength(0)
    } finally {
      await context.close()
    }
  } finally {
    await server.dispose()
  }
})
