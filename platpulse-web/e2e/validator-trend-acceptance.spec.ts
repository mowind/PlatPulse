/**
 * Owner acceptance for the Validator daily trend (issue #219).
 *
 * GET /api/admin/v1/validators/{validator_id}/trend answers daily rank, stake
 * and delegator snapshots on the Server's *configured* IANA calendar, discloses
 * coverage, grain, source, delay, truncation and foreign-timezone rows instead of
 * assuming them, keeps cumulative Provider counters from reading as period
 * earnings, and keeps a Validator's stored days answering after a Node purge.
 *
 * Every assertion here travels WebUI -> real Server HTTP -> SQLite. The Agent
 * Report is real (the harness enrolls an Agent and submits the wire fixture), the
 * newest day is written by the Server's *own* Validator refresh loop, and the
 * deployment that loop talks to is ./platscan-replay.ts, a local replay of the
 * captured mainnet PlatScan responses — so no transport in the chain is mocked.
 *
 * The configured calendar is Asia/Kathmandu (UTC+05:45) on purpose: one local day
 * starts at 18:15:00Z of the previous UTC day, so a surface that quietly bucketed
 * by UTC midnight would fail this spec instead of passing on a zone whose offset
 * happens to be zero.
 */

import { expect, test, type Locator, type Page } from '@playwright/test'
import { readFileSync } from 'node:fs'

import type { AdminValidatorTrendResponse } from '../src/api/generated'
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
import { PLATSCAN_CAPTURE_NODE_ID, startPlatscanReplay, type PlatscanReplay } from './platscan-replay'
import { startDisposableServer, type DisposableServer } from './server-harness'

/** The seeded Validator this spec reads. Its Node id is the captured mainnet
 *  Validator, so the automatic link the Server discovers reuses this row. */
const VALIDATOR_ID = 'validator-trend-e2e'
const NETWORK_KEY = 'platon-mainnet'
/** The Server's configured daily/monthly calendar for this run. */
const TIMEZONE = 'Asia/Kathmandu'
/** Kathmandu has no daylight saving: its fixed offset is +05:45, so a local
 *  midnight is 18:15:00Z of the previous UTC day. */
const TIMEZONE_OFFSET = '+05:45'
const OWNER_PATH = '/admin/validators/' + VALIDATOR_ID
const PANEL = '[data-slot="validator-trend-panel"]'
const REPORT_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_minimal.json'
/** The genesis hash the harness Network is registered with (server-harness.ts). */
const REGISTERED_GENESIS = '0x' + '0'.repeat(63) + '1'
/** The Node announces the captured Validator's P2P public key as its enode. */
const OBSERVED_ENODE = 'enode://' + PLATSCAN_CAPTURE_NODE_ID.slice(2) + '@10.0.0.1:30303'
/** Consecutive configured local days seeded, ending yesterday. */
const SEEDED_DAYS = 80
/** The one local day inside the newest 31 that deliberately carries no row. */
const SILENCED_DAYS_AGO = 5
/** One stored day carries a stake the presentation cannot read, so its cell has
 *  to say Unknown instead of a fabricated zero. */
const UNREADABLE_STAKE_DAYS_AGO = 3
/** The panel's default preset, mirrored from VALIDATOR_TREND_PRESETS. */
const DEFAULT_DAYS = 30
const DEFAULT_LIMIT = 31
/** The presets that ask for one page of 31 days on purpose, so "Load older days"
 *  is a reachable Owner path while more than 31 local days are stored. */
const LONG_DAYS = 90
const LONG_LIMIT = 31
const DAY_MS = 86_400_000
const TREND_QUERY = '/trend?'

// ---------------------------------------------------------- configured calendar

const LOCAL_DATE = new Intl.DateTimeFormat('en-CA', {
  timeZone: TIMEZONE,
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
})

/** The configured local date one instant falls into, as YYYY-MM-DD. */
function localDateOf(instant: Date): string {
  const parts = LOCAL_DATE.formatToParts(instant)
  const part = (type: string): string => parts.find((item) => item.type === type)?.value ?? ''
  return part('year') + '-' + part('month') + '-' + part('day')
}

/** The configured local date N days before an instant. Kathmandu has no daylight
 *  saving, so one local day really is 24 hours. */
function localDaysAgo(daysAgo: number, now: Date): string {
  return localDateOf(new Date(now.getTime() - daysAgo * DAY_MS))
}

/** Shifts a configured local date by whole days (pure calendar arithmetic). */
function shiftLocalDate(localDate: string, days: number): string {
  const [year, month, day] = localDate.split('-').map(Number)
  return new Date(Date.UTC(year, month - 1, day) + days * DAY_MS).toISOString().slice(0, 10)
}

/** The configured month after one month key. */
function nextMonthKey(monthKey: string): string {
  const [year, month] = monthKey.split('-').map(Number)
  if (month === 12) return String(year + 1) + '-01'
  return String(year) + '-' + String(month + 1).padStart(2, '0')
}

/** A canonical second-precision UTC instant, the shape the Server stores. */
function canonicalInstant(instant: Date): string {
  return instant.toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/** The UTC instant a configured local day starts at. The offset is what makes
 *  this a real configured boundary: 2026-02-02T00:00+05:45 is 2026-02-01T18:15:00Z. */
function localDayStart(localDate: string): string {
  return canonicalInstant(new Date(localDate + 'T00:00:00' + TIMEZONE_OFFSET))
}

/** A local wall-clock instant inside one configured local day. */
function localInstant(localDate: string, timeOfDay: string): string {
  return canonicalInstant(new Date(localDate + 'T' + timeOfDay + TIMEZONE_OFFSET))
}

// ------------------------------------------------------------- presentation copy

/** Mirrors formatAmountExact (src/lib/amount.ts): the source digits with a
 *  grouped integer part, and Unknown when the stored value cannot be read. */
function groupedAmount(value: string | null | undefined): string {
  if (typeof value !== 'string' || !/^\d+(\.\d+)?$/.test(value)) return 'Unknown'
  const [integer, fractional = ''] = value.split('.')
  const grouped = integer.replace(/\B(?=(\d{3})+(?!\d))/g, ',')
  return fractional === '' ? grouped : grouped + '.' + fractional
}

/** Mirrors formatObservedAt (src/components/StatusBadge.tsx): a compact UTC
 *  rendering, never a derived freshness verdict. */
function displayedAt(timestamp: string): string {
  return timestamp.slice(0, 19).replace('T', ' ') + ' UTC'
}

/** The DetailList value the panel states for the requested window. */
function requestedText(page: AdminValidatorTrendResponse): string {
  return (
    page.requestedFromLocalDate +
    ' → ' +
    page.requestedToLocalDate +
    ' (' +
    page.requestedDays +
    ' configured local day(s))'
  )
}

/** The DetailList value the panel states for the answered stretch. */
function answeredText(page: AdminValidatorTrendResponse): string {
  return (
    page.answeredFromLocalDate + ' → ' + page.answeredToLocalDate + ' (' + page.expectedDays + ' day(s))'
  )
}

/** The truncation notice the panel states for a paged answer. */
function truncationText(page: AdminValidatorTrendResponse): string {
  return (
    'The window holds more configured local days than this page answered (' +
    page.expectedDays +
    ' days shown). Only the newest part is here, and the next page continues strictly older than ' +
    page.continuation +
    '; no day is answered twice. The days this page does not reach are not counted as silences here.'
  )
}

// ------------------------------------------------------------ SQL seed material

const SNAPSHOT_COLUMNS =
  'snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, ' +
  'provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, ' +
  'reward_rate, delegator_count, epoch, block_count'

/** A single-quoted SQL text literal; no seeded value carries a quote. */
function sqlText(value: string): string {
  return "'" + value + "'"
}

/** One stored day. The values have a deterministic shape so the chart has
 *  something to draw: rank improves and the cumulative counters grow as the days
 *  get newer. */
function snapshotInsert(localDate: string, daysAgo: number, timezone: string): string {
  // Even days carry the Provider's own stamp, odd days only the Server receipt,
  // so the panel has to disclose both a Provider timestamp and the receipt
  // fallback the Server stands in with when the Provider sends none.
  const observedAt = localInstant(localDate, daysAgo % 2 === 0 ? '09:30:00' : '12:45:00')
  const providerTimestamp = daysAgo % 2 === 0 ? observedAt : null
  const receivedAt =
    providerTimestamp === null
      ? observedAt
      : canonicalInstant(new Date(Date.parse(observedAt) + 125_000))
  const stakeAmount =
    daysAgo === UNREADABLE_STAKE_DAYS_AGO ? 'not-a-decimal' : String(1_000_000 + daysAgo * 1_234) + '.000000'
  const values = [
    sqlText('snap-' + timezone.replace('/', '-') + '-' + localDate),
    sqlText(VALIDATOR_ID),
    sqlText(timezone),
    sqlText(localDate),
    sqlText(localDate.slice(0, 7)),
    sqlText(observedAt),
    sqlText(receivedAt),
    providerTimestamp === null ? 'NULL' : sqlText(providerTimestamp),
    sqlText('platscan'),
    sqlText('seed-' + timezone + '-' + localDate),
    String(1 + ((daysAgo * 13) % 40)),
    sqlText(stakeAmount),
    sqlText(String(4_000_000 + (SEEDED_DAYS - daysAgo) * 250) + '.123456'),
    sqlText('12.5'),
    String(180 + ((daysAgo * 7) % 60)),
    String(1_000 + (SEEDED_DAYS - daysAgo)),
    String(100_000 + (SEEDED_DAYS - daysAgo) * 9),
  ]
  return (
    'INSERT INTO validator_daily_snapshots (' + SNAPSHOT_COLUMNS + ') VALUES (' + values.join(', ') + ')'
  )
}

/** The Validator row the automatic discovery has to reuse, 80 stored configured
 *  local days ending yesterday, one deliberate silence inside the newest 31 days,
 *  and one row of that same local date formed in UTC. */
function seedSql(now: Date): string {
  const createdAt = sqlText(canonicalInstant(now))
  const statements = [
    'INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (' +
      [
        sqlText(VALIDATOR_ID),
        sqlText(NETWORK_KEY),
        sqlText(PLATSCAN_CAPTURE_NODE_ID),
        'NULL',
        createdAt,
        createdAt,
      ].join(', ') +
      ')',
  ]
  for (let daysAgo = 1; daysAgo <= SEEDED_DAYS; daysAgo += 1) {
    // Exactly one local day inside the newest 31 carries no row, so the answer
    // has to disclose a real silence rather than a zero.
    if (daysAgo === SILENCED_DAYS_AGO) continue
    statements.push(snapshotInsert(localDaysAgo(daysAgo, now), daysAgo, TIMEZONE))
  }
  // The silenced local date also holds one row formed in UTC: it is disclosed as
  // a foreign row and never merged into the configured calendar, and the
  // Kathmandu day stays a silence.
  statements.push(snapshotInsert(localDaysAgo(SILENCED_DAYS_AGO, now), SILENCED_DAYS_AGO, 'UTC'))
  return statements.join(';\n') + ';\n'
}

// --------------------------------------------------------- Report and HTTP access

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

function fixtureNodeId(): string {
  const report = JSON.parse(readFileSync(REPORT_FIXTURE, 'utf8')) as {
    nodes: { node_id: string }[]
  }
  return report.nodes[0].node_id
}

/** One Report mutator that makes the fixture Node's Validator identity
 *  identified: the Node announces the captured Validator's P2P public key as its
 *  enode and the genesis hash the harness Network is registered with.
 *
 *  The fixture Inventory already declares platon-mainnet for this Node at
 *  revision 1 and this Agent has no accepted Report yet, so it is left alone —
 *  re-declaring an accepted revision with different content is refused as an
 *  Inventory revision conflict. */
function seedValidatorReport(observedAt: string): (report: Record<string, unknown>) => void {
  return (report) => {
    const section = (report.nodes as FixtureNode[])[0]
    report.generated_at = observedAt
    report.report_sequence = 1
    const identity = section.chain.network_identity
    identity.attempted_at = observedAt
    identity.latest_observed_at = observedAt
    identity.latest.genesis_hash = REGISTERED_GENESIS
    const metadata = section.chain.static_metadata
    metadata.attempted_at = observedAt
    metadata.latest_observed_at = observedAt
    metadata.latest.enode = OBSERVED_ENODE
  }
}

/** The window a preset asks for, mirrored from validatorTrendRange
 *  (src/validatorTrend.ts): from = now - days, to = now, canonical seconds. */
function trendQuery(days: number, limit: number, now: Date): string {
  const to = canonicalInstant(now)
  const from = canonicalInstant(new Date(now.getTime() - days * DAY_MS))
  return 'from=' + encodeURIComponent(from) + '&to=' + encodeURIComponent(to) + '&limit=' + String(limit)
}

/** Ask the Server for one trend page and fail loudly when it does not answer. */
async function trendAnswer(server: DisposableServer, query: string): Promise<AdminValidatorTrendResponse> {
  const path = '/api/admin/v1/validators/' + VALIDATOR_ID + '/trend?' + query
  const response = await server.adminGet(path)
  expect(response.status, 'GET ' + path).toBe(200)
  return response.body as AdminValidatorTrendResponse
}

/** Poll until the Server's own refresh loop has stored *today's* configured local
 *  day from the replayed deployment: the loop's first tick is immediate and
 *  refresh_seconds is 5, so this is a matter of seconds. */
async function waitForLiveDay(
  server: DisposableServer,
  replay: PlatscanReplay,
  timeoutMs = 120_000,
): Promise<string> {
  const deadline = Date.now() + timeoutMs
  let last: AdminValidatorTrendResponse | undefined
  for (;;) {
    last = await trendAnswer(server, trendQuery(1, 1, new Date()))
    const point = last.points[0]
    const today = localDateOf(new Date())
    if (
      point !== undefined &&
      point.localDate === today &&
      point.rank === 1 &&
      point.sampleTime === 'receipt'
    ) {
      return today
    }
    if (Date.now() >= deadline) {
      throw new Error(
        'the Server never stored today from the PlatScan replay: ' +
          JSON.stringify({
            stored: last.points.map((item) => item.localDate + ' rank ' + String(item.rank)),
            detailRequests: replay.detailRequests,
            requestedNodeIds: replay.requestedNodeIds,
            rankingPages: replay.rankingPages,
          }),
      )
    }
    await new Promise((resolve) => setTimeout(resolve, 1_000))
  }
}

/** Poll until the Server's own identity discovery has opened the automatic link
 *  between the reporting Node and the seeded Validator. The refresh loop's first
 *  tick precedes the accepted Report, so the link exists only on the pass that
 *  follows it: the live day alone does not prove the association. */
async function waitForAutomaticLink(server: DisposableServer, timeoutMs = 120_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const answer = await trendAnswer(server, trendQuery(DEFAULT_DAYS, DEFAULT_LIMIT, new Date()))
    if (answer.associations.length === 1) return
    if (Date.now() >= deadline) {
      throw new Error(
        'the Server never linked the reporting Node to the seeded Validator: ' +
          JSON.stringify({ associations: answer.associations.length }),
      )
    }
    await new Promise((resolve) => setTimeout(resolve, 1_000))
  }
}

// ------------------------------------------------------------------- panel DOM

/** A DetailList value inside the panel. */
function detailValue(panel: Locator, label: string): Locator {
  return panel.locator('dl > div:has(> dt:text-is("' + label + '")) > dd')
}

/** One stored-day row, located by the local date in its row header. */
function trendRow(panel: Locator, localDate: string): Locator {
  // The row is found in one CSS query: a filter({ has }) locator is re-rooted
  // inside the candidate element, so an absolutely rooted one never matches.
  return panel.locator(
    '[data-slot="validator-trend-points"] tbody tr:has(> th[scope="row"] > span:text-is("' +
      localDate +
      '"))',
  )
}

/** One labelled cell of a stored-day row. */
function rowCell(row: Locator, label: string): Locator {
  return row.locator('td[data-label="' + label + '"]')
}

/** The facts the panel must state at every viewport and theme. */
async function expectPanelStated(page: Page, silencedLocalDate: string): Promise<Locator> {
  const panel = page.locator(PANEL)
  await expect(panel.getByRole('heading', { level: 2, name: 'Daily trend' })).toBeVisible({
    timeout: 30_000,
  })
  await expect(panel.locator('[data-slot="validator-trend-coverage"]')).toContainText(
    'Some configured local days in the answered stretch carry no stored snapshot.',
  )
  await expect(panel.locator('[data-slot="validator-trend-gaps"] li')).toHaveText(
    'No snapshot was stored for ' + silencedLocalDate + '. It is a silence, not a zero.',
  )
  await expect(panel.locator('[data-slot="validator-trend-points"] tbody tr').first()).toBeVisible()
  await expect(panel.locator('[data-slot="validator-trend-months"] tbody tr').first()).toBeVisible()
  return panel
}

test('the Owner reads the real Kathmandu daily trend and it survives a Node purge', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the trend flow runs once per suite')
  test.setTimeout(600_000)

  const now = new Date()
  const silencedLocalDate = localDaysAgo(SILENCED_DAYS_AGO, now)
  const replay = await startPlatscanReplay()
  const server = await startDisposableServer({
    seedSql: seedSql(now),
    validatorProvider: {
      timezone: TIMEZONE,
      deployments: { [NETWORK_KEY]: replay.baseUrl },
      refreshSeconds: 5,
    },
  })
  try {
    // 1. A real Agent Report. The Node's enode carries the captured Validator's
    //    public key, so the Server identifies the Validator and automatic
    //    discovery links it to the seeded validators row.
    const agent = await server.enrollAgent()
    const observedAt = canonicalInstant(new Date())
    await server.submitReport(agent.agentId, agent.credential, seedValidatorReport(observedAt))

    // 2. The Server's own refresh loop writes today's configured local day from
    //    the replayed deployment: page 1 of the captured cohort ranks the captured
    //    node first, and PlatScan carries no Provider timestamp, so the day is
    //    bucketed by the Server receipt.
    const today = await waitForLiveDay(server, replay)
    await waitForAutomaticLink(server)
    expect(replay.detailRequests, 'the Server fetched the Validator detail').toBeGreaterThan(0)
    expect(replay.requestedNodeIds).toContain(PLATSCAN_CAPTURE_NODE_ID)
    expect(replay.rankingPages.slice(0, 5)).toEqual([1, 2, 3, 4, 5])

    // 3. The Server's answer for the window the panel's default preset asks for.
    const answer = await trendAnswer(server, trendQuery(DEFAULT_DAYS, DEFAULT_LIMIT, now))
    expect(answer.validatorId).toBe(VALIDATOR_ID)
    expect(answer.networkKey).toBe(NETWORK_KEY)
    expect(answer.timezone).toBe(TIMEZONE)
    expect(answer.counterSemantics).toBe('cumulative')
    expect(answer.requestedDays).toBe(31)
    expect(answer.expectedDays).toBe(31)
    expect(answer.observedDays).toBe(30)
    expect(answer.missingDays).toBe(1)
    expect(answer.coverage).toBe('partial')
    expect(answer.truncated).toBe(false)
    expect(answer.continuation ?? null).toBeNull()
    expect(answer.clamped).toBe(false)
    expect(answer.answeredFromLocalDate).toBe(answer.requestedFromLocalDate)
    expect(answer.answeredToLocalDate).toBe(today)
    expect(answer.firstObservedLocalDate).toBe(answer.answeredFromLocalDate)
    // The one missing configured day is the seeded silence, listed as a genuine
    // one-day gap rather than smoothed over.
    expect(answer.gaps).toHaveLength(1)
    expect(answer.gaps[0].fromLocalDate).toBe(silencedLocalDate)
    expect(answer.gaps[0].toLocalDate).toBe(silencedLocalDate)
    expect(answer.gaps[0].days).toBe(1)
    // The UTC row of that local date is disclosed, never merged.
    expect(answer.foreignRows).toBe(1)
    expect(answer.foreignTimezones).toEqual(['UTC'])
    expect(answer.points).toHaveLength(30)

    const newest = answer.points[answer.points.length - 1]
    expect(newest.localDate).toBe(today)
    expect(newest.rank).toBe(1)
    expect(newest.sampleTime).toBe('receipt')
    expect(newest.providerTimestamp ?? null).toBeNull()
    expect(newest.delaySeconds ?? null).toBeNull()
    expect(newest.clockSuspect).toBe(false)
    expect(newest.source).toBe('platscan')
    expect(newest.monthKey).toBe(today.slice(0, 7))
    // The configured calendar, not UTC midnight: this local day is 18:15Z to
    // 18:15Z.
    expect(newest.dayStart).toBe(localDayStart(today))
    expect(newest.dayEnd).toBe(localDayStart(shiftLocalDate(today, 1)))
    expect(newest.dayStart.endsWith('T18:15:00Z')).toBe(true)

    // A seeded day carries the Provider's own stamp: sample_at is the stamp, the
    // delay is measured from it, and delay 125s is a real measurement, not zero.
    const providerPoints = answer.points.filter((point) => point.sampleTime === 'provider')
    expect(providerPoints.length).toBeGreaterThan(0)
    const providerPoint = providerPoints[providerPoints.length - 1]
    expect(providerPoint.providerTimestamp).toBe(providerPoint.sampleAt)
    expect(providerPoint.delaySeconds).toBe(125)

    // The configured months the answer touches, with the month boundary mapped
    // into UTC instead of a UTC month start.
    const newestMonth = answer.months[answer.months.length - 1]
    expect(newestMonth.monthKey).toBe(today.slice(0, 7))
    expect(newestMonth.monthStart).toBe(localDayStart(newestMonth.monthKey + '-01'))
    expect(newestMonth.monthEnd).toBe(localDayStart(nextMonthKey(newestMonth.monthKey) + '-01'))
    expect(newestMonth.monthEnd.endsWith('T18:15:00Z')).toBe(true)

    // The discovered association is the seeded Validator linked to the reported
    // Node, and it is still open.
    const nodeId = fixtureNodeId()
    expect(answer.associations).toHaveLength(1)
    const association = answer.associations[0]
    expect(association.nodeId).toBe(nodeId)
    expect(association.origin).toBe('automatic')
    expect(association.validUntil ?? null).toBeNull()
    expect(association.current).toBe(true)
    expect(association.nodeLifecycle.length).toBeGreaterThan(0)
    expect(answer.deletedNodes).toBe(0)
    expect(answer.associationHistoryPartial).toBe(false)

    // 4. The Owner-facing panel, judged against that answer.
    const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, OWNER_PATH)
      const panel = page.locator(PANEL)
      await expect(panel.getByRole('heading', { level: 2, name: 'Daily trend' })).toBeVisible({
        timeout: 30_000,
      })
      // The coverage verdict is a badge and a sentence, never a colour alone.
      await expect(panel.locator('[data-slot="status-badge"]').first()).toHaveText('Partial')
      await expect(detailValue(panel, 'Coverage')).toHaveText('Partial')
      await expect(panel.locator('[data-slot="validator-trend-coverage"]')).toHaveText(
        'Some configured local days in the answered stretch carry no stored snapshot. The days that do are shown; the missing ones are listed as silences, never as zeros.',
      )
      await expect(detailValue(panel, 'Configured timezone')).toHaveText(TIMEZONE)
      await expect(detailValue(panel, 'Requested')).toHaveText(requestedText(answer))
      await expect(detailValue(panel, 'Answered')).toHaveText(answeredText(answer))
      await expect(detailValue(panel, 'Observed / missing')).toHaveText('30 observed · 1 missing')
      await expect(detailValue(panel, 'First / last observed')).toHaveText(
        answer.firstObservedLocalDate + ' / ' + answer.lastObservedLocalDate,
      )
      await expect(detailValue(panel, 'Association history')).toHaveText('Resolvable intervals complete')
      // An answered page that is the whole window states no stretch notice.
      await expect(panel.locator('[data-slot="validator-trend-stretch"]')).toHaveCount(0)
      await expect(panel.locator('[data-slot="validator-trend-truncated"]')).toHaveCount(0)

      // The cumulative counters are named as cumulative, with the sentence that
      // keeps them from reading as period earnings.
      await expect(panel.locator('[data-slot="validator-trend-counters"]')).toHaveText(
        'Reward, block and stake values are the cumulative Provider counters as of each sample. This surface never computes period earnings, net profit, or a re-bucketed series from them.',
      )
      await expect(panel.locator('[data-slot="validator-trend-foreign"]')).toHaveText(
        '1 stored snapshot(s) for this Validator inside this stretch were formed in another configured timezone (UTC). They are disclosed here and are never merged into the configured calendar or quietly re-bucketed.',
      )
      // The gap is a proven silence, in the Operator's own words.
      await expect(panel.locator('[data-slot="validator-trend-gaps"] li')).toHaveCount(1)
      await expect(panel.locator('[data-slot="validator-trend-gaps"] li')).toHaveText(
        'No snapshot was stored for ' + silencedLocalDate + '. It is a silence, not a zero.',
      )
      await expect(panel.locator('[data-slot="validator-trend-no-gaps"]')).toHaveCount(0)

      // The chart draws the stored days and the silence as a band, and says so in
      // its accessible name.
      const chart = panel.locator('[data-slot="validator-trend-chart"] svg[role="img"]')
      await expect(chart).toHaveCount(1)
      const chartLabel = await chart.getAttribute('aria-label')
      expect(chartLabel).toContain('Rank of this Validator over ' + answer.answeredFromLocalDate)
      expect(chartLabel).toContain('in ' + TIMEZONE)
      expect(chartLabel).toContain('30 stored day(s) drawn, 1 configured local day(s) without a snapshot')
      await expect(panel.locator('[data-slot="validator-trend-chart"] figcaption')).toContainText(
        'Shaded bands are silences, and the line breaks across them rather than drawing through.',
      )

      // The stored-days table: the row header carries the real configured day
      // window, and the counters are labelled cumulative.
      const pointsTable = panel.locator('[data-slot="validator-trend-points"]')
      await expect(pointsTable.locator('tbody tr')).toHaveCount(30)
      await expect(pointsTable.locator('thead')).toContainText('Local day (' + TIMEZONE + ')')
      await expect(pointsTable.locator('thead')).toContainText('Reward (cumulative)')
      await expect(pointsTable.locator('thead')).toContainText('Blocks (cumulative)')

      const liveRow = trendRow(panel, today)
      await expect(liveRow).toHaveCount(1)
      await expect(liveRow.locator('th[scope="row"]')).toContainText(
        localDayStart(today) + ' → ' + localDayStart(shiftLocalDate(today, 1)) + ' · 24 hours',
      )
      await expect(liveRow.locator('th[scope="row"]')).toContainText('Month ' + today.slice(0, 7))
      await expect(rowCell(liveRow, 'Rank')).toHaveText('1')
      await expect(rowCell(liveRow, 'Sample time')).toContainText('Server receipt (fallback)')
      await expect(rowCell(liveRow, 'Sample time')).toContainText(displayedAt(newest.sampleAt))
      await expect(rowCell(liveRow, 'Delay')).toHaveText('Unknown')
      await expect(rowCell(liveRow, 'Source')).toContainText('platscan')
      // The replay's detail answer carries no epoch, and an unknown value is
      // named as Unknown, never as a zero or as the literal null.
      await expect(rowCell(liveRow, 'Source')).toContainText(
        'Epoch ' + String(newest.epoch ?? 'Unknown'),
      )

      const providerRow = trendRow(panel, providerPoint.localDate)
      await expect(providerRow).toHaveCount(1)
      await expect(rowCell(providerRow, 'Rank')).toHaveText(String(providerPoint.rank))
      await expect(rowCell(providerRow, 'Delegators')).toHaveText(String(providerPoint.delegatorCount))
      await expect(rowCell(providerRow, 'Reward (cumulative)')).toHaveText(
        groupedAmount(providerPoint.rewardAmount),
      )
      await expect(rowCell(providerRow, 'Blocks (cumulative)')).toHaveText(
        String(providerPoint.blockCount),
      )
      await expect(rowCell(providerRow, 'Sample time')).toContainText('Provider timestamp')
      await expect(rowCell(providerRow, 'Sample time')).toContainText(
        displayedAt(providerPoint.sampleAt),
      )
      await expect(rowCell(providerRow, 'Delay')).toHaveText('2m')

      // A stored value the presentation cannot read is Unknown, never a zero.
      const unreadableRow = trendRow(panel, localDaysAgo(UNREADABLE_STAKE_DAYS_AGO, now))
      await expect(unreadableRow).toHaveCount(1)
      await expect(rowCell(unreadableRow, 'Stake')).toHaveText('Unknown')

      // The configured-months table states the same boundaries the Server sent.
      const monthsTable = panel.locator('[data-slot="validator-trend-months"]')
      await expect(monthsTable.locator('tbody tr')).toHaveCount(answer.months.length)
      const monthRow = monthsTable.locator(
        'tbody tr:has(> th[scope="row"]:text-is("' + newestMonth.monthKey + '"))',
      )
      await expect(monthRow).toHaveCount(1)
      await expect(monthRow.locator('td[data-label="Configured boundary in UTC"]')).toHaveText(
        newestMonth.monthStart + ' → ' + newestMonth.monthEnd,
      )
      await expect(monthRow.locator('td[data-label="Observed days"]')).toHaveText(
        String(newestMonth.observedDays),
      )

      // The Node association list resolves the linked Node with its open interval.
      const associationList = panel.locator('[data-slot="validator-trend-association-list"]')
      await expect(associationList.locator('li')).toHaveCount(1)
      const associationLink = associationList.getByRole('link')
      await expect(associationLink).toHaveAttribute('href', '/admin/nodes/' + nodeId)
      await expect(associationLink).toHaveText(
        (association.nodeDisplayName ?? '').trim() || nodeId,
      )
      const associationItem = associationList.locator('li').first()
      await expect(associationItem).toContainText('automatic · ' + displayedAt(association.validFrom))
      await expect(associationItem).toContainText('open interval')
      await expect(associationItem.locator('[data-slot="status-badge"]')).toHaveText('Open')
      await expect(associationItem).toContainText('Node ' + association.nodeLifecycle)
      await expect(panel.locator('[data-slot="validator-trend-no-associations"]')).toHaveCount(0)
      await expect(panel.locator('[data-slot="validator-trend-associations-notice"]')).toHaveCount(0)

      // 5. Pagination. The 90-day preset keeps only LONG_LIMIT stored days, so the
      //    page is genuinely truncated and the continuation is reachable. The
      //    Server answers the stretch from the newest day down to the oldest row it
      //    kept, so that stretch spans LONG_LIMIT + 1 local days and the one silence
      //    inside it is the only missing day.
      const longAnswer = await trendAnswer(server, trendQuery(LONG_DAYS, LONG_LIMIT, now))
      expect(longAnswer.truncated).toBe(true)
      expect(longAnswer.expectedDays).toBe(LONG_LIMIT + 1)
      expect(longAnswer.observedDays).toBe(LONG_LIMIT)
      expect(longAnswer.missingDays).toBe(1)
      expect(longAnswer.answeredToLocalDate).toBe(today)
      expect(longAnswer.answeredFromLocalDate).toBe(localDaysAgo(LONG_LIMIT, now))
      expect(longAnswer.continuation).toBe(longAnswer.answeredFromLocalDate)
      expect(longAnswer.requestedFromLocalDate).toBe(shiftLocalDate(today, -90))

      await panel.getByRole('button', { name: '90 days' }).click()
      await expect(panel.locator('[data-slot="validator-trend-truncated"]')).toHaveText(
        truncationText(longAnswer),
      )
      await expect(detailValue(panel, 'Answered')).toHaveText(answeredText(longAnswer))
      await expect(panel.locator('[data-slot="validator-trend-stretch"]')).toContainText(
        'This page answers ' +
          longAnswer.answeredFromLocalDate +
          ' to ' +
          longAnswer.answeredToLocalDate +
          ' of the requested ' +
          longAnswer.requestedFromLocalDate +
          ' to ' +
          longAnswer.requestedToLocalDate,
      )
      const loadOlder = panel.getByRole('button', { name: 'Load older days' })
      await expect(loadOlder).toBeEnabled()
      await expect(panel.getByRole('button', { name: 'Return to newest' })).toBeDisabled()

      // Observation only: the panel's own request has to carry the Server's
      // cursor, never a client-side re-slice of a wider answer.
      const trendRequests: string[] = []
      page.on('request', (request) => {
        if (request.url().includes(TREND_QUERY)) trendRequests.push(request.url())
      })
      await loadOlder.click()
      // The next page continues strictly older than the cursor: its local-day
      // stretch ends the day before the cursor date.
      const olderTo = shiftLocalDate(longAnswer.continuation, -1)
      const olderFrom = shiftLocalDate(olderTo, -30)
      await expect(detailValue(panel, 'Answered')).toHaveText(
        olderFrom + ' → ' + olderTo + ' (31 day(s))',
      )
      expect(olderFrom < longAnswer.answeredFromLocalDate).toBe(true)
      await expect
        .poll(() =>
          trendRequests.some(
            (url) => new URL(url).searchParams.get('before') === longAnswer.continuation,
          ),
        )
        .toBe(true)
      expect(trendRequests.length).toBeGreaterThan(0)
      // The stretch that now answers excludes the silence, and the panel says so
      // instead of re-stating the newer page's gap.
      await expect(panel.locator('[data-slot="validator-trend-no-gaps"]')).toHaveText(
        'No configured local day of the answered stretch is missing a snapshot.',
      )
      await expect(panel.locator('[data-slot="validator-trend-gaps"]')).toHaveCount(0)
      // Truncation alone keeps coverage partial: a bounded page never reads as a
      // complete history.
      await expect(panel.locator('[data-slot="status-badge"]').first()).toHaveText('Partial')

      await panel.getByRole('button', { name: 'Return to newest' }).click()
      await expect(detailValue(panel, 'Answered')).toHaveText(answeredText(longAnswer))
      await expect(panel.locator('[data-slot="validator-trend-gaps"] li')).toHaveText(
        'No snapshot was stored for ' + silencedLocalDate + '. It is a silence, not a zero.',
      )
      await expect(panel.locator('[data-slot="validator-trend-chart"] figcaption')).toContainText(
        'configured local day(s) of this stretch hold no snapshot',
      )

      // 6. Story 68: Purge deletes the Node and its association intervals, and the
      //    Validator's own stored days keep answering.
      const purge = await server.adminPost('/api/admin/v1/nodes/' + nodeId + '/purge', {
        confirmNodeId: nodeId,
      })
      expect(purge.status, 'Purge answers 200 for the confirmed Node').toBe(200)

      const purged = await trendAnswer(server, trendQuery(DEFAULT_DAYS, DEFAULT_LIMIT, now))
      expect(purged.deletedNodes).toBe(1)
      expect(purged.associationHistoryPartial).toBe(true)
      expect(purged.associations).toHaveLength(0)
      expect(purged.points).toHaveLength(30)
      expect(purged.observedDays).toBe(30)
      expect(purged.missingDays).toBe(1)
      expect(purged.coverage).toBe('partial')
      expect(purged.gaps).toHaveLength(1)

      await gotoAuthenticated(page, server.baseUrl, OWNER_PATH)
      const purgedPanel = await expectPanelStated(page, silencedLocalDate)
      await expect(detailValue(purgedPanel, 'Association history')).toHaveText(
        'Partial · 1 deleted Node(s)',
      )
      await expect(purgedPanel.locator('[data-slot="validator-trend-associations-notice"]')).toHaveText(
        '1 Node(s) of this Network were deleted by Purge. A purge removes that Node\'s association intervals with the Node, so those intervals are unavailable here rather than never having existed; they are never re-attached to a surviving Node, and this Validator\'s own recorded days above are retained.',
      )
      await expect(purgedPanel.locator('[data-slot="validator-trend-no-associations"]')).toHaveText(
        'No association interval of this Validator still resolves to a Node; the deleted Node intervals above are unavailable rather than never having existed.',
      )
      await expect(purgedPanel.locator('[data-slot="validator-trend-association-list"]')).toHaveCount(0)
      // The retained days still answer, and nothing turns into a healthy zero.
      await expect(purgedPanel.locator('[data-slot="validator-trend-points"] tbody tr')).toHaveCount(30)
      await expect(detailValue(purgedPanel, 'Observed / missing')).toHaveText('30 observed · 1 missing')
      await expect(purgedPanel.locator('[data-slot="status-badge"]').first()).toHaveText('Partial')
      await expect(detailValue(purgedPanel, 'Configured timezone')).toHaveText(TIMEZONE)
      await expect(rowCell(trendRow(purgedPanel, today), 'Rank')).toHaveText('1')
    } finally {
      await context.close()
    }
  } finally {
    await replay.stop()
    await server.dispose()
  }
})

test('the trend panel holds its geometry and disclosure at every viewport and theme', async ({
  browser,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix sets its own viewports')
  test.setTimeout(600_000)

  const now = new Date()
  const silencedLocalDate = localDaysAgo(SILENCED_DAYS_AGO, now)
  // The configured calendar has to be Kathmandu here too: with no
  // [validator_provider] the Server answers on UTC, and the seeded Kathmandu days
  // would be disclosed as foreign rows instead of being the days under test.
  const replay = await startPlatscanReplay()
  const server = await startDisposableServer({
    seedSql: seedSql(now),
    validatorProvider: {
      timezone: TIMEZONE,
      deployments: { [NETWORK_KEY]: replay.baseUrl },
      refreshSeconds: 5,
    },
  })
  try {
    // No Agent Report here: the seeded Validator alone carries the stored days and
    // no Node is ever linked, so the matrix also proves the panel answers for a
    // Validator with no association. Waiting for the live day keeps the answered
    // stretch to the one seeded silence at every viewport.
    await waitForLiveDay(server, replay)
    const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
    const page = await context.newPage()
    try {
      await loginToDisposableServer(page, server.baseUrl)
      const today = localDateOf(new Date())
      for (const viewport of VIEWPORTS) {
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.setViewportSize({ width: viewport.width, height: viewport.height })
          await page.emulateMedia({ colorScheme })
          testInfo.annotations.push({
            type: 'surface',
            description: 'validator daily trend @ ' + viewport.width + 'x' + viewport.height + ' ' + colorScheme,
          })
          await gotoAuthenticated(page, server.baseUrl, OWNER_PATH)
          await expectResolvedTheme(page, colorScheme)
          const panel = await expectPanelStated(page, silencedLocalDate)
          await expect(panel.locator('[data-slot="validator-trend-no-associations"]')).toHaveText(
            'No association interval of this Validator still resolves to a Node.',
          )
          await expectLocalTableScroll(page, 'validator-trend-points')
          await expectNoHorizontalOverflow(page)

          if (viewport.width <= 768) {
            await expectVisibleInteractiveTargets(page)
            // The preset control is reachable by keyboard and by touch, and each
            // narrow pass exercises the real re-request.
            const seven = await focusByKeyboard(page, panel.getByRole('button', { name: '7 days' }))
            await expect(seven).toBeFocused()
            await expectFocusedElementHasVisibleFocus(page)
            await seven.tap()
            await expect(detailValue(panel, 'Requested')).toHaveText(
              shiftLocalDate(today, -7) +
                ' → ' +
                today +
                ' (8 configured local day(s))',
            )
            await expect(detailValue(panel, 'Answered')).toHaveText(
              shiftLocalDate(today, -7) + ' → ' + today + ' (8 day(s))',
            )

            const ninety = await focusByKeyboard(page, panel.getByRole('button', { name: '90 days' }))
            await expect(ninety).toBeFocused()
            await ninety.tap()
            await expect(panel.locator('[data-slot="validator-trend-truncated"]')).toContainText(
              'Only the newest part is here',
            )
            const loadOlder = await focusByKeyboard(
              page,
              panel.getByRole('button', { name: 'Load older days' }),
            )
            const beforeAnswered = (await detailValue(panel, 'Answered').textContent()) ?? ''
            await loadOlder.tap()
            await expect
              .poll(async () => (await detailValue(panel, 'Answered').textContent()) !== beforeAnswered)
              .toBe(true)
            const afterAnswered = (await detailValue(panel, 'Answered').textContent()) ?? ''
            expect(afterAnswered.slice(0, 10) < beforeAnswered.slice(0, 10)).toBe(true)
            await expectNoHorizontalOverflow(page)
          }
          testInfo.annotations.pop()
        }
      }
    } finally {
      await context.close()
    }
  } finally {
    await replay.stop()
    await server.dispose()
  }
})
