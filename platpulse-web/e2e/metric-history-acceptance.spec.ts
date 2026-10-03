import { expect, test, type Page } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import type { AdminNodeMetricHistoryResponse } from '../src/api/generated'
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
 * Node metric history acceptance: the raw 24 hour window (issue #213, design
 * §11.4) and the aggregate tiers behind it (issue #214, design §11.6).
 *
 * Each test owns one throwaway Server, port, and temporary SQLite database and
 * drives the production WebUI build against real Server HTTP: no mock, no
 * test-only seam, and no jsdom. Every observation is stored by submitting a
 * real Agent Report, so the history the Owner reads is the history the Server
 * actually wrote, and the silence in the middle of the window is a real pause
 * the capacity protection recorded — never a fabricated zero or a line drawn
 * across a stretch nobody observed.
 */

/**
 * The largest floor the Server accepts: the thresholds are stored in SQLite, so
 * the ceiling is `i64::MAX`. It stays a string because it sits one past
 * `Number.MAX_SAFE_INTEGER`, and as a JavaScript number it would round up to a
 * floor the Server rejects.
 */
const MAX_PERSISTED_BYTES = '9223372036854775807'

/** A floor every real filesystem clears, so protection stays released. */
const CLEARED_FLOOR = 1

/** The canonical fixture supplies the healthy Node process probe the minimal
 * one disables; without it the report carries no Node metric history at all.
 * The report itself is the minimal fixture, so the Node it declares is the
 * minimal one's: the two fixtures describe different Nodes. */
const CANONICAL_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_canonical.json'
const REPORT_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_minimal.json'

const MINUTE_SECONDS = 60
const HOUR_SECONDS = 3600
const DAY_SECONDS = 24 * HOUR_SECONDS
const FIVE_MINUTES_SECONDS = 300

function enableNodeProcess(report: Record<string, unknown>) {
  const canonical = JSON.parse(readFileSync(CANONICAL_FIXTURE, 'utf8')) as {
    nodes: { process: unknown }[]
  }
  const nodes = report.nodes as Record<string, unknown>[]
  nodes[0].process = canonical.nodes[0].process
}

/** Canonical second-precision instant, seconds before now, matching the
 * Server's own timestamp format. */
function canonicalAgo(seconds: number): string {
  const instant = Math.floor(Date.now() / 1000) * 1000 - seconds * 1000
  return new Date(instant).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/**
 * One genuinely new Report whose Node process observation is stamped at a real
 * instant relative to now. The fixture's own timestamps are fixed calendar
 * values, and an observation stamped there would land outside the one day raw
 * window; the report id and sequence move so the Server stores a new
 * observation instead of answering an exact replay from its receipt.
 */
function reportObservedAt(observed: string, sequence: number) {
  return (report: Record<string, unknown>) => {
    enableNodeProcess(report)
    const nodes = report.nodes as Record<string, unknown>[]
    const component = nodes[0].process as Record<string, unknown>
    component.attempted_at = observed
    component.latest_observed_at = observed
    report.report_sequence = sequence
    report.report_id = '0195f2a1-0091-4091-8091-0000000000' + String(sequence).padStart(2, '0')
  }
}

/** The grain aligned instant closest to (but not after) the requested age, so a
 * seeded stretch lands on the boundaries its own tier buckets by: a five minute
 * tier buckets on five minute boundaries, a one minute tier on minute ones. */
function alignedEpoch(seconds: number, grainSeconds: number): number {
  return Math.floor((Math.floor(Date.now() / 1000) - seconds) / grainSeconds) * grainSeconds
}

function epochInstant(epoch: number): string {
  return new Date(epoch * 1000).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

function alignedAgo(seconds: number, grainSeconds: number): string {
  return epochInstant(alignedEpoch(seconds, grainSeconds))
}

/**
 * A short run of real Reports one grain apart, oldest first. One counted run
 * inside a single five minute bucket proves the bucket keeps a spike: the
 * envelope holds the minimum and the maximum the raw samples once showed.
 */
function grainRun(ageSeconds: number, grainSeconds: number, values: number[]) {
  // Aligned once, then stepped off that boundary: a run of observations one
  // minute apart inside one bucket is what proves the bucket keeps the count,
  // the minimum and the maximum. Re-aligning each offset instead would collapse
  // them onto the same instant, which is one observation delivered three times.
  const base = alignedEpoch(ageSeconds, grainSeconds)
  return values.map((value, index) => ({
    instant: epochInstant(base + index * MINUTE_SECONDS),
    value,
  }))
}

/** The same real Report with its Node process CPU observation set to a chosen
 * value, so a seeded bucket has an envelope with extremes worth keeping. */
function reportWithCpu(observed: string, sequence: number, cpuPercent: number) {
  return (report: Record<string, unknown>) => {
    reportObservedAt(observed, sequence)(report)
    const nodes = report.nodes as Record<string, unknown>[]
    const component = nodes[0].process as Record<string, unknown>
    const latest = component.latest as Record<string, unknown>
    latest.cpu_percent = cpuPercent
  }
}

function fixtureNodeId(): string {
  const report = JSON.parse(readFileSync(REPORT_FIXTURE, 'utf8')) as {
    nodes: { node_id: string }[]
  }
  return report.nodes[0].node_id
}

/**
 * Declare a new low-space floor in the Server's configuration file. The
 * capacity policy is read at startup, so the Server is restarted on the same
 * state directory: a floor no real filesystem clears adopts protection, and a
 * cleared floor releases it. This is the Operator action the issue describes
 * and it never pokes the database behind the running Server's back.
 */
function declareFloor(stateDir: string, floorBytes: number | string) {
  const path = join(stateDir, 'server.toml')
  const lines = readFileSync(path, 'utf8').split('\n')
  const start = lines.indexOf('[capacity]')
  expect(start, 'the harness declares a [capacity] section').toBeGreaterThan(-1)
  let end = start + 1
  while (end < lines.length && !lines[end].startsWith('[')) end += 1
  lines.splice(
    start,
    end - start,
    '[capacity]',
    'enabled = true',
    'pause_below_bytes = ' + floorBytes,
    'resume_above_bytes = ' + floorBytes,
    'sample_interval_seconds = 5',
  )
  writeFileSync(path, lines.join('\n'))
}

/** Read the Owner-only raw history for one series over real HTTP. */
async function readHistory(
  server: DisposableServer,
  nodeId: string,
  metric: string,
  range = '',
): Promise<AdminNodeMetricHistoryResponse> {
  const response = await server.adminGet(
    '/api/admin/v1/nodes/' + nodeId + '/metric-history?metric=' + metric + range,
  )
  expect(response.status).toBe(200)
  return response.body as AdminNodeMetricHistoryResponse
}

/**
 * The plot must never draw a line across a silence the Server reported: every
 * drawn stretch stays entirely on one side of the band. The observations are
 * split at the reported silence before they are folded into columns, and each
 * column sits where its own observations are, so a stretch only has to stay
 * outside the band. The tolerance absorbs the two decimals a drawn path is
 * written with, nothing more.
 */
async function expectNoLineCrossesTheGap(page: Page) {
  const crossings = await page.evaluate(() => {
    const band = document.querySelector('[data-slot="metric-history-gap-band"]')
    if (!band) return -1
    const start = Number(band.getAttribute("x"))
    const width = Number(band.getAttribute("width"))
    const tolerance = 0.005
    const lines = Array.from(document.querySelectorAll('[data-slot="metric-history-line"]'))
    return lines.filter((line) => {
      const path = line.getAttribute("d") ?? ""
      const xs = Array.from(path.matchAll(/[ML] ([0-9.]+) /g)).map((match) => Number(match[1]))
      if (xs.length === 0) return false
      const min = Math.min(...xs)
      const max = Math.max(...xs)
      return max > start + tolerance && min < start + width - tolerance
    }).length
  })
  expect(crossings, "no drawn line may cross the reported silence").toBe(0)
}
test.describe('Raw Node metric history (issue #213)', () => {
  test('reads a stored 24 hour series, reports the protection pause in the middle of it, and never fills the silence in', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the process-heavy flow runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: { pauseBelowBytes: CLEARED_FLOOR, resumeAboveBytes: CLEARED_FLOOR, sampleIntervalSeconds: 5 },
    })
    try {
      const nodeId = fixtureNodeId()
      const context = await browser.newContext()
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        const agent = await server.enrollAgent()

        // One stored observation before the pause. Every instant is captured
        // once and reused, so an assertion never races the clock past a second.
        const observed = {
          beforePause: canonicalAgo(3 * HOUR_SECONDS),
          afterPause: canonicalAgo(20 * MINUTE_SECONDS),
          newest: canonicalAgo(10 * MINUTE_SECONDS),
        }
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(observed.beforePause, 1))

        // The Operator declares a floor this deployment cannot clear. The two
        // Reports submitted under protection are still accepted and still
        // commit their core projection: only optional history is paused, and
        // every skipped observation is recorded as a pause.
        declareFloor(server.stateDir, MAX_PERSISTED_BYTES)
        await server.restart()
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(2 * HOUR_SECONDS), 2))
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(HOUR_SECONDS), 3))

        // The floor clears, and the same series is stored again.
        declareFloor(server.stateDir, CLEARED_FLOOR)
        await server.restart()
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(observed.afterPause, 4))
        await server.submitReport(agent.agentId, agent.credential, reportObservedAt(observed.newest, 5))

        // The raw window the Server answers with is the one the policy
        // declares, and the series carries only what was really stored: the
        // three observations, and the pause in the middle of them.
        const history = await readHistory(server, nodeId, 'process_cpu_percent')
        expect(history.grain).toBe('raw')
        expect(history.aggregateSupported).toBe(true)
        expect(history.historyHorizonDays).toBe(30)
        expect(history.rawRetentionDays).toBe(1)
        expect(history.windowSeconds).toBe(24 * HOUR_SECONDS)
        expect(history.availability).toBeNull()
        expect(history.truncated).toBe(false)
        expect(history.continuation).toBeNull()
        // A window this young is answered by stored samples alone: the tiers are
        // consulted, hold nothing, and are named as the silences they are. Every
        // point is one stored observation, so nothing here reads as a bucket.
        expect(history.segments).toHaveLength(1)
        expect(history.segments[0].grain).toBe('raw')
        expect(history.segments[0].source).toBe('raw')
        expect(history.segments[0].pointCount).toBe(3)
        expect(history.segments[0].truncated).toBe(false)
        for (const item of history.items) {
          expect(item.grain).toBe('raw')
          expect(item.source).toBe('raw')
          expect(item.sampleCount).toBe(1)
          expect(item.minValue).toBe(item.value)
          expect(item.maxValue).toBe(item.value)
          expect(item.lastObservedAt).toBe(item.observedAt)
        }
        expect(history.items.map((item) => item.observedAt)).toEqual([
          observed.beforePause,
          observed.afterPause,
          observed.newest,
        ])
        expect(history.series.observed).toBe(true)
        expect(history.series.observationCount).toBe(3)
        expect(history.series.replayedCount).toBe(0)
        expect(history.series.correctedCount).toBe(0)
        expect(history.series.sampledCount).toBe(3)
        // Coverage is the stretch two stored observations prove — the ten
        // minutes between the two after the pause — never the paused hours.
        expect(history.series.coverageSeconds).toBe(10 * MINUTE_SECONDS)
        expect(history.gaps).toHaveLength(1)
        const pause = history.gaps[0]
        expect(pause.kind).toBe('protection_pause')
        expect(pause.skippedCount).toBe(2)
        expect(pause.reason).toBe('low-space protection paused sample collection')
        expect(pause.from).toBe(observed.beforePause)

        // The timing evidence of a sample belongs to that sample: observed and
        // received are reported apart, and the delay is measured, not assumed.
        for (const item of history.items) {
          expect(item.delaySeconds ?? -1).toBeGreaterThanOrEqual(0)
          expect(item.clockSuspect).toBe(false)
        }

        // A series this Node never reported is named as absent, an unusable
        // series token and range are refused, and an unknown Node never leaks.
        const neverReported = await readHistory(server, nodeId, 'data_directory_percent')
        expect(neverReported.series.observed).toBe(false)
        expect(neverReported.items).toHaveLength(0)
        expect(neverReported.series.observationCount).toBe(0)

        const invalidMetric = await server.adminGet(
          '/api/admin/v1/nodes/' + nodeId + '/metric-history?metric=carrier_pigeons',
        )
        expect(invalidMetric.status).toBe(400)
        expect((invalidMetric.body as { error: { code: string } }).error.code).toBe('invalid_metric')
        const invalidRange = await server.adminGet(
          '/api/admin/v1/nodes/' + nodeId + '/metric-history?metric=process_cpu_percent&from=yesterday',
        )
        expect(invalidRange.status).toBe(400)
        expect((invalidRange.body as { error: { code: string } }).error.code).toBe('invalid_history_range')
        const unknownNode = await server.adminGet(
          '/api/admin/v1/nodes/0195f2a1-00ff-40ff-80ff-0000000000ff/metric-history?metric=process_cpu_percent',
        )
        expect(unknownNode.status).toBe(404)

        // The production WebUI shows the same answer on the Node detail page.
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        await expect(page.getByRole('heading', { level: 2, name: 'Metric history' })).toBeVisible()
        // The samples table names the timing evidence it carries, and a delay
        // never replaces the observation it belongs to.
        await expect(page.getByRole('columnheader', { name: 'Observed' })).toBeVisible()
        await expect(page.getByRole('columnheader', { name: 'Received' })).toBeVisible()
        await expect(page.getByText('3 stored observation(s) since the first one')).toBeVisible()
        await expect(page.getByText(/0 replay\(s\), 0 correction\(s\)/)).toBeVisible()
        await expect(page.getByText('10 minutes of 24 hours', { exact: false })).toBeVisible()
        // The tier list is on screen even when no tier but the raw window was
        // needed, and a stored sample is never drawn as a bucket.
        await expect(page.getByText('Tiers in this answer')).toBeVisible()
        await expect(page.locator('[data-slot=\'metric-history-segments\']')).toContainText(
          'stored samples · 3 points',
        )
        await expect(page.getByText('Investigation horizon')).toBeVisible()
        await expect(page.locator('[data-slot=\'metric-history-buckets\']')).toHaveCount(0)
        await expect(page.locator('[data-slot=\'metric-history-bucket\']')).toHaveCount(0)
        await expect(page.getByRole('button', { name: 'Load older points' })).toHaveCount(0)

        // The pause is drawn as an undrawn band, and the line is broken at it:
        // nothing is plotted inside the silence. The observation before the
        // pause sits alone in its column, so it is drawn as a point with a
        // min/max whisker instead of a segment reaching across the silence, and
        // no drawn line crosses the band.
        const chart = page.locator('[data-slot=\'metric-history-chart\']')
        await expect(chart).toBeVisible()
        await expect(page.locator('[data-slot=\'metric-history-gap-band\']')).toHaveCount(1)
        expect(
          await page.locator('[data-slot=\'metric-history-whisker\']').count(),
        ).toBeGreaterThan(0)
        await expectNoLineCrossesTheGap(page)
        const gaps = page.locator('[data-slot=\'metric-history-gaps\']')
        await expect(gaps.getByText('Protection pause', { exact: true })).toBeVisible()
        await expect(gaps.getByText(/2 observation\(s\) skipped/)).toBeVisible()
        await expect(gaps).toContainText('low-space protection paused sample collection')
        await expect(page.locator('[data-slot=\'metric-history-sample\']')).toHaveCount(3)

        // Narrowing the window drops the pause out of the answered range: the
        // two samples after it remain, and no silence is reported for the
        // stretch the request no longer covers.
        await page.getByRole('group', { name: 'History range' }).getByRole('button', { name: '1 hour' }).click()
        await expect(page.locator('[data-slot=\'metric-history-sample\']')).toHaveCount(2)
        await expect(page.locator('[data-slot=\'metric-history-gaps\']')).toHaveCount(0)

        // A series the Node never reported is named as absent: no chart and no
        // zero line.
        await page.getByLabel('Metric series').selectOption('data_directory_percent')
        await expect(
          page.getByText(/This Node never reported Data directory/),
        ).toBeVisible()
        await expect(page.locator('[data-slot=\'metric-history-chart\']')).toHaveCount(0)
        await expect(page.locator('[data-slot=\'metric-history-samples\']')).toHaveCount(0)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('the metric history surface holds across the fixed viewport and theme matrix while a collection gap is displayed', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: { pauseBelowBytes: CLEARED_FLOOR, resumeAboveBytes: CLEARED_FLOOR, sampleIntervalSeconds: 5 },
    })
    try {
      const nodeId = fixtureNodeId()
      const agent = await server.enrollAgent()
      // Two minutes of real cadence, then a long stretch nobody observed: the
      // Server derives the gap from the series' own cadence, so the matrix does
      // not need a fabricated flag to display one.
      const first = canonicalAgo(23 * HOUR_SECONDS)
      const second = canonicalAgo(23 * HOUR_SECONDS - 2 * MINUTE_SECONDS)
      const third = canonicalAgo(23 * HOUR_SECONDS - 4 * MINUTE_SECONDS)
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(first, 1))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(second, 2))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(third, 3))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(10 * MINUTE_SECONDS), 4))
      await server.submitReport(agent.agentId, agent.credential, reportObservedAt(canonicalAgo(5 * MINUTE_SECONDS), 5))

      const history = await readHistory(server, nodeId, 'process_cpu_percent')
      expect(history.items).toHaveLength(5)
      expect(history.gaps).toHaveLength(1)
      expect(history.gaps[0].kind).toBe('collection_gap')
      expect(history.gaps[0].reason).toBe('no observation was received in this stretch')

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
            await expect(page.getByRole('heading', { level: 2, name: 'Metric history' })).toBeVisible()
            await expectResolvedTheme(page, colorScheme)
            await expect(page.locator('[data-slot=\'metric-history-chart\']')).toBeVisible()
            await expect(page.locator('[data-slot=\'metric-history-gap-band\']')).toHaveCount(1)
            await expectNoLineCrossesTheGap(page)
            await expect(
              page
                .locator('[data-slot=\'metric-history-gaps\']')
                .getByText('Collection gap', { exact: true }),
            ).toBeVisible()
            // The newest observations scroll inside their own region, never the
            // document, and the gap identity never widens the page.
            await expectLocalTableScroll(page, 'metric-history-samples')
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              // The range presets and the series selector are the interactive
              // targets this panel owns: they stay touch sized and reachable by
              // keyboard while the gap is on screen.
              await expectVisibleInteractiveTargets(page)
              const series = await focusByKeyboard(page, page.getByLabel('Metric series'))
              await expect(series).toBeFocused()
              await expectFocusedElementHasVisibleFocus(page)
            }
          }
        }
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})

test.describe('Aggregate Node metric history (issue #214)', () => {
  test('answers the stretches past the raw window with the buckets that counted them, and pages a bounded answer older', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the process-heavy flow runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: { pauseBelowBytes: CLEARED_FLOOR, resumeAboveBytes: CLEARED_FLOOR, sampleIntervalSeconds: 5 },
    })
    try {
      const nodeId = fixtureNodeId()
      const agent = await server.enrollAgent()

      // A representative history, oldest first, that only the tiers can answer:
      // every observation is far older than the one day raw window. The run ten
      // days back is one five minute bucket with a spike inside it; the run
      // three days back is three one minute buckets one minute apart.
      const seeded = [
        ...grainRun(10 * DAY_SECONDS, FIVE_MINUTES_SECONDS, [1, 9, 5]),
        { instant: alignedAgo(6 * DAY_SECONDS, MINUTE_SECONDS), value: 6 },
        ...grainRun(3 * DAY_SECONDS, MINUTE_SECONDS, [2, 8, 4]),
      ]
      const oldInstants = seeded.slice(0, 3).map((observation) => observation.instant)
      const recentInstants = seeded.slice(4).map((observation) => observation.instant)
      let sequence = 0
      for (const observation of seeded) {
        sequence += 1
        await server.submitReport(
          agent.agentId,
          agent.credential,
          reportWithCpu(observation.instant, sequence, observation.value),
        )
      }

      // Ten days back the raw samples are long released: the answer is the five
      // minute bucket that counted them, carrying the extremes the raw samples
      // proved, and its newest reading as the value.
      const fiveMinuteOnly = await readHistory(
        server,
        nodeId,
        'process_cpu_percent',
        '&from=' +
          canonicalAgo(10 * DAY_SECONDS + 2 * HOUR_SECONDS) +
          '&to=' +
          canonicalAgo(10 * DAY_SECONDS - 2 * HOUR_SECONDS),
      )
      expect(fiveMinuteOnly.grain).toBe('5m')
      expect(fiveMinuteOnly.items).toHaveLength(1)
      const bucket = fiveMinuteOnly.items[0]
      expect(bucket.observedAt).toBe(oldInstants[0])
      expect(bucket.grain).toBe('5m')
      expect(bucket.source).toBe('aggregate')
      expect(bucket.sampleCount).toBe(3)
      expect(bucket.minValue).toBe(1)
      expect(bucket.maxValue).toBe(9)
      expect(bucket.value).toBe(5)
      expect(bucket.lastObservedAt).toBe(oldInstants[2])
      expect(fiveMinuteOnly.segments).toHaveLength(1)
      expect(fiveMinuteOnly.segments[0].grain).toBe('5m')
      expect(fiveMinuteOnly.segments[0].source).toBe('aggregate')

      // One bounded answer over the whole horizon. Each stretch is served at its
      // own grain, and not one point beyond the raw window is presented as a
      // stored sample.
      const monthRange =
        '&from=' + canonicalAgo(29 * DAY_SECONDS) + '&to=' + canonicalAgo(0)
      const month = await readHistory(server, nodeId, 'process_cpu_percent', monthRange)
      expect(month.availability).toBeNull()
      expect(month.windowSeconds).toBe(29 * DAY_SECONDS)
      expect(month.grain).toBe('1m')
      expect(month.truncated).toBe(false)
      expect(month.continuation).toBeNull()
      expect(month.items.map((item) => item.observedAt)).toEqual([
        oldInstants[0],
        seeded[3].instant,
        ...recentInstants,
      ])
      expect(month.items.map((item) => item.grain)).toEqual(['5m', '1m', '1m', '1m', '1m'])
      for (const item of month.items) {
        expect(item.source).toBe('aggregate')
        expect(item.sampleCount).toBeGreaterThanOrEqual(1)
        expect(item.minValue).toBeLessThanOrEqual(item.value)
        expect(item.maxValue).toBeGreaterThanOrEqual(item.value)
        // No bucket is filled with an invented zero: every value was observed.
        expect(item.value).toBeGreaterThan(0)
      }
      // The tier that answered each stretch is named, including the raw tier the
      // answer consulted and found nothing in.
      expect(month.segments.map((segment) => segment.grain)).toEqual(['5m', '1m', 'raw'])
      expect(month.segments.map((segment) => segment.source)).toEqual([
        'aggregate',
        'aggregate',
        'raw',
      ])
      expect(month.segments[2].pointCount).toBe(0)
      expect(month.segments.every((segment) => segment.truncated)).toBe(false)
      // Coverage is what a point's own evidence proves, and never a silence: the
      // five minute bucket proves the two minutes between its first and last
      // observation (its max_gap_seconds is far below the threshold that would
      // call its span a hole), and the three one minute buckets prove the two
      // minutes between them. The days between those stretches are gaps.
      expect(month.series.coverageSeconds).toBe(4 * MINUTE_SECONDS)
      expect(month.series.sampledCount).toBe(5)
      expect(month.gaps.map((gap) => gap.kind)).toEqual(['collection_gap', 'collection_gap'])

      // Asking again for a narrow window inside an old bucket never widens it
      // back into stored samples: it is the same bucket, still counted.
      const zoomed = await readHistory(
        server,
        nodeId,
        'process_cpu_percent',
        '&from=' +
          canonicalAgo(3 * DAY_SECONDS + 10 * MINUTE_SECONDS) +
          '&to=' +
          canonicalAgo(3 * DAY_SECONDS - 10 * MINUTE_SECONDS),
      )
      expect(zoomed.grain).toBe('1m')
      expect(zoomed.items.map((item) => item.observedAt)).toEqual(recentInstants)
      expect(
        zoomed.items.every((item) => item.source === 'aggregate' && item.sampleCount === 1),
      ).toBe(true)

      // One bounded answer at a time: the newest tier spends the budget first,
      // the answer stops where the budget ran out, and the next page continues
      // strictly older. Nothing is repeated and nothing is skipped.
      const pages: AdminNodeMetricHistoryResponse[] = []
      let cursor: string | null = null
      for (let attempt = 0; attempt < 8; attempt += 1) {
        const answer = await readHistory(
          server,
          nodeId,
          'process_cpu_percent',
          monthRange + '&limit=1' + (cursor ? '&before=' + cursor : ''),
        )
        pages.push(answer)
        cursor = answer.continuation
        if (!cursor) break
      }
      expect(pages[0].items).toHaveLength(1)
      expect(pages[0].items[0].observedAt).toBe(recentInstants[2])
      expect(pages[0].continuation).toBe(recentInstants[2])
      expect(pages.slice(0, 4).every((answer) => answer.truncated)).toBe(true)
      expect(pages).toHaveLength(5)
      expect(pages[4].truncated).toBe(false)
      expect(pages[4].continuation).toBeNull()
      expect(
        pages.flatMap((answer) => answer.items.map((item) => item.observedAt)),
      ).toEqual(month.items.map((item) => item.observedAt).reverse())

      // The production WebUI says the same thing rather than drawing days of
      // stored samples: the tiers are named, a bucket is drawn as a square, and
      // the table behind the plot never calls a bucket a sample.
      const context = await browser.newContext({ hasTouch: true })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        await expect(page.getByRole('heading', { level: 2, name: 'Metric history' })).toBeVisible()
        await page
          .getByRole('group', { name: 'History range' })
          .getByRole('button', { name: '30 days' })
          .click()

        await expect(page.getByText('Tiers in this answer')).toBeVisible()
        await expect(page.getByText('Investigation horizon')).toBeVisible()
        await expect(
          page.getByText(/30 days · older stretches are answered by buckets/),
        ).toBeVisible()
        await expect(page.locator('[data-slot=\'metric-history-tiers\']')).toContainText(
          'answered by 5 minute buckets and 1 minute buckets',
        )
        const tiers = page.locator('[data-slot=\'metric-history-segments\']')
        await expect(tiers).toContainText('5 minute buckets')
        await expect(tiers).toContainText('1 minute buckets')
        await expect(tiers).toContainText('consulted, holds nothing')

        expect(
          await page.locator('[data-slot=\'metric-history-bucket\']').count(),
        ).toBeGreaterThan(0)
        const legend = page.locator('[data-slot=\'metric-history-buckets\']')
        await expect(legend).toBeVisible()
        await expect(legend).toContainText('a square is a bucket')

        const samples = page.locator('[data-slot=\'metric-history-samples\']')
        await expect(samples).toContainText('5 minute bucket')
        await expect(samples).toContainText('observations over')
        await expectNoHorizontalOverflow(page)

        // The tier surface holds at every geometry the ticket names: the tier
        // list, the bucket markers and the legend stay on screen in either
        // theme, the table scrolls inside its own region rather than the
        // document, and at touch widths the presets stay touch sized and
        // keyboard reachable.
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
            await page
              .getByRole('group', { name: 'History range' })
              .getByRole('button', { name: '30 days' })
              .click()
            await expectResolvedTheme(page, colorScheme)
            await expect(page.locator('[data-slot=\'metric-history-chart\']')).toBeVisible()
            await expect(page.locator('[data-slot=\'metric-history-tiers\']')).toContainText(
              'answered by 5 minute buckets and 1 minute buckets',
            )
            await expect(page.locator('[data-slot=\'metric-history-segments\']')).toContainText(
              '5 minute buckets',
            )
            expect(
              await page.locator('[data-slot=\'metric-history-bucket\']').count(),
            ).toBeGreaterThan(0)
            await expect(page.locator('[data-slot=\'metric-history-buckets\']')).toBeVisible()
            await expectLocalTableScroll(page, 'metric-history-samples')
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              await expectVisibleInteractiveTargets(page)
              const series = await focusByKeyboard(page, page.getByLabel('Metric series'))
              await expect(series).toBeFocused()
              await expectFocusedElementHasVisibleFocus(page)
            }
          }
        }
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
