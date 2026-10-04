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
async function expectNoLineCrossesTheGap(page: Page, surface: string) {
  const crossings = await page.evaluate((name) => {
    const panel = document.querySelector(
      '[data-slot="metric-history-panel"][data-surface="' + name + '"]',
    )
    if (!panel) return -1
    const tolerance = 0.005
    const bands = Array.from(
      panel.querySelectorAll('[data-slot="metric-history-gap-band"]'),
    ).map((band) => {
      const start = Number(band.getAttribute('x'))
      return { start, end: start + Number(band.getAttribute('width')) }
    })
    if (bands.length === 0) return -1
    const lines = Array.from(
      panel.querySelectorAll('[data-slot="metric-history-line"]'),
    )
    return lines
      .flatMap((line) => {
        const path = line.getAttribute('d') ?? ''
        const xs = Array.from(path.matchAll(/[ML] ([0-9.]+) /g)).map((match) =>
          Number(match[1]),
        )
        if (xs.length === 0) return [false]
        const min = Math.min(...xs)
        const max = Math.max(...xs)
        return bands.map(
          (band) => max > band.start + tolerance && min < band.end - tolerance,
        )
      })
      .filter(Boolean).length
  }, surface)
  expect(crossings, 'no drawn line may cross the reported silence').toBe(0)
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
      const panel = processPanel(page)
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
        await expect(panel.getByRole('heading', { level: 2, name: 'Metric history', exact: true })).toBeVisible()
        // The samples table names the timing evidence it carries, and a delay
        // never replaces the observation it belongs to.
        await expect(panel.getByRole('columnheader', { name: 'Observed' })).toBeVisible()
        await expect(panel.getByRole('columnheader', { name: 'Received' })).toBeVisible()
        await expect(panel.getByText('3 stored observation(s) since the first one')).toBeVisible()
        await expect(panel.getByText(/0 replay\(s\), 0 correction\(s\)/)).toBeVisible()
        await expect(panel.getByText('10 minutes of 24 hours', { exact: false })).toBeVisible()
        // The tier list is on screen even when no tier but the raw window was
        // needed, and a stored sample is never drawn as a bucket.
        await expect(panel.getByText('Tiers in this answer')).toBeVisible()
        await expect(panel.locator('[data-slot=\'metric-history-segments\']')).toContainText(
          'stored samples · 3 points',
        )
        await expect(panel.getByText('Investigation horizon')).toBeVisible()
        await expect(panel.locator('[data-slot=\'metric-history-buckets\']')).toHaveCount(0)
        await expect(panel.locator('[data-slot=\'metric-history-bucket\']')).toHaveCount(0)
        await expect(panel.getByRole('button', { name: 'Load older points' })).toHaveCount(0)

        // The pause is drawn as an undrawn band, and the line is broken at it:
        // nothing is plotted inside the silence. The observation before the
        // pause sits alone in its column, so it is drawn as a point with a
        // min/max whisker instead of a segment reaching across the silence, and
        // no drawn line crosses the band.
        const chart = panel.locator('[data-slot=\'metric-history-chart\']')
        await expect(chart).toBeVisible()
        await expect(panel.locator('[data-slot=\'metric-history-gap-band\']')).toHaveCount(1)
        expect(
          await panel.locator('[data-slot=\'metric-history-whisker\']').count(),
        ).toBeGreaterThan(0)
        await expectNoLineCrossesTheGap(page, 'node-process')
        const gaps = panel.locator('[data-slot=\'metric-history-gaps\']')
        await expect(gaps.getByText('Protection pause', { exact: true })).toBeVisible()
        await expect(gaps.getByText(/2 observation\(s\) skipped/)).toBeVisible()
        await expect(gaps).toContainText('low-space protection paused sample collection')
        await expect(panel.locator('[data-slot=\'metric-history-sample\']')).toHaveCount(3)

        // Narrowing the window drops the pause out of the answered range: the
        // two samples after it remain, and no silence is reported for the
        // stretch the request no longer covers.
        await panel.getByRole('group', { name: 'History range' }).getByRole('button', { name: '1 hour' }).click()
        await expect(panel.locator('[data-slot=\'metric-history-sample\']')).toHaveCount(2)
        await expect(panel.locator('[data-slot=\'metric-history-gaps\']')).toHaveCount(0)

        // A series the Node never reported is named as absent: no chart and no
        // zero line.
        await panel.getByLabel('Metric series').selectOption('data_directory_percent')
        await expect(
          panel.getByText(/This Node never reported Data directory/),
        ).toBeVisible()
        await expect(panel.locator('[data-slot=\'metric-history-chart\']')).toHaveCount(0)
        await expect(panel.locator('[data-slot=\'metric-history-samples\']')).toHaveCount(0)
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
      const panel = processPanel(page)
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
            await expect(panel.getByRole('heading', { level: 2, name: 'Metric history', exact: true })).toBeVisible()
            await expectResolvedTheme(page, colorScheme)
            await expect(panel.locator('[data-slot=\'metric-history-chart\']')).toBeVisible()
            await expect(panel.locator('[data-slot=\'metric-history-gap-band\']')).toHaveCount(1)
            await expectNoLineCrossesTheGap(page, 'node-process')
            await expect(
              panel
                .locator('[data-slot=\'metric-history-gaps\']')
                .getByText('Collection gap', { exact: true }),
            ).toBeVisible()
            // The newest observations scroll inside their own region, never the
            // document, and the gap identity never widens the page.
            await expectLocalTableScroll(page, 'metric-history-samples', PROCESS_PANEL)
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              // The range presets and the series selector are the interactive
              // targets this panel owns: they stay touch sized and reachable by
              // keyboard while the gap is on screen.
              await expectVisibleInteractiveTargets(page)
              const series = await focusByKeyboard(page, panel.getByLabel('Metric series'))
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
      const panel = processPanel(page)
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        await expect(panel.getByRole('heading', { level: 2, name: 'Metric history', exact: true })).toBeVisible()
        await panel
          .getByRole('group', { name: 'History range' })
          .getByRole('button', { name: '30 days' })
          .click()

        await expect(panel.getByText('Tiers in this answer')).toBeVisible()
        await expect(panel.getByText('Investigation horizon')).toBeVisible()
        await expect(
          panel.getByText(/30 days · older stretches are answered by buckets/),
        ).toBeVisible()
        await expect(panel.locator('[data-slot=\'metric-history-tiers\']')).toContainText(
          'answered by 5 minute buckets and 1 minute buckets',
        )
        const tiers = panel.locator('[data-slot=\'metric-history-segments\']')
        await expect(tiers).toContainText('5 minute buckets')
        await expect(tiers).toContainText('1 minute buckets')
        await expect(tiers).toContainText('consulted, holds nothing')

        expect(
          await panel.locator('[data-slot=\'metric-history-bucket\']').count(),
        ).toBeGreaterThan(0)
        const legend = panel.locator('[data-slot=\'metric-history-buckets\']')
        await expect(legend).toBeVisible()
        await expect(legend).toContainText('a square is a bucket')

        const samples = panel.locator('[data-slot=\'metric-history-samples\']')
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
            await panel
              .getByRole('group', { name: 'History range' })
              .getByRole('button', { name: '30 days' })
              .click()
            await expectResolvedTheme(page, colorScheme)
            await expect(panel.locator('[data-slot=\'metric-history-chart\']')).toBeVisible()
            await expect(panel.locator('[data-slot=\'metric-history-tiers\']')).toContainText(
              'answered by 5 minute buckets and 1 minute buckets',
            )
            await expect(panel.locator('[data-slot=\'metric-history-segments\']')).toContainText(
              '5 minute buckets',
            )
            expect(
              await panel.locator('[data-slot=\'metric-history-bucket\']').count(),
            ).toBeGreaterThan(0)
            await expect(panel.locator('[data-slot=\'metric-history-buckets\']')).toBeVisible()
            await expectLocalTableScroll(page, 'metric-history-samples', PROCESS_PANEL)
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              await expectVisibleInteractiveTargets(page)
              const series = await focusByKeyboard(page, panel.getByLabel('Metric series'))
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

/**
 * Shared Host resource history acceptance (issue #215, design §11.5 and §11.6,
 * WebUI §15.17): the machine an Agent observes is collected once, stored once,
 * and counted once, so the Agent page and every Node page of that Agent answer
 * the same series, while the Node's own Process series stays a different thing
 * on the same screen. A storage series is named by the mount path the Agent
 * reported and never by a device identity nobody proved, and no single Node's
 * Purge removes the shared evidence.
 *
 * Each test owns one throwaway Server, port, and temporary SQLite database and
 * drives the production WebUI against real Server HTTP: every observation below
 * was written by a real Agent Report, and every number on screen came back out
 * of SQLite.
 */

/** The five Host components one real Report carries. */
const HOST_COMPONENTS = [
  'cpu_percent',
  'memory',
  'load',
  'disk',
  'network_throughput',
]

/** The Host quantities one Report is asked to carry. The Node process probe
 * stays exactly as the minimal fixture leaves it (disabled), so a page showing
 * Host evidence beside an empty Process series is showing two different things
 * rather than two views of one. */
type HostEvidence = {
  cpuPercent?: number
  mounts?: { mountPath: string; totalBytes: number; usedBytes: number }[]
}

/** The identity every Report in this file carries: the Agent that wrote it, the
 * sequence that makes it a new delivery, and the instant it was generated at.
 * An observation is recognised by its own instant, never by this stamp. */
function stampHostReport(
  report: Record<string, unknown>,
  generatedAt: string,
  sequence: number,
) {
  // A Host series belongs to one Agent and the Server identifies a Report by
  // its id, so the id carries the Agent: two Agents that each wrote their
  // first Report are never mistaken for one another.
  const owner = String(report.agent_id).slice(-12)
  report.report_sequence = sequence
  report.report_id =
    '0195f2a1-00' + String(sequence).padStart(2, '0') + '-4091-8091-' + owner
  report.generated_at = generatedAt
}

function hostReportObservedAt(
  observed: string,
  sequence: number,
  evidence: HostEvidence,
) {
  return (report: Record<string, unknown>) => {
    const host = report.host as Record<string, Record<string, unknown>>
    if (evidence.cpuPercent !== undefined) {
      host.cpu_percent.latest = evidence.cpuPercent
    }
    if (evidence.mounts) {
      host.disk.latest = {
        mounts: evidence.mounts.map((mount) => ({
          mount_path: mount.mountPath,
          total_bytes: mount.totalBytes,
          used_bytes: mount.usedBytes,
        })),
      }
    }
    for (const name of HOST_COMPONENTS) {
      host[name].attempted_at = observed
      host[name].latest_observed_at = observed
    }
    stampHostReport(report, observed, sequence)
  }
}

/** One Report about the machine's CPU probe alone, for the case that asks what
 * a probe which answered nothing does to a series. `attemptedAt` is when the
 * probe ran; the CPU reading is either the fresh one it returned, or the last
 * good reading it carried with the instant that reading was really observed at,
 * so a failure never becomes an observation of its own. Every other Host
 * component states that its probe failed and carries nothing, because a reading
 * nobody took is unknown and is never stored as a zero. */
function hostProbeReport(
  attemptedAt: string,
  sequence: number,
  cpu: { value: number; observedAt: string; ok: boolean },
) {
  return (report: Record<string, unknown>) => {
    const host = report.host as Record<string, Record<string, unknown>>
    const failure = { code: 'probe_failed', message: 'the Host probe failed' }
    for (const name of HOST_COMPONENTS) {
      host[name].status = 'error'
      host[name].attempted_at = attemptedAt
      host[name].error = failure
      // A reading nobody took is an omitted field, never a null one: the report
      // contract keeps "omitted" and "null" distinct on the wire.
      delete host[name].latest
      delete host[name].latest_observed_at
    }
    host.cpu_percent.status = cpu.ok ? 'ok' : 'error'
    host.cpu_percent.latest = cpu.value
    host.cpu_percent.latest_observed_at = cpu.observedAt
    delete host.cpu_percent.error
    if (!cpu.ok) host.cpu_percent.error = failure
    stampHostReport(report, attemptedAt, sequence)
  }
}

/** A per-Agent Report writer: one Agent's sequence is its own. */
function hostReporter(
  server: DisposableServer,
  agent: { agentId: string; credential: string },
) {
  let sequence = 0
  return async (observed: string, evidence: HostEvidence) => {
    sequence += 1
    await server.submitReport(
      agent.agentId,
      agent.credential,
      hostReportObservedAt(observed, sequence, evidence),
    )
  }
}

/** An Owner read of one series. A storage series is always read with a mount
 * path, because the Server answers a path it never saw honestly instead of
 * refusing the request. `range` is a raw query fragment, so a case that reads
 * an older stretch keeps saying exactly which stretch it asked for. */
async function readSeries(
  server: DisposableServer,
  path: string,
  metric: string,
  dimension = '',
  range = '',
): Promise<AdminNodeMetricHistoryResponse> {
  const query =
    'metric=' + metric + '&dimension=' + encodeURIComponent(dimension) + range
  const response = await server.adminGet(path + '?' + query)
  expect(response.status, JSON.stringify(response.body)).toBe(200)
  return response.body as AdminNodeMetricHistoryResponse
}

function agentHostHistory(
  server: DisposableServer,
  agentId: string,
  metric: string,
  dimension = '',
  range = '',
) {
  return readSeries(
    server,
    '/api/admin/v1/agents/' + agentId + '/metric-history',
    metric,
    dimension,
    range,
  )
}

function nodeHostHistory(
  server: DisposableServer,
  nodeId: string,
  metric: string,
  dimension = '',
  range = '',
) {
  return readSeries(
    server,
    '/api/admin/v1/nodes/' + nodeId + '/host-metric-history',
    metric,
    dimension,
    range,
  )
}

/** Whether one of the recorded Admin requests asked for `metric` in
 * `dimension`, decoded so a mount path reads as the path rather than as the
 * escapes a URL carries it in. */
function askedForSeries(
  requests: string[],
  metric: string,
  dimension: string,
): boolean {
  return requests
    .map((url) => decodeURIComponent(url))
    .some((url) => url.includes('metric=' + metric) && url.includes('dimension=' + dimension))
}

async function expectRefusal(
  server: DisposableServer,
  path: string,
  status: number,
  code: string,
) {
  const response = await server.adminGet(path)
  expect(response.status, JSON.stringify(response.body)).toBe(status)
  expect((response.body as { error?: { code?: string } }).error?.code).toBe(code)
}

/** One panel of one surface: the Node page reads two series vocabularies, so
 * every locator is the panel's, never the page's. */
function hostPanel(page: Page, surface: string) {
  return page.locator(
    '[data-slot="metric-history-panel"][data-surface="' + surface + '"]',
  )
}

/** The three surfaces that render the shared body, as the selectors the helpers
 * which take a page are pointed at. */
const PROCESS_PANEL =
  '[data-slot="metric-history-panel"][data-surface="node-process"]'
const NODE_HOST_PANEL =
  '[data-slot="metric-history-panel"][data-surface="node-host"]'
const AGENT_HOST_PANEL =
  '[data-slot="metric-history-panel"][data-surface="agent-host"]'

/** The Node's own Process panel. The #213 and #214 cases read that one, and the
 * Node page also answers the shared Host series beside it. */
function processPanel(page: Page) {
  return page.locator(PROCESS_PANEL)
}

test.describe('Shared Host resource history (issue #215)', () => {
  test('stores the machine the Agent observes once and answers it on the Agent page, every Node page, and by the mount path the Agent reported', async ({
    browser,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== 'desktop-1280',
      'the process-heavy flow runs once',
    )
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    })
    try {
      const nodeId = fixtureNodeId()
      const agent = await server.enrollAgent()
      const report = hostReporter(server, agent)
      const mounts = [
        {
          mountPath: '/data',
          totalBytes: 107374182400,
          usedBytes: 42949672960,
        },
        {
          mountPath: '/mnt/archive',
          totalBytes: 2147483648,
          usedBytes: 1073741824,
        },
      ]

      // Three real observations of one machine, two minutes apart, each a whole
      // Report: the Host series is written once for the Agent, as part of
      // accepting the Report, and no route writes a second copy of it.
      await report(canonicalAgo(10 * MINUTE_SECONDS), { cpuPercent: 11, mounts })
      await report(canonicalAgo(8 * MINUTE_SECONDS), { cpuPercent: 2.5, mounts })
      await report(canonicalAgo(6 * MINUTE_SECONDS), { cpuPercent: 9, mounts })

      // The Agent route answers the shared series and names the Agent that owns
      // it; no Node is named because no Node was asked about.
      const fromAgent = await agentHostHistory(server, agent.agentId, 'cpu_percent')
      expect(fromAgent.scopeKind).toBe('host')
      expect(fromAgent.scopeKey).toBe(agent.agentId)
      expect(fromAgent.nodeId).toBeNull()
      expect(fromAgent.dimension).toBe('')
      expect(fromAgent.grain).toBe('raw')
      expect(fromAgent.series.observed).toBe(true)
      expect(fromAgent.items.map((item) => item.value)).toEqual([11, 2.5, 9])
      expect(fromAgent.series.observationCount).toBe(3)
      expect(fromAgent.series.sampledCount).toBe(3)

      // Every Node of that Agent reads the same stored series, and the Node
      // route states which Node asked without claiming the evidence as its own.
      const fromNode = await nodeHostHistory(server, nodeId, 'cpu_percent')
      expect(fromNode.scopeKind).toBe('host')
      expect(fromNode.scopeKey).toBe(agent.agentId)
      expect(fromNode.nodeId).toBe(nodeId)
      expect(fromNode.items.map((item) => item.observedAt)).toEqual(
        fromAgent.items.map((item) => item.observedAt),
      )
      expect(fromNode.items.map((item) => item.value)).toEqual([11, 2.5, 9])
      expect(fromNode.series.observationCount).toBe(3)

      // The Node's own Process series is a different series, untouched by every
      // Host write above: the fixture leaves that probe disabled, so nothing
      // was ever observed for it and nothing is answered as a zero.
      const nodeProcess = await readHistory(server, nodeId, 'process_cpu_percent')
      expect(nodeProcess.scopeKind).toBe('node')
      expect(nodeProcess.scopeKey).toBe(nodeId)
      expect(nodeProcess.series.observed).toBe(false)
      expect(nodeProcess.items).toEqual([])

      // Neither vocabulary answers the other, whichever kind of route asks the
      // other kind's name: a shared Host series is not a Node series and a Node
      // series is not the Agent's machine.
      await expectRefusal(
        server,
        '/api/admin/v1/nodes/' + nodeId + '/host-metric-history?metric=process_cpu_percent',
        400,
        'invalid_metric',
      )
      await expectRefusal(
        server,
        '/api/admin/v1/nodes/' + nodeId + '/metric-history?metric=cpu_percent',
        400,
        'invalid_metric',
      )
      await expectRefusal(
        server,
        '/api/admin/v1/agents/' +
          agent.agentId +
          '/metric-history?metric=process_cpu_percent',
        400,
        'invalid_metric',
      )
      await expectRefusal(
        server,
        '/api/admin/v1/agents/0195f2a1-0009-4009-8009-000000000009/metric-history?metric=cpu_percent',
        404,
        'not_found',
      )

      // Every quantity is stored as it was collected: bytes stay bytes and a
      // load average stays a load average, with no baked percentage invented
      // for the Operator and nothing folded into a single number.
      const memoryUsed = await agentHostHistory(
        server,
        agent.agentId,
        'memory_used_bytes',
      )
      expect(memoryUsed.items.map((item) => item.value)).toEqual([
        4294967296, 4294967296, 4294967296,
      ])
      const memoryTotal = await agentHostHistory(
        server,
        agent.agentId,
        'memory_total_bytes',
      )
      expect(memoryTotal.items.map((item) => item.value)).toEqual([
        17179869184, 17179869184, 17179869184,
      ])
      const load1 = await agentHostHistory(server, agent.agentId, 'load1')
      expect(load1.items.map((item) => item.value)).toEqual([0.4, 0.4, 0.4])
      const load15 = await agentHostHistory(server, agent.agentId, 'load15')
      expect(load15.items.map((item) => item.value)).toEqual([0.3, 0.3, 0.3])
      const networkIn = await agentHostHistory(
        server,
        agent.agentId,
        'network_rx_bytes_per_sec',
      )
      expect(networkIn.items.map((item) => item.value)).toEqual([0, 0, 0])
      const networkOut = await agentHostHistory(
        server,
        agent.agentId,
        'network_tx_bytes_per_sec',
      )
      expect(networkOut.items).toHaveLength(3)

      // Two mounts of one machine are two series, named by the paths the Agent
      // reported. A path nobody reported, and the same series asked for without
      // a path at all, are series nobody ever observed rather than zeros.
      const dataUsed = await agentHostHistory(
        server,
        agent.agentId,
        'disk_used_bytes',
        '/data',
      )
      expect(dataUsed.dimension).toBe('/data')
      expect(dataUsed.series.observed).toBe(true)
      expect(dataUsed.items.map((item) => item.value)).toEqual([
        42949672960, 42949672960, 42949672960,
      ])
      const archiveUsed = await agentHostHistory(
        server,
        agent.agentId,
        'disk_used_bytes',
        '/mnt/archive',
      )
      expect(archiveUsed.dimension).toBe('/mnt/archive')
      expect(archiveUsed.items.map((item) => item.value)).toEqual([
        1073741824, 1073741824, 1073741824,
      ])
      const dataCapacity = await agentHostHistory(
        server,
        agent.agentId,
        'disk_total_bytes',
        '/data',
      )
      expect(dataCapacity.items.map((item) => item.value)).toEqual([
        107374182400, 107374182400, 107374182400,
      ])
      const unnamed = await nodeHostHistory(server, nodeId, 'disk_used_bytes')
      expect(unnamed.dimension).toBe('')
      expect(unnamed.series.observed).toBe(false)
      expect(unnamed.items).toEqual([])
      const unreported = await nodeHostHistory(
        server,
        nodeId,
        'disk_used_bytes',
        '/mnt/other',
      )
      expect(unreported.series.observed).toBe(false)
      expect(unreported.items).toEqual([])
      // A dimension is part of the series key rather than a filter over it, so
      // the machine's own CPU asked for with a mount path is a series nobody
      // observed: an honest empty answer, neither an invented zero nor a
      // refusal of a name the Server does know.
      const cpuWithPath = await agentHostHistory(
        server,
        agent.agentId,
        'cpu_percent',
        '/data',
      )
      expect(cpuWithPath.series.observed).toBe(false)
      expect(cpuWithPath.items).toEqual([])

      const context = await browser.newContext({ hasTouch: true })
      const page = await context.newPage()
      // Every Host series this page asks for, recorded from before the first
      // navigation: a panel that asks for nothing proves it by the requests
      // that were never made.
      const hostSeriesRequests: string[] = []
      page.on('request', (request) => {
        const url = request.url()
        const hostRoute =
          url.includes('/host-metric-history') || url.includes('/api/admin/v1/agents/')
        if (hostRoute && url.includes('metric=')) hostSeriesRequests.push(url)
      })
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)

        // The Node page holds both vocabularies at once: the shared Host series
        // the Agent collected, and the Node's own Process series this Agent
        // never reported. They are separate cards, so neither is read as the
        // other, and the Host card names the Agent whose evidence it is.
        const nodeHost = hostPanel(page, 'node-host')
        await expect(
          nodeHost.getByRole('heading', { level: 2, name: 'Host metric history' }),
        ).toBeVisible()
        const nodeOwner = nodeHost.locator('[data-slot="host-metric-history-owner"]')
        await expect(nodeOwner).toContainText(agent.agentId)
        await expect(nodeOwner).toContainText('does not remove it')
        await expect(nodeHost).toContainText(
          '3 stored observation(s) since the first one',
        )
        await expect(nodeHost.locator('[data-slot="metric-history-chart"]')).toBeVisible()
        const nodeSamples = nodeHost.locator('[data-slot="metric-history-samples"]')
        await expect(nodeSamples).toContainText('11%')
        await expect(nodeSamples).toContainText('2.5%')
        await expect(nodeSamples).toContainText('9.0%')
        await expect(nodeHost.getByText('Process CPU')).toHaveCount(0)

        const nodeProcess = hostPanel(page, 'node-process')
        await expect(nodeProcess.getByText(/never reported Process CPU/)).toBeVisible()
        await expect(nodeProcess.locator('[data-slot="metric-history-chart"]')).toHaveCount(0)

        // A storage series is read once the Operator names the path the Agent
        // reported: the panel asks for the path instead of charting a guess,
        // and then answers each mount path with its own stored series.
        const mountPath = nodeHost.getByLabel('Mount path')
        await nodeHost.getByLabel('Metric series').selectOption('disk_used_bytes')
        await expect(
          nodeHost.getByText(
            'Enter the mount path the Agent reported to read one storage series.',
          ),
        ).toBeVisible()
        await expect(nodeHost.locator('[data-slot="metric-history-chart"]')).toHaveCount(0)
        // The path is asked for rather than guessed: while the name is empty the
        // panel issues not one storage request, so no unnamed series can be
        // charted behind the question. The Host reads that did happen prove the
        // recorder is listening, so the emptiness below is a decision and not a
        // listener that never fired.
        expect(hostSeriesRequests.length).toBeGreaterThan(0)
        expect(
          hostSeriesRequests.filter((url) => url.includes('metric=disk_used_bytes')),
          'no storage series is asked for before a mount path is named',
        ).toEqual([])
        await mountPath.fill('/data')
        // The request that proves the path was asked for leaves after the fill
        // returns, so it is waited for rather than assumed.
        await expect
          .poll(() => askedForSeries(hostSeriesRequests, 'disk_used_bytes', '/data'), {
            message: 'the named mount path is what the panel then asks for',
          })
          .toBe(true)
        await expect(nodeHost.locator('[data-slot="metric-history-chart"]')).toBeVisible()
        await expect(nodeHost.locator('[data-slot="metric-history-samples"]')).toContainText(
          '40.0 GiB',
        )
        await mountPath.fill('/mnt/archive')
        await expect(nodeHost.locator('[data-slot="metric-history-samples"]')).toContainText(
          '1.00 GiB',
        )
        await mountPath.fill('/mnt/other')
        await expect(nodeHost.getByText(/never reported Host storage used/)).toBeVisible()
        await expect(nodeHost.locator('[data-slot="metric-history-chart"]')).toHaveCount(0)

        // The Agent page reads the same stored series under its own name and
        // says whose evidence it is: this Host is the Agent's, not one Node's.
        await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)
        const agentHost = hostPanel(page, 'agent-host')
        await expect(
          agentHost.getByRole('heading', { level: 2, name: 'Host resource history' }),
        ).toBeVisible()
        const agentOwner = agentHost.locator('[data-slot="host-metric-history-owner"]')
        await expect(agentOwner).toContainText(agent.agentId)
        await expect(agentOwner).toContainText('1 retained Node it reports')
        await expect(agentHost).toContainText('3 stored observation(s) since the first one')
        const agentSamples = agentHost.locator('[data-slot="metric-history-samples"]')
        await expect(agentSamples).toContainText('11%')
        await expect(agentSamples).toContainText('9.0%')
        await expect(agentHost.getByText('Process CPU')).toHaveCount(0)
        await agentHost.getByLabel('Metric series').selectOption('disk_total_bytes')
        // The Agent page reads its Host series by the same rule: an unnamed
        // storage series is not asked for at all here either.
        expect(
          hostSeriesRequests.filter((url) => url.includes('metric=disk_total_bytes')),
          'an unnamed storage series is not asked for on the Agent page either',
        ).toEqual([])
        await agentHost.getByLabel('Mount path').fill('/data')
        await expect(agentHost.locator('[data-slot="metric-history-samples"]')).toContainText(
          '100 GiB',
        )
        await expect
          .poll(() => askedForSeries(hostSeriesRequests, 'disk_total_bytes', '/data'), {
            message: 'the Agent page asks for the path the Operator named',
          })
          .toBe(true)

        // The Agent page states the section as its own step of the page, and the
        // vocabulary inside it is the Host's: no Process series is offered here,
        // because a Process series belongs to one Node.
        const section = page.locator('section[aria-label="Host resource history"]')
        await expect(section).toContainText('Host storage capacity')
        await expect(section.getByText('Process CPU')).toHaveCount(0)
      } finally {
        await context.close()
      }

      // One Node's Purge removes that Node's own evidence and nothing else: the
      // shared Host series survives complete, the route that named the deleted
      // Node is honestly gone, and the Agent still answers every observation.
      const purged = await server.adminPost(
        '/api/admin/v1/nodes/' + nodeId + '/purge',
        { confirmNodeId: nodeId },
      )
      expect(purged.status, JSON.stringify(purged.body)).toBe(200)
      const survivors = await agentHostHistory(server, agent.agentId, 'cpu_percent')
      expect(survivors.items.map((item) => item.value)).toEqual([11, 2.5, 9])
      expect(survivors.series.observationCount).toBe(3)
      await expectRefusal(
        server,
        '/api/admin/v1/nodes/' + nodeId + '/host-metric-history?metric=cpu_percent',
        404,
        'not_found',
      )
    } finally {
      await server.dispose()
    }
  })

  test('holds the shared Host surface on both pages that read it across every fixed viewport and theme', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix flow runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    })
    try {
      const nodeId = fixtureNodeId()
      const agent = await server.enrollAgent()
      const report = hostReporter(server, agent)

      // A real cadence and then a real silence: three observations two minutes
      // apart, hours of nothing, then the two newest readings. The Server
      // derives the silence from the series' own cadence, so the drawn line
      // never crosses it.
      const observations = [
        { seconds: 23 * HOUR_SECONDS, value: 11 },
        { seconds: 23 * HOUR_SECONDS - 2 * MINUTE_SECONDS, value: 12 },
        { seconds: 23 * HOUR_SECONDS - 4 * MINUTE_SECONDS, value: 13 },
        { seconds: 10 * MINUTE_SECONDS, value: 14 },
        { seconds: 5 * MINUTE_SECONDS, value: 15 },
      ]
      for (const observation of observations) {
        await report(canonicalAgo(observation.seconds), {
          cpuPercent: observation.value,
        })
      }

      // The silence is reported as a silence, by kind and by its real length,
      // and it is one silence of the shared series both pages read.
      const answer = await agentHostHistory(server, agent.agentId, 'cpu_percent')
      expect(answer.items.map((item) => item.value)).toEqual([11, 12, 13, 14, 15])
      expect(answer.gaps.map((gap) => gap.kind)).toEqual(['collection_gap'])
      expect(answer.gaps[0].seconds).toBeGreaterThan(20 * HOUR_SECONDS)

      const context = await browser.newContext({
        hasTouch: true,
        colorScheme: 'light',
      })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })

            // The Node page: the shared Host series, its silence drawn as a
            // silence, and the Node's own empty Process series beside it.
            await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
            await expectResolvedTheme(page, colorScheme)
            const nodeHost = hostPanel(page, 'node-host')
            await expect(
              nodeHost.getByRole('heading', { level: 2, name: 'Host metric history' }),
            ).toBeVisible()
            await expect(
              nodeHost.locator('[data-slot="host-metric-history-owner"]'),
            ).toContainText(agent.agentId)
            await expect(nodeHost.locator('[data-slot="metric-history-chart"]')).toBeVisible()
            await expect(nodeHost.locator('[data-slot="metric-history-gap-band"]')).toHaveCount(1)
            await expect(nodeHost.locator('[data-slot="metric-history-gaps"]')).toContainText(
              'Collection gap',
            )
            await expect(nodeHost.locator('[data-slot="metric-history-samples"]')).toContainText(
              '15%',
            )
            await expect(nodeHost.getByText(/never reported Host CPU/)).toHaveCount(0)
            await expectNoLineCrossesTheGap(page, 'node-host')
            await expectLocalTableScroll(page, 'metric-history-samples', NODE_HOST_PANEL)
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              await expectVisibleInteractiveTargets(page)
              const series = await focusByKeyboard(page, nodeHost.getByLabel('Metric series'))
              await expect(series).toBeFocused()
              await expectFocusedElementHasVisibleFocus(page)
              // A touch tap narrows the window to six hours, and the panel then
              // answers the six hours it covers instead of leaving the older
              // chart standing.
              await nodeHost.getByRole('button', { name: '6 hours' }).tap()
              const narrowed = nodeHost.locator('[data-slot="metric-history-samples"]')
              await expect(narrowed).toContainText('15%')
              await expect(narrowed).not.toContainText('11%')
              await nodeHost.getByRole('button', { name: '24 hours' }).tap()
              await expect(narrowed).toContainText('11%')
            }

            // The Agent page answers the same shared series in the same theme,
            // and stays inside the viewport while it does.
            await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)
            const agentHost = hostPanel(page, 'agent-host')
            await expect(
              agentHost.getByRole('heading', { level: 2, name: 'Host resource history' }),
            ).toBeVisible()
            await expect(
              agentHost.locator('[data-slot="host-metric-history-owner"]'),
            ).toContainText(agent.agentId)
            await expect(agentHost.locator('[data-slot="metric-history-chart"]')).toBeVisible()
            await expect(agentHost.locator('[data-slot="metric-history-gap-band"]')).toHaveCount(1)
            await expect(agentHost.locator('[data-slot="metric-history-samples"]')).toContainText(
              '11%',
            )
            await expectLocalTableScroll(page, 'metric-history-samples', AGENT_HOST_PANEL)
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              const series = await focusByKeyboard(page, agentHost.getByLabel('Metric series'))
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

  test('carries the last good Host reading through a failed probe and charts no silence it never measured', async ({
    browser,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== 'desktop-1280',
      'the probe-failure flow runs once',
    )
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    })
    try {
      const nodeId = fixtureNodeId()
      const agent = await server.enrollAgent()

      // Three readings of one machine, five minutes apart: that is the cadence
      // the Server derives from this series alone, so five minutes of spacing is
      // this series' own rhythm and never a silence to draw.
      const readings = [
        { age: 20 * MINUTE_SECONDS, cpuPercent: 21 },
        { age: 15 * MINUTE_SECONDS, cpuPercent: 22 },
        { age: 10 * MINUTE_SECONDS, cpuPercent: 23 },
      ]
      let sequence = 0
      for (const reading of readings) {
        sequence += 1
        await server.submitReport(
          agent.agentId,
          agent.credential,
          hostProbeReport(canonicalAgo(reading.age), sequence, {
            value: reading.cpuPercent,
            observedAt: canonicalAgo(reading.age),
            ok: true,
          }),
        )
      }

      // Then the probe fails: it ran five minutes later, the machine answered
      // nothing, and the Agent states the failure while carrying the last good
      // reading with the instant it was really observed at. A carried reading is
      // a delivery, not an observation, so nothing is stored for it and the
      // failure never becomes a value of its own.
      sequence += 1
      await server.submitReport(
        agent.agentId,
        agent.credential,
        hostProbeReport(canonicalAgo(5 * MINUTE_SECONDS), sequence, {
          value: 23,
          observedAt: canonicalAgo(10 * MINUTE_SECONDS),
          ok: false,
        }),
      )

      // The ledger holds the three readings the machine really answered, counts
      // the carried one as a replay, and keeps no point for the failed attempt.
      const cpu = await agentHostHistory(server, agent.agentId, 'cpu_percent')
      expect(cpu.scopeKind).toBe('host')
      expect(cpu.series.observed).toBe(true)
      expect(cpu.items.map((item) => item.value)).toEqual([21, 22, 23])
      expect(cpu.series.observationCount).toBe(3)
      expect(cpu.series.sampledCount).toBe(3)
      expect(cpu.series.replayedCount).toBe(1)
      expect(cpu.series.correctedCount).toBe(0)
      // Five minutes apart is not a silence, and the stretch before the first
      // reading is not one either: a gap is a silence between two observations,
      // never a stretch the series simply has no older evidence for.
      expect(cpu.gaps).toEqual([])

      // The machine's network probe answered nothing on every attempt, so the
      // series was never observed at all: unknown, and unknown is never a zero.
      const network = await agentHostHistory(
        server,
        agent.agentId,
        'network_rx_bytes_per_sec',
      )
      expect(network.series.observed).toBe(false)
      expect(network.series.observationCount).toBe(0)
      expect(network.items).toEqual([])

      // A Node page reads the same shared series: the same three readings, the
      // same carried delivery, and neither attributed to the Node itself.
      const viaNode = await nodeHostHistory(server, nodeId, 'cpu_percent')
      expect(viaNode.scopeKey).toBe(agent.agentId)
      expect(viaNode.items.map((item) => item.value)).toEqual([21, 22, 23])
      expect(viaNode.series.observationCount).toBe(3)
      expect(viaNode.series.replayedCount).toBe(1)

      const context = await browser.newContext({ hasTouch: true })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        const nodeHost = hostPanel(page, 'node-host')
        await expect(
          nodeHost.getByRole('heading', { level: 2, name: 'Host metric history' }),
        ).toBeVisible()
        // The failure is stated as a carried delivery: the ledger still counts
        // the three readings, and the carried one is named as a replay instead of
        // being folded in as if the machine had answered.
        await expect(nodeHost).toContainText('3 stored observation(s) since the first one')
        await expect(nodeHost).toContainText('1 replay(s), 0 correction(s)')
        const samples = nodeHost.locator('[data-slot="metric-history-samples"]')
        await expect(samples).toContainText('21%')
        await expect(samples).toContainText('22%')
        await expect(samples).toContainText('23%')
        // Three rows for three readings: the failed attempt added no row and
        // stands nowhere as a zero.
        await expect(
          nodeHost.locator('[data-slot="metric-history-samples"] tbody tr'),
        ).toHaveCount(3)
        await expect(samples).not.toContainText('0%')
        // The cadence is this series' own, so no band is drawn across it and no
        // silence is claimed for the stretch before the first reading.
        await expect(
          nodeHost.locator('[data-slot="metric-history-gap-band"]'),
        ).toHaveCount(0)
        await expect(nodeHost.locator('[data-slot="metric-history-gaps"]')).toHaveCount(0)

        // A series that never observed says so in the panel's own words, charts
        // nothing and stores nothing, rather than drawing a zero.
        await nodeHost.getByLabel('Metric series').selectOption('network_rx_bytes_per_sec')
        await expect(nodeHost.getByText(/never reported Host network in/)).toBeVisible()
        await expect(nodeHost).toContainText(
          'No observation was ever recorded for this series',
        )
        await expect(nodeHost.locator('[data-slot="metric-history-chart"]')).toHaveCount(0)
        await expect(
          nodeHost.locator('[data-slot="metric-history-samples"]'),
        ).toHaveCount(0)

        // The Agent page states the same shared evidence, because it is the same
        // series read from one more page rather than a second copy of it.
        await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)
        const agentHost = hostPanel(page, 'agent-host')
        await expect(
          agentHost.getByRole('heading', { level: 2, name: 'Host resource history' }),
        ).toBeVisible()
        await expect(agentHost).toContainText('3 stored observation(s) since the first one')
        await expect(agentHost).toContainText('1 replay(s), 0 correction(s)')
        await expect(
          agentHost.locator('[data-slot="metric-history-samples"]'),
        ).toContainText('23%')
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('answers an older Host stretch with the tiers that hold it, on the Node page and on the Agent page', async ({
    browser,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== 'desktop-1280',
      'the Host tier flow runs once; the Node Process case already holds the tier surface at every viewport',
    )
    test.setTimeout(600_000)
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    })
    try {
      const nodeId = fixtureNodeId()
      const agent = await server.enrollAgent()
      const report = hostReporter(server, agent)

      // The shape the Node Process case seeds, written by the Host probe instead:
      // three readings inside one old five minute bucket, one reading six days
      // back, and three readings three days back. Ten days back the raw samples
      // are released, so the five minute tier is the only evidence left of it.
      const old = grainRun(10 * DAY_SECONDS, FIVE_MINUTES_SECONDS, [11, 19, 15])
      const middle = {
        instant: alignedAgo(6 * DAY_SECONDS, MINUTE_SECONDS),
        value: 16,
      }
      const recent = grainRun(3 * DAY_SECONDS, MINUTE_SECONDS, [12, 18, 14])
      for (const observation of [...old, middle, ...recent]) {
        await report(observation.instant, { cpuPercent: observation.value })
      }

      // The Host series is served at two grains by the same engine that serves
      // the Node series: the old stretch from the five minute tier, the rest from
      // one minute buckets, and the raw tier consulted and found empty.
      const monthRange =
        '&from=' + canonicalAgo(29 * DAY_SECONDS) + '&to=' + canonicalAgo(0)
      const month = await nodeHostHistory(
        server,
        nodeId,
        'cpu_percent',
        '',
        monthRange,
      )
      expect(month.scopeKind).toBe('host')
      expect(month.availability).toBeNull()
      expect(month.windowSeconds).toBe(29 * DAY_SECONDS)
      expect(month.grain).toBe('1m')
      expect(month.items.map((item) => item.value)).toEqual([15, 16, 12, 18, 14])
      expect(month.items.map((item) => item.grain)).toEqual(['5m', '1m', '1m', '1m', '1m'])
      expect(month.segments.map((segment) => segment.grain)).toEqual(['5m', '1m', 'raw'])
      expect(month.segments.map((segment) => segment.source)).toEqual([
        'aggregate',
        'aggregate',
        'raw',
      ])
      expect(month.segments[2].pointCount).toBe(0)
      expect(month.segments.every((segment) => segment.truncated)).toBe(false)
      // The old bucket kept the extremes its raw samples proved, so a spike
      // inside the bucket is not lost by the coarser grain.
      expect(month.items[0].sampleCount).toBe(3)
      expect(month.items[0].minValue).toBe(11)
      expect(month.items[0].maxValue).toBe(19)
      expect(month.items[0].lastObservedAt).toBe(old[2].instant)
      expect(month.series.sampledCount).toBe(5)
      expect(month.series.coverageSeconds).toBe(4 * MINUTE_SECONDS)
      expect(month.gaps.map((gap) => gap.kind)).toEqual([
        'collection_gap',
        'collection_gap',
      ])

      // The seven day preset is answered by one minute buckets alone: the ten day
      // old bucket lies outside the range it asked for.
      const week = await nodeHostHistory(
        server,
        nodeId,
        'cpu_percent',
        '',
        '&from=' + canonicalAgo(7 * DAY_SECONDS) + '&to=' + canonicalAgo(0),
      )
      expect(week.grain).toBe('1m')
      expect(week.items.map((item) => item.value)).toEqual([16, 12, 18, 14])
      expect(week.items.every((item) => item.grain === '1m')).toBe(true)
      expect(week.items.every((item) => item.source === 'aggregate')).toBe(true)

      const context = await browser.newContext({ hasTouch: true })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        const nodeHost = hostPanel(page, 'node-host')
        // The default window is the last day and the newest Host reading is three
        // days old: the panel says the series was observed and that nothing in
        // this window holds it, rather than drawing an empty axis as a zero.
        await expect(
          nodeHost.getByText(/no stored sample falls inside this window/),
        ).toBeVisible()
        await nodeHost
          .getByRole('group', { name: 'History range' })
          .getByRole('button', { name: '30 days' })
          .click()
        await expect(
          nodeHost.locator('[data-slot="metric-history-tiers"]'),
        ).toContainText('answered by 5 minute buckets and 1 minute buckets')
        const segments = nodeHost.locator('[data-slot="metric-history-segments"]')
        await expect(segments).toContainText('5 minute buckets')
        await expect(segments).toContainText('1 minute buckets')
        await expect(segments).toContainText('consulted, holds nothing')
        expect(
          await nodeHost.locator('[data-slot="metric-history-bucket"]').count(),
        ).toBeGreaterThan(0)
        await expect(
          nodeHost.locator('[data-slot="metric-history-buckets"]'),
        ).toContainText('a square is a bucket')
        const samples = nodeHost.locator('[data-slot="metric-history-samples"]')
        await expect(samples).toContainText('5 minute bucket')
        await expect(samples).toContainText('15%')
        await expectNoLineCrossesTheGap(page, 'node-host')
        await expectLocalTableScroll(page, 'metric-history-samples', NODE_HOST_PANEL)
        await expectNoHorizontalOverflow(page)

        // The Agent page answers the same stretch with the same tiers: one Host
        // series read from two pages. A touch width in the other theme is enough
        // here, because the tier surface itself is held at every viewport by the
        // Node Process case above.
        await page.setViewportSize({ width: 360, height: 800 })
        await page.emulateMedia({ colorScheme: 'dark' })
        await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)
        const agentHost = hostPanel(page, 'agent-host')
        await agentHost
          .getByRole('group', { name: 'History range' })
          .getByRole('button', { name: '30 days' })
          .click()
        await expectResolvedTheme(page, 'dark')
        await expect(
          agentHost.locator('[data-slot="metric-history-tiers"]'),
        ).toContainText('answered by 5 minute buckets and 1 minute buckets')
        await expect(
          agentHost.locator('[data-slot="metric-history-samples"]'),
        ).toContainText('5 minute bucket')
        await expectLocalTableScroll(page, 'metric-history-samples', AGENT_HOST_PANEL)
        await expectNoHorizontalOverflow(page)
        await expectVisibleInteractiveTargets(page)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
