import { expect, test } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import type { CapacityOverview } from '../src/api/generated'
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
 * Storage capacity and low-space protection acceptance (issue #212, Stories 43
 * to 46 of the capacity and low-space protection closed loop).
 *
 * Each test owns one throwaway Server, port, and temporary SQLite database,
 * declares a real [capacity] policy in that Server's configuration file, and
 * drives the production WebUI build against real Server HTTP: no mock, no
 * test-only seam, and no jsdom. The floor the harness declares is the largest
 * byte count the Server accepts, so the pressure is a real measurement of the
 * state directory rather than a fabricated flag, and no product default is
 * invented anywhere to make the flow pass.
 *
 * The flow is the issue's: optional history pauses, the gap it leaves is
 * visible per series in the WebUI, the same Report still commits its core
 * projection and is still accepted, and releasing the floor resumes history
 * while the record of what was lost survives the recovery.
 */

/**
 * The largest floor the Server accepts: it stores the thresholds in SQLite, so
 * the ceiling is `i64::MAX`. The digits stay a string because that value sits
 * one past `Number.MAX_SAFE_INTEGER`: as a JavaScript number it would round up
 * to a floor the Server rejects.
 */
const MAX_PERSISTED_BYTES = '9223372036854775807'

const PANEL_TITLE = 'Storage capacity and low-space protection'

/** The canonical fixture supplies the healthy probe the minimal one disables. */
const CANONICAL_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_canonical.json'

/**
 * The minimal wire fixture disables its Node process probe, and a disabled
 * probe produces no optional history to skip. Grafting the canonical fixture's
 * healthy process component in is what the Rust acceptance suite does, and it
 * is what makes the recorded gap attributable to a Node series as well as to a
 * host series.
 */
function enableNodeProcess(report: Record<string, unknown>) {
  const canonical = JSON.parse(readFileSync(CANONICAL_FIXTURE, 'utf8')) as {
    nodes: { process: unknown }[]
  }
  const nodes = report.nodes as Record<string, unknown>[]
  nodes[0].process = canonical.nodes[0].process
}

/**
 * A second, genuinely new report: a fresh report id and a higher sequence.
 * Submitting the identical body again would be an exact replay, which the
 * Server answers from the immutable receipt without re-running the writer, and
 * would therefore prove nothing about resumed history.
 */
function nextRound(report: Record<string, unknown>) {
  report.report_sequence = 2
  report.report_id = '0195f2a1-0090-4090-8090-000000000090'
}

/** Read the policy, the live sample, and the recorded gaps over real HTTP. */
async function readCapacity(server: DisposableServer): Promise<CapacityOverview> {
  const response = await server.adminGet('/api/admin/v1/capacity')
  expect(response.status).toBe(200)
  return response.body as CapacityOverview
}

/** The interval the Operator is looking at. */
function activeInterval(
  overview: CapacityOverview,
): CapacityOverview['recentIntervals'][number] {
  const interval = overview.recentIntervals[0]
  expect(interval, 'the Server records the protection interval it opened').toBeDefined()
  return interval
}

/**
 * The operator lowers the declared floor: any real filesystem clears one byte,
 * so the next startup releases protection instead of adopting it. Editing the
 * configuration file and restarting on the same state directory is the
 * operator action the issue describes, and the test never pokes the database
 * behind the running Server's back.
 */
function lowerDeclaredFloor(stateDir: string, floorBytes: number) {
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

/** A policy whose floor no real filesystem clears, on a five second cadence. */
function forcedPressure() {
  return {
    pauseBelowBytes: MAX_PERSISTED_BYTES,
    resumeAboveBytes: MAX_PERSISTED_BYTES,
    sampleIntervalSeconds: 5,
  }
}

test.describe('Storage capacity and low-space protection (issue #212)', () => {
  test('a declared low-space floor pauses optional history with a visible gap and releases it when the floor clears', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the process-heavy flow runs once')
    test.setTimeout(600_000)
    const server = await startDisposableServer({ capacity: forcedPressure() })
    try {
      const context = await browser.newContext()
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)

        // The declared floor is above every real filesystem, so the Server
        // adopts the pressure while it starts and the Operator sees it
        // without touching SQL.
        const underPressure = await readCapacity(server)
        expect(underPressure.enabled).toBe(true)
        expect(underPressure.protected).toBe(true)
        expect(underPressure.sampleIntervalSeconds).toBe(5)
        expect(underPressure.policyOrigin).toContain('server.toml')
        expect(activeInterval(underPressure).startedReason).toBe('low_space')
        expect(activeInterval(underPressure).endedAt).toBeNull()

        await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
        await expect(
          page.getByRole('heading', { level: 1, name: 'Operations', exact: true }),
        ).toBeVisible()
        await expect(page.getByText(PANEL_TITLE, { exact: true })).toBeVisible()
        await expect(page.getByText('Protecting', { exact: true })).toBeVisible()
        // The note names the declared hysteresis instead of claiming the volume
        // is below a floor it is already above: the I64 max floor renders
        // through the shared byte formatter, so both levels read 8388608 TiB.
        await expect(
          page.getByText(
            'Optional history is paused while available space is at or below 8388608 TiB, ' +
              'resumes at 8388608 TiB available, and every skipped sample is recorded below.',
            { exact: true },
          ),
        ).toBeVisible()

        // A Report submitted under pressure is still accepted, and its core
        // projection still commits: protection pauses optional history only.
        const agent = await server.enrollAgent()
        const receipt = await server.submitReport(
          agent.agentId,
          agent.credential,
          enableNodeProcess,
        )
        expect(receipt.receipt.disposition).toBe('accepted')

        // The pause is a recorded gap, counted per series, not a silence.
        await expect
          .poll(
            async () => activeInterval(await readCapacity(server)).skippedSampleCount,
            {
              timeout: 30_000,
              message: 'a paused history records every skipped optional sample',
            },
          )
          .toBeGreaterThan(0)
        const gapped = activeInterval(await readCapacity(server))
        expect(gapped.endedAt).toBeNull()
        expect(
          gapped.skippedSeries.some(
            (series) =>
              series.scopeKind === 'host' &&
              series.scopeKey === agent.agentId &&
              series.metric === 'network_rx_bytes_per_sec',
          ),
        ).toBe(true)
        expect(
          gapped.skippedSeries.some(
            (series) =>
              series.scopeKind === 'node' && series.metric === 'process_cpu_percent',
          ),
        ).toBe(true)

        // The same gap is visible in the production WebUI, per series.
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
        const intervals = page.locator('[data-slot=\'capacity-intervals-table\']')
        await expect(intervals.getByText('Low space', { exact: true })).toBeVisible()
        await expect(intervals.getByText('Still active', { exact: true })).toBeVisible()
        const gap = page.locator('[data-slot=\'capacity-skipped-series\']')
        // Both Agent host series lost samples, so the scope token repeats once
        // per series; naming the metric is what identifies each row.
        await expect(gap.getByText('host:' + agent.agentId, { exact: true })).toHaveCount(2)
        await expect(gap.getByText('network_rx_bytes_per_sec', { exact: true })).toBeVisible()
        await expect(gap.getByText('process_memory_percent', { exact: true })).toBeVisible()

        // The operator declares a floor this deployment clears and restarts on
        // the same state directory. The open interval keeps the thresholds it
        // opened with, so the release is the Server's own measurement.
        lowerDeclaredFloor(server.stateDir, 1)
        await server.restart()

        const released = await readCapacity(server)
        expect(released.protected).toBe(false)
        const closed = activeInterval(released)
        expect(closed.intervalId).toBe(gapped.intervalId)
        expect(closed.endedReason).toBe('resumed')
        expect(closed.endedAt).not.toBeNull()
        expect(closed.resumedAvailableBytes ?? 0).toBeGreaterThan(0)
        // Ending the interval keeps the record of what was lost.
        expect(closed.skippedSampleCount).toBe(gapped.skippedSampleCount)
        expect(closed.skippedSeriesTotal).toBe(gapped.skippedSeriesTotal)

        // Optional history is written again. The discarded report is accepted,
        // so its optional samples reached the writer without failing the
        // transaction, and the skip counter does not move: while the gate was
        // paused the very same report shape moved it, which is the control that
        // makes an unchanged counter evidence rather than silence.
        const resumed = await server.submitReport(
          agent.agentId,
          agent.credential,
          (report) => {
            enableNodeProcess(report)
            nextRound(report)
          },
        )
        expect(resumed.receipt.disposition).toBe('accepted')
        expect(activeInterval(await readCapacity(server)).skippedSampleCount).toBe(
          gapped.skippedSampleCount,
        )

        // The WebUI reports the release with the measurement that ended it.
        await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
        await expect(page.getByText('Monitoring', { exact: true })).toBeVisible()
        // The Ended cell is one sentence: the timestamp, the reason label, and
        // the measurement that ended the interval.
        await expect(
          page.locator('[data-slot=\'capacity-intervals-table\']'),
        ).toContainText('· Resumed at')
        await expect(
          page
            .locator('[data-slot=\'capacity-intervals-table\']')
            .getByText('Still active', { exact: true }),
        ).toHaveCount(0)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('the capacity surface holds across the fixed viewport and theme matrix while the gap is displayed', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
    test.setTimeout(600_000)
    const server = await startDisposableServer({ capacity: forcedPressure() })
    try {
      const agent = await server.enrollAgent()
      await server.submitReport(agent.agentId, agent.credential, enableNodeProcess)
      await expect
        .poll(
          async () => activeInterval(await readCapacity(server)).skippedSampleCount,
          { timeout: 30_000, message: 'the matrix runs against a Server that is pausing history' },
        )
        .toBeGreaterThan(0)

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/operations')
            await expect(
              page.getByRole('heading', { level: 1, name: 'Operations', exact: true }),
            ).toBeVisible()
            await expect(page.getByText(PANEL_TITLE, { exact: true })).toBeVisible()
            await expect(page.getByText('Protecting', { exact: true })).toBeVisible()
            await expectResolvedTheme(page, colorScheme)
            await expect(
              page
                .locator('[data-slot=\'capacity-intervals-table\']')
                .getByText('Low space', { exact: true }),
            ).toBeVisible()
            await expect(
              page
                .locator('[data-slot=\'capacity-skipped-series\']')
                // The Agent's two host series each carry the same scope token,
                // and the first of them is what the viewport must show.
                .getByText('host:' + agent.agentId, { exact: true })
                .first(),
            ).toBeVisible()
            // The interval table scrolls inside its own region, never the
            // document, and the gap identity never widens the page.
            await expectLocalTableScroll(page, 'capacity-intervals-table')
            await expectNoHorizontalOverflow(page)
            if (viewport.width <= 768) {
              // The card is a read-only surface, so the touch and focus
              // guarantees are the ones the hosting page must keep while the
              // gap is on screen.
              await expectVisibleInteractiveTargets(page)
              const statusFilter = await focusByKeyboard(
                page,
                page.getByRole('group', { name: 'Operation filters' }).getByLabel('Status'),
              )
              await expect(statusFilter).toBeFocused()
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
