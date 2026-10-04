import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { formatAmountExact } from '../lib/amount'

const OWNER_SESSION = {
  session: {
    userId: 'u1',
    username: 'admin',
    role: 'owner',
    createdAt: '2026-08-12T00:00:00Z',
    lastSeenAt: '2026-08-12T00:00:00Z',
    expiresAt: '2026-08-19T00:00:00Z',
  },
  csrfToken: 'csrf-token',
}

const NODE_A_ID = '0195f2a1-0014-4014-8014-000000000014'
const NODE_B_ID = '0195f2a1-0015-4015-8015-000000000015'
const NODE_C_ID = '0195f2a1-0016-4016-8016-000000000016'
const NODE_D_ID = '0195f2a1-0017-4017-8017-000000000017'

const KEY_A = '0x' + 'a'.repeat(128)
const KEY_B = '0x' + 'b'.repeat(128)
const KEY_C = '0x' + 'c'.repeat(128)
const KEY_D = '0x' + 'd'.repeat(128)

const MISMATCH_REASON =
  "The observed P2P public key belongs to a different Network identity than the Node's Agent reported."

/** An identified Node whose automatic Link is projected into the Public view. */
const IDENTIFIED = {
  nodeId: NODE_A_ID,
  nodeDisplayName: 'Node A',
  networkKey: 'platon-mainnet',
  lifecycle: 'active',
  state: 'identified',
  reason: null,
  observedValidatorNodeKey: KEY_A,
  validatorId: 'v-1',
  validatorNodeKey: KEY_B,
  associationEffective: true,
  evaluatedAt: '2026-03-01T00:00:00Z',
}

/** Identified, but the Node is not Active, so Public projects no association. */
const IDENTIFIED_NOT_ACTIVE = {
  ...IDENTIFIED,
  nodeId: NODE_B_ID,
  nodeDisplayName: 'Node B',
  lifecycle: 'retired',
  observedValidatorNodeKey: KEY_C,
  validatorId: 'v-2',
  validatorNodeKey: KEY_C,
  associationEffective: false,
}

/** A contradicted identity: no Validator is established, and none is absent. */
const MISMATCHED = {
  ...IDENTIFIED,
  nodeId: NODE_C_ID,
  nodeDisplayName: null,
  state: 'network_identity_mismatch',
  reason: MISMATCH_REASON,
  observedValidatorNodeKey: KEY_D,
  validatorId: null,
  validatorNodeKey: null,
  associationEffective: false,
}

/** A Node the Server never evaluated. */
const NOT_EVALUATED = {
  ...IDENTIFIED,
  nodeId: NODE_D_ID,
  nodeDisplayName: 'Node D',
  state: 'not_evaluated',
  observedValidatorNodeKey: null,
  validatorId: null,
  validatorNodeKey: null,
  associationEffective: false,
  evaluatedAt: null,
}

const STAKE_A = '1234000000000000000000'

/** A freshly observed Validator with the full last-good evidence. */
const VALIDATOR_A = {
  validatorId: 'v-1',
  validatorNodeId: KEY_B,
  networkKey: 'platon-mainnet',
  displayName: 'Alpha Validator',
  linkCount: 2,
  createdAt: '2026-02-01T00:00:00Z',
  updatedAt: '2026-03-01T00:00:00Z',
  insight: {
    currentValidatorStatus: 'validator',
    currentValidatorStatusState: 'current',
    currentValidatorStatusQualifier: null,
    activity: 'producing',
    activityState: 'current',
    rank: 12,
    stakeAmount: STAKE_A,
    delegatorCount: 18,
    blockCount: 4321,
    rewardAmount: '5000000000000000000',
    rewardRate: '0.0525',
    epoch: 812,
    source: 'platsScan',
    freshness: 'fresh',
    outcome: 'success',
    counterState: 'monotonic',
    state: 'ok',
    validatorNodeId: KEY_B,
    receivedAt: '2026-03-01T00:00:00Z',
    attemptedAt: '2026-03-01T00:00:00Z',
    providerTimestamp: '2026-02-28T23:59:00Z',
    lastGoodReceivedAt: '2026-03-01T00:00:00Z',
    lastGoodAgeSeconds: 95,
    diagnostic: null,
  },
}

/** The same identity after the Provider stopped answering: retained, stale. */
const VALIDATOR_B = {
  ...VALIDATOR_A,
  validatorId: 'v-2',
  validatorNodeId: KEY_C,
  displayName: 'Beta Validator',
  linkCount: 1,
  insight: {
    ...VALIDATOR_A.insight,
    currentValidatorStatusState: 'stale',
    activityState: 'stale',
    rank: 88,
    stakeAmount: null,
    delegatorCount: null,
    rewardAmount: null,
    rewardRate: null,
    epoch: null,
    freshness: 'stale',
    outcome: 'error',
    diagnostic: 'provider request timed out',
    lastGoodAgeSeconds: 7200,
    validatorNodeId: KEY_C,
  },
}

/** A chain identity the Server resolved but never observed successfully. */
const VALIDATOR_C = {
  validatorId: 'v-3',
  validatorNodeId: KEY_D,
  networkKey: 'platon-mainnet',
  displayName: 'Gamma Validator',
  linkCount: 0,
  createdAt: '2026-02-01T00:00:00Z',
  updatedAt: '2026-02-01T00:00:00Z',
  insight: null,
}

const LINK_OPEN = {
  linkId: 'link-1',
  validatorId: 'v-1',
  validatorNodeId: KEY_B,
  nodeId: NODE_A_ID,
  nodeDisplayName: 'Node A',
  networkKey: 'platon-mainnet',
  role: null,
  validFrom: '2026-02-01T00:00:00Z',
  validUntil: null,
  createdAt: '2026-02-01T00:00:00Z',
  updatedAt: '2026-02-01T00:00:00Z',
}

/** An interval that ended at a key change: the earlier association stays readable. */
const LINK_ENDED = {
  ...LINK_OPEN,
  linkId: 'link-2',
  nodeId: NODE_B_ID,
  nodeDisplayName: 'Node Retired',
  validUntil: '2026-02-20T00:00:00Z',
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const TEST_ORIGIN = 'http://platpulse.test'

function mockFetch(
  routes: Record<string, (request: Request) => Response | Promise<Response>>,
) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const url = request.url.replace(TEST_ORIGIN, '')
    for (const [pattern, handler] of Object.entries(routes)) {
      if (pattern.endsWith('*')) {
        if (url.startsWith(pattern.slice(0, -1))) return handler(request)
      } else if (url === pattern) {
        return handler(request)
      }
    }
    return jsonResponse({ error: { code: 'not_found' } }, 404)
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

function registryRoutes(overrides: Record<string, unknown> = {}) {
  return {
    '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    '/api/admin/v1/validators': () =>
      jsonResponse(overrides.validators ?? [VALIDATOR_A, VALIDATOR_B, VALIDATOR_C], 200),
    '/api/admin/v1/validator-identities': () =>
      jsonResponse(
        overrides.identities ?? [IDENTIFIED, IDENTIFIED_NOT_ACTIVE, MISMATCHED, NOT_EVALUATED],
        200,
      ),
  }
}

async function renderAt(path: string) {
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', path)
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
}

beforeEach(() => {
  window.history.replaceState({}, '', '/')
  client.setConfig({ baseUrl: TEST_ORIGIN })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  // Each case owns its own Server answer: a cached registry from the previous
  // test must never stand in for the one under test.
  adminQueryClient.clear()
})

describe('PAGE-ADMIN-VALIDATORS (automatic identity and registry)', () => {
  it('shows one coverage row per evaluated Node, with the unresolved states named and the Public association kept separate', async () => {
    mockFetch(registryRoutes())
    renderAt('/admin/validators')

    await screen.findByRole('heading', { level: 1, name: 'Validators' })
    await screen.findByText(/4 Nodes/)
    expect(screen.getByText('1 unresolved')).toBeTruthy()
    expect(screen.getByText(/4 Nodes · 3 evaluated · 2 identified/)).toBeTruthy()

    const nodeA = await screen.findByRole('row', { name: /Node A/ })
    expect(nodeA.textContent).toContain('Identified')
    expect(nodeA.textContent).toContain(
      'An automatic Link is open and the Public projection shows this association.',
    )
    expect(nodeA.textContent).toContain('Shown')
    expect(nodeA.textContent).toContain('Active Node with an open Link')
    // The full observed key stays inspectable even though the cell is shortened.
    expect(within(nodeA).getByTitle(KEY_A)).toBeTruthy()
    expect(within(nodeA).getByRole('link', { name: /Node A/ }).getAttribute('href')).toBe(
      '/admin/nodes/' + NODE_A_ID,
    )
    expect(
      within(nodeA).getAllByRole('link').some((link) => link.getAttribute('href') === '/admin/validators/v-1'),
    ).toBe(true)

    const nodeB = screen.getByRole('row', { name: /Node B/ })
    expect(nodeB.textContent).toContain('Not shown')
    expect(nodeB.textContent).toContain('Open Link, Node not Active')

    // A contradicted identity is an unresolved state with its own reason, not an absence.
    const nodeC = screen.getByRole('row', { name: new RegExp(NODE_C_ID) })
    expect(nodeC.textContent).toContain('Network identity mismatch')
    expect(nodeC.textContent).toContain(MISMATCH_REASON)
    expect(nodeC.textContent).toContain('Not established')
    expect(nodeC.textContent).toContain('No Link to project')

    const nodeD = screen.getByRole('row', { name: /Node D/ })
    expect(nodeD.textContent).toContain('Not evaluated')
    expect(nodeD.textContent).toContain(
      'No automatic Validator identity has been established for this Node.',
    )
    expect(nodeD.textContent).toContain('Never evaluated')
  })

  it('reports the verdict with its currency, the staking metrics, and Unknown instead of a fresh zero', async () => {
    mockFetch(registryRoutes())
    renderAt('/admin/validators')

    await screen.findByRole('heading', { level: 1, name: 'Validators' })
    const alpha = await screen.findByRole('row', { name: /Alpha Validator/ })
    expect(alpha.textContent).toContain('Validator')
    expect(alpha.textContent).toContain('Current')
    expect(alpha.textContent).toContain('Producing')
    expect(alpha.textContent).toContain('#12')
    expect(alpha.textContent).toContain(formatAmountExact(STAKE_A))
    expect(alpha.textContent).toContain('18')

    // The Provider stopped answering: the last-good verdict stands, marked retained.
    const beta = screen.getByRole('row', { name: /Beta Validator/ })
    expect(beta.textContent).toContain('Retained (stale)')
    expect(beta.textContent).toContain('#88')
    expect(beta.textContent).toContain('Unknown')
    expect(beta.textContent).not.toContain('0%')

    // A verdict that was never established is unknown, never a negative conclusion.
    const gamma = screen.getByRole('row', { name: /Gamma Validator/ })
    expect(gamma.textContent).toContain('Validator status unknown')
    expect(gamma.textContent).toContain('Not established')
    expect(gamma.textContent).toContain('No last-good observation')
  })

  it('filters both tables from one search field and explains the empty result', async () => {
    mockFetch(registryRoutes())
    renderAt('/admin/validators')

    await screen.findByRole('heading', { level: 1, name: 'Validators' })
    await screen.findByRole('row', { name: /Node A/ })
    fireEvent.change(screen.getByLabelText('Search Validators and Nodes'), {
      target: { value: 'Node D' },
    })

    expect(screen.queryByRole('row', { name: /Node A/ })).toBeNull()
    expect(await screen.findByRole('row', { name: /Node D/ })).toBeTruthy()
    expect(await screen.findByText('No Validator matches this search.')).toBeTruthy()
    // The summary counts the whole answer, so a search that hides the
    // unresolved Nodes cannot certify coverage.
    expect(screen.getByText('1 unresolved')).toBeTruthy()
    expect(screen.getByText(/4 Nodes · 3 evaluated · 2 identified/)).toBeTruthy()
  })

  it('shows the Empty state when no Node was evaluated and no Validator was resolved', async () => {
    mockFetch(registryRoutes({ identities: [], validators: [] }))
    renderAt('/admin/validators')

    await screen.findByRole('heading', { level: 1, name: 'Validators' })
    expect(
      await screen.findByText('No Node has been evaluated for an automatic Validator identity yet.'),
    ).toBeTruthy()
    // An empty answer is not coverage that passed.
    expect(await screen.findByText('Nothing evaluated')).toBeTruthy()
    expect(screen.queryByText('Resolved')).toBeNull()
    expect(screen.getByText(/0 Nodes · 0 evaluated · 0 identified/)).toBeTruthy()
    expect(
      screen.getByText(
        'No Validator has been resolved yet. A Validator appears once an Agent observation establishes one.',
      ),
    ).toBeTruthy()
  })

  it('keeps a failed identity load actionable without inventing coverage', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/validators': () => jsonResponse([], 200),
      '/api/admin/v1/validator-identities': () =>
        jsonResponse({ error: { code: 'internal', message: 'server database is unavailable' } }, 503),
    })
    renderAt('/admin/validators')

    // The identity dimension reports its own failure and stays retryable; it
    // never renders an empty coverage table as if every Node were unresolved.
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toMatch(
      /server database is unavailable|Unable to load Validator identity coverage/,
    )
    expect(within(alert).getByRole('button', { name: 'Try again' })).toBeTruthy()
    expect(screen.queryByRole('row', { name: /Node A/ })).toBeNull()
    expect(
      await screen.findByText(
        'No Validator has been resolved yet. A Validator appears once an Agent observation establishes one.',
      ),
    ).toBeTruthy()
  })
})

describe('PAGE-ADMIN-VALIDATOR-DETAIL (one chain identity)', () => {
  it('shows the verdict, the Provider evidence, the staking metrics, and the Node associations', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/validators/v-1': () =>
        jsonResponse({ ...VALIDATOR_A, links: [LINK_OPEN, LINK_ENDED] }, 200),
    })
    renderAt('/admin/validators/v-1')

    await screen.findByRole('heading', { level: 1, name: /Alpha Validator/ })
    expect(screen.getByRole('heading', { name: 'Current Validator status' })).toBeTruthy()
    expect(screen.getAllByText('Validator').length).toBeGreaterThan(0)
    expect(screen.getByText('Current')).toBeTruthy()
    expect(screen.getByText('1m')).toBeTruthy()

    expect(screen.getByRole('heading', { name: 'Provider evidence' })).toBeTruthy()
    expect(screen.getByText('platsScan')).toBeTruthy()
    expect(screen.getByText('success')).toBeTruthy()
    expect(screen.getByText('monotonic')).toBeTruthy()

    expect(screen.getByRole('heading', { name: 'Staking metrics' })).toBeTruthy()
    expect(screen.getByText('#12')).toBeTruthy()
    expect(screen.getByText(formatAmountExact(STAKE_A))).toBeTruthy()
    expect(screen.getByText('4321')).toBeTruthy()
    expect(screen.getByText('0.0525%')).toBeTruthy()

    expect(screen.getByRole('heading', { name: 'Node associations' })).toBeTruthy()
    const nodeLink = screen.getByRole('link', { name: 'Node A' })
    expect(nodeLink.getAttribute('href')).toBe('/admin/nodes/' + NODE_A_ID)
    expect(screen.getByText('Open')).toBeTruthy()
    expect(screen.getByText('Ended')).toBeTruthy()
    // An automatic Link carries no manual role, and an interval that ended at a
    // key change stays readable; a Node Purge deletes the purged Node's own.
    expect(screen.getAllByText('Automatic identity, no manual role')).toHaveLength(2)
    expect(screen.getByRole('link', { name: 'Node Retired' })).toBeTruthy()
  })

  it('marks a retained verdict and an unestablished one, never as a fresh zero', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
      '/api/admin/v1/validators/v-2': () => jsonResponse({ ...VALIDATOR_B, links: [] }, 200),
    })
    renderAt('/admin/validators/v-2')

    await screen.findByRole('heading', { level: 1, name: /Beta Validator/ })
    expect(screen.getAllByText('Retained (stale)').length).toBeGreaterThan(0)
    expect(
      screen.getByText('The last successful verdict is retained; the latest refresh failed.'),
    ).toBeTruthy()
    expect(screen.getByText('provider request timed out')).toBeTruthy()
    // The metrics the last good answer never carried stay Unknown.
    expect(screen.getAllByText('Unknown').length).toBeGreaterThanOrEqual(4)
    expect(
      screen.getByText('No Node association is recorded for this Validator.'),
    ).toBeTruthy()
  })

  it('shows a non-leaking unavailable state for an unknown Validator', async () => {
    mockFetch({
      '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    })
    renderAt('/admin/validators/no-such-validator')

    await screen.findByRole('heading', { level: 1, name: 'Validator unavailable' })
    expect(
      screen.getByRole('link', { name: 'Back to Validators' }).getAttribute('href'),
    ).toBe('/admin/validators')
  })
})

/** One stored configured local day, for the trend panel assertions. */
function trendPoint(overrides: Record<string, unknown> = {}) {
  return {
    blockCount: 900,
    clockSuspect: false,
    dayEnd: '2026-02-01T15:00:00Z',
    dayStart: '2026-01-31T15:00:00Z',
    delaySeconds: 30,
    delegatorCount: 40,
    epoch: 5,
    localDate: '2026-02-01',
    monthKey: '2026-02',
    observationKey: 'observation-Asia/Tokyo-2026-02-01',
    providerTimestamp: '2026-01-31T15:00:00Z',
    rank: 12,
    receivedAt: '2026-01-31T15:00:30Z',
    rewardAmount: '25.000000',
    rewardRate: '0.05',
    sampleAt: '2026-01-31T15:00:00Z',
    sampleTime: 'provider',
    source: 'platsScan',
    stakeAmount: '1000.000000',
    ...overrides,
  }
}

function trendPage(overrides: Record<string, unknown> = {}) {
  return {
    answeredFromLocalDate: '2026-02-01',
    answeredToLocalDate: '2026-02-03',
    associationHistoryPartial: false,
    associations: [],
    associationsTruncated: false,
    clamped: false,
    continuation: null,
    counterSemantics: 'cumulative',
    coverage: 'partial',
    deletedNodes: 0,
    expectedDays: 3,
    firstObservedLocalDate: '2026-02-01',
    foreignRows: 0,
    foreignTimezones: [],
    gaps: [{ days: 1, fromLocalDate: '2026-02-03', toLocalDate: '2026-02-03' }],
    lastObservedLocalDate: '2026-02-02',
    missingDays: 1,
    months: [
      {
        firstLocalDate: '2026-02-01',
        lastLocalDate: '2026-02-02',
        monthEnd: '2026-02-28T15:00:00Z',
        monthKey: '2026-02',
        monthStart: '2026-01-31T15:00:00Z',
        observedDays: 2,
      },
    ],
    networkKey: 'platon-mainnet',
    observedDays: 2,
    points: [
      trendPoint(),
      trendPoint({
        dayEnd: '2026-02-02T15:00:00Z',
        dayStart: '2026-02-01T15:00:00Z',
        localDate: '2026-02-02',
        observationKey: 'observation-Asia/Tokyo-2026-02-02',
        providerTimestamp: null,
        receivedAt: '2026-02-02T01:00:00Z',
        sampleAt: '2026-02-02T01:00:00Z',
        sampleTime: 'receipt',
        delaySeconds: null,
      }),
    ],
    requestedDays: 3,
    requestedFrom: '2026-01-31T15:00:00Z',
    requestedFromLocalDate: '2026-02-01',
    requestedTo: '2026-02-03T15:00:00Z',
    requestedToLocalDate: '2026-02-03',
    timezone: 'Asia/Tokyo',
    truncated: false,
    validatorId: 'v-1',
    ...overrides,
  }
}

function detailRoutes(trend: () => Response) {
  return {
    '/api/public/v1/session': () => jsonResponse(OWNER_SESSION, 200),
    '/api/admin/v1/validators/v-1': () => jsonResponse({ ...VALIDATOR_A, links: [LINK_OPEN] }, 200),
    '/api/admin/v1/validators/v-1/trend*': trend,
  }
}

function calledUrls(fetchMock: ReturnType<typeof mockFetch>): string[] {
  return fetchMock.mock.calls.map((call) => {
    const input = call[0]
    return input instanceof Request ? input.url : String(input)
  })
}

describe('PAGE-ADMIN-VALIDATOR-TREND (daily snapshots over the configured calendar)', () => {
  it('answers the configured local days, their real width, their months, and every silence', async () => {
    mockFetch(detailRoutes(() => jsonResponse(trendPage(), 200)))
    renderAt('/admin/validators/v-1')

    await screen.findAllByText('Asia/Tokyo')
    expect(screen.getByRole('heading', { name: 'Daily trend' })).toBeTruthy()
    expect(screen.getAllByText('Partial').length).toBeGreaterThan(0)
    expect(screen.getByText('2 observed · 1 missing')).toBeTruthy()
    expect(screen.getByText('2026-02-01 → 2026-02-03 (3 day(s))')).toBeTruthy()
    // The configured month boundary is disclosed in the UTC coordinate.
    expect(screen.getByText('2026-01-31T15:00:00Z → 2026-02-28T15:00:00Z')).toBeTruthy()
    // A day with no snapshot is a silence, and never a zero.
    expect(
      screen.getByText('No snapshot was stored for 2026-02-03. It is a silence, not a zero.'),
    ).toBeTruthy()
    // The real width of a configured local day, not a pretended 24 hours.
    expect(screen.getAllByText(/24 hours/).length).toBeGreaterThan(0)
    // Which timestamp chose the day, and the delay a fallback cannot measure.
    expect(screen.getAllByText('Provider timestamp').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Server receipt (fallback)').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Unknown').length).toBeGreaterThan(0)
    // Cumulative counters are labelled cumulative, and no earnings are derived.
    expect(screen.getByText(/cumulative Provider counters as of each sample/)).toBeTruthy()
    expect(screen.getByText(/never computes period earnings, net profit/)).toBeTruthy()
    expect(screen.getByText('Reward (cumulative)')).toBeTruthy()
    expect(screen.getByText('Blocks (cumulative)')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Load older days' })).toBeTruthy()
  })

  it('reads a stored day without a metric as Unknown rather than a value', async () => {
    mockFetch(
      detailRoutes(() =>
        jsonResponse(
          trendPage({
            gaps: [],
            missingDays: 0,
            observedDays: 2,
            points: [trendPoint({ rank: null, delegatorCount: null })],
          }),
          200,
        ),
      ),
    )
    renderAt('/admin/validators/v-1')

    await screen.findByRole('columnheader', { name: 'Rank' })
    const points = document.querySelector('[data-slot="validator-trend-points"]')
    expect(points).not.toBeNull()
    // The stored row carries no rank and no delegator count, so both read Unknown.
    expect(within(points as HTMLElement).getAllByText('Unknown').length).toBeGreaterThanOrEqual(2)
    expect(
      screen.getByText('No configured local day of the answered stretch is missing a snapshot.'),
    ).toBeTruthy()
  })

  it('claims nothing when the stretch holds days and none of them was observed', async () => {
    mockFetch(
      detailRoutes(() =>
        jsonResponse(
          trendPage({
            coverage: 'unavailable',
            firstObservedLocalDate: null,
            gaps: [{ days: 3, fromLocalDate: '2026-02-01', toLocalDate: '2026-02-03' }],
            lastObservedLocalDate: null,
            missingDays: 3,
            months: [],
            observedDays: 0,
            points: [],
          }),
          200,
        ),
      ),
    )
    renderAt('/admin/validators/v-1')

    expect((await screen.findAllByText('Unavailable')).length).toBeGreaterThan(0)
    expect(screen.getByText(/missing evidence, not a zero and not a healthy stretch/)).toBeTruthy()
    expect(screen.getByText(/No snapshot was stored for any configured local day/)).toBeTruthy()
    expect(screen.getByText(/They are a silence, not a zero/)).toBeTruthy()
  })

  it('discloses foreign-bucket rows instead of merging them, and a purged Node instead of hiding it', async () => {
    mockFetch(
      detailRoutes(() =>
        jsonResponse(
          trendPage({
            associationHistoryPartial: true,
            associations: [],
            deletedNodes: 1,
            foreignRows: 2,
            foreignTimezones: ['UTC'],
          }),
          200,
        ),
      ),
    )
    renderAt('/admin/validators/v-1')

    expect(
      await screen.findByText(/never merged into the configured calendar or quietly re-bucketed/),
    ).toBeTruthy()
    expect(screen.getByText(/unavailable here rather than never having existed/)).toBeTruthy()
    expect(screen.getAllByText(/retained/).length).toBeGreaterThan(0)
    expect(
      screen.getByText(
        /the deleted Node intervals above are unavailable rather than never having existed/,
      ),
    ).toBeTruthy()
  })

  it('pages into strictly older days with the Server cursor and returns to the newest', async () => {
    const fetchMock = mockFetch(
      detailRoutes(() => jsonResponse(trendPage({ continuation: '2026-02-01', truncated: true }), 200)),
    )
    renderAt('/admin/validators/v-1')

    await screen.findByText(/holds more configured local days than this page answered/)
    expect(calledUrls(fetchMock).some((url) => url.includes('/trend?'))).toBe(true)
    expect(screen.getByRole('button', { name: 'Return to newest' }).hasAttribute('disabled')).toBe(
      true,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Load older days' }))
    await waitFor(() => {
      expect(calledUrls(fetchMock).some((url) => url.includes('before=2026-02-01'))).toBe(true)
    })
    expect(
      screen.getByRole('button', { name: 'Return to newest' }).hasAttribute('disabled'),
    ).toBe(false)

    fireEvent.click(screen.getByRole('button', { name: 'Return to newest' }))
    await waitFor(() => {
      expect(
        screen.getByRole('button', { name: 'Return to newest' }).hasAttribute('disabled'),
      ).toBe(true)
    })
  })

  it('keeps the panel honest when the trend itself cannot be loaded', async () => {
    mockFetch(
      detailRoutes(() =>
        jsonResponse(
          { error: { code: 'unavailable', message: 'server database is unavailable' } },
          503,
        ),
      ),
    )
    renderAt('/admin/validators/v-1')

    const alert = await screen.findByText(
      /server database is unavailable|Unable to load the Validator trend/,
    )
    expect(alert).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Current Validator status' })).toBeTruthy()
  })
})

