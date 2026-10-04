import { expect, test, type Page } from '@playwright/test'

import type { AdminAgentStorageMountsResponse } from '../src/api/generated'
import {
  VIEWPORTS,
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
 * Storage by mount path (issue #216, design §11.6). Each test owns one throwaway
 * Server, port, and temporary SQLite database and drives the production WebUI
 * against real Server HTTP: every mount path on screen was written by a real
 * Agent Report, and the list is read back out of SQLite.
 *
 * The card's three claims are what these tests hold it to: a storage series is
 * named by the mount path and by nothing else (so the paths that hold evidence
 * are the paths the Server answers, and a path is never presented as a device),
 * a path is called silent only against a cadence the Server actually measured
 * (a slow cadence or an unmeasurable one is never read as a stopped path), and a
 * list that leaves older paths out says so instead of hiding them.
 */

/** A floor every real filesystem clears, so protection stays released while
 * history collection is genuinely engaged. */
const CLEARED_FLOOR = 1

const MINUTE_SECONDS = 60

/** The five Host components one real Report carries. */
const HOST_COMPONENTS = ['cpu_percent', 'memory', 'load', 'disk', 'network_throughput']

const STORAGE_MOUNTS_PANEL = '[data-slot="storage-mounts-panel"]'
const STORAGE_MOUNT_ROW = '[data-slot="storage-mount-row"]'
const AGENT_HOST_PANEL = '[data-slot="metric-history-panel"][data-surface="agent-host"]'

type HostEvidence = {
  cpuPercent?: number
  mounts?: { mountPath: string; totalBytes: number; usedBytes: number }[]
}

/** Canonical second-precision instant, seconds before now. */
function canonicalAgo(seconds: number): string {
  const instant = Math.floor(Date.now() / 1000) * 1000 - seconds * 1000
  return new Date(instant).toISOString().replace(/\.\d{3}Z$/, 'Z')
}

/** The identity every Report in this file carries. An observation is recognised
 * by its own instant, never by this stamp. */
function stampHostReport(
  report: Record<string, unknown>,
  generatedAt: string,
  sequence: number,
) {
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

/** The Owner's read of the mount list itself. */
async function readMounts(
  server: DisposableServer,
  agentId: string,
): Promise<AdminAgentStorageMountsResponse> {
  const response = await server.adminGet(
    '/api/admin/v1/agents/' + agentId + '/storage-mounts',
  )
  expect(response.status, JSON.stringify(response.body)).toBe(200)
  return response.body as AdminAgentStorageMountsResponse
}

/** Whether one of the recorded Admin requests asked for `metric` in
 * `dimension`, decoded so a mount path reads as the path itself. */
function askedForSeries(
  requests: string[],
  metric: string,
  dimension: string,
): boolean {
  return requests
    .map((url) => decodeURIComponent(url))
    .some(
      (url) => url.includes('metric=' + metric) && url.includes('dimension=' + dimension),
    )
}

/** The Agent page's mount rows, in the order the Server answered them. */
function mountRows(page: Page) {
  return page.locator(STORAGE_MOUNTS_PANEL).locator(STORAGE_MOUNT_ROW)
}

test.describe('Storage by mount path (issue #216)', () => {
  test('answers every mount path the stored evidence holds, in the state its own cadence supports, and reads one on demand', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the flow runs once')
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    })
    try {
      const agent = await server.enrollAgent()
      const report = hostReporter(server, agent)

      // Five Reports five minutes apart: a cadence the Server can measure. One
      // path is reported by every one of them, and one stopped being reported
      // three cadences ago — which is a silence, not a deleted series.
      const observations = [
        { seconds: 30 * MINUTE_SECONDS, used: 100, archive: true, cpu: 11 },
        { seconds: 25 * MINUTE_SECONDS, used: 200, archive: true, cpu: 12 },
        { seconds: 20 * MINUTE_SECONDS, used: 300, archive: true, cpu: 13 },
        { seconds: 15 * MINUTE_SECONDS, used: 400, archive: false, cpu: 14 },
        { seconds: 10 * MINUTE_SECONDS, used: 512, archive: false, cpu: 15 },
      ]
      for (const observation of observations) {
        const mounts = [
          { mountPath: '/data', totalBytes: 1024, usedBytes: observation.used },
        ]
        if (observation.archive) {
          mounts.push({
            mountPath: '/mnt/archive',
            totalBytes: 4096,
            usedBytes: 2048,
          })
        }
        await report(canonicalAgo(observation.seconds), {
          cpuPercent: observation.cpu,
          mounts,
        })
      }

      // The Server's own answer: one row per path, newest path first, each path's
      // state derived from the cadence the stored observations support.
      const answer = await readMounts(server, agent.agentId)
      expect(answer.cadenceSeconds).toBe(5 * MINUTE_SECONDS)
      expect(answer.silenceThresholdSeconds).toBe(15 * MINUTE_SECONDS)
      expect(answer.usedMetric).toBe('disk_used_bytes')
      expect(answer.capacityMetric).toBe('disk_total_bytes')
      expect(answer.mountLimit).toBe(256)
      expect(answer.truncated).toBe(false)
      expect(answer.mounts.map((mount) => mount.mountPath)).toEqual([
        '/data',
        '/mnt/archive',
      ])
      expect(answer.mounts.map((mount) => mount.observationState)).toEqual([
        'reported',
        'silent',
      ])
      expect(answer.mounts[0].used.observationCount).toBe(5)
      expect(answer.mounts[0].used.latestValue).toBe(512)
      expect(answer.mounts[0].capacity.latestValue).toBe(1024)
      // The path that stopped is still a series with its readings and its own
      // count: stopping is not deletion.
      expect(answer.mounts[1].used.observationCount).toBe(3)
      expect(answer.mounts[1].used.latestValue).toBe(2048)
      expect(answer.mounts[1].silentSeconds).toBeGreaterThanOrEqual(1190)
      expect(answer.mounts[1].silentSeconds).toBeLessThan(1300)
      // A mount is a path and two series. Nothing here names a device, because
      // the Server never proved one.
      expect(Object.keys(answer.mounts[0]).sort()).toEqual([
        'capacity',
        'mountPath',
        'observationState',
        'silentSeconds',
        'used',
      ])

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      const requests: string[] = []
      page.on('request', (request) => requests.push(request.url()))
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)

        // 06: the mount list sits with the Host history it names paths for.
        await expect(
          page.getByRole('heading', { level: 2, name: 'Storage by mount path' }),
        ).toBeVisible()
        const list = page.getByRole('region', { name: 'Storage mount paths' })
        await expect(list).toBeVisible()
        const rows = mountRows(page)
        await expect(rows).toHaveCount(2)
        await expect(rows.nth(0)).toHaveAttribute('data-mount-path', '/data')
        await expect(rows.nth(1)).toHaveAttribute('data-mount-path', '/mnt/archive')
        await expect(rows.nth(0)).toHaveAttribute('data-mount-state', 'reported')
        await expect(rows.nth(1)).toHaveAttribute('data-mount-state', 'silent')

        // The list says where it came from: evidence, never a device identity.
        await expect(page.locator(STORAGE_MOUNTS_PANEL)).toContainText(
          'rather than from a device identity',
        )
        await expect(page.locator('[data-slot="storage-mounts-cadence"]')).toContainText(
          'every 5 minutes',
        )
        await expect(page.locator('[data-slot="storage-mounts-silence"]')).toContainText(
          ': three cadences of at most five minutes, and never less than two minutes.',
        )
        // Truncation is not claimed when nothing was left out.
        await expect(page.locator('[data-slot="storage-mounts-truncation"]')).toHaveCount(0)

        // A reported path: current, with its newest reading as a share of capacity.
        await expect(rows.nth(0)).toContainText('Current')
        await expect(rows.nth(0)).toContainText('512 B of 1.00 KiB · 50%')
        await expect(rows.nth(0)).toContainText('5 observations recorded')

        // A path that stopped: silent against the measured cadence only, with the
        // readings it already stored still on its series.
        await expect(rows.nth(1)).toContainText('Stale')
        await expect(rows.nth(1)).toContainText(
          'longer than the 15 minutes a silence is judged against on this Host',
        )
        await expect(rows.nth(1)).toContainText('The readings already stored stay on the series.')
        await expect(rows.nth(1)).toContainText('3 observations recorded')

        // Reading a path asks the Server for that exact path's series: the
        // Operator picks the path here instead of typing it from memory.
        expect(askedForSeries(requests, 'disk_used_bytes', '/data')).toBe(false)
        await rows.nth(0).getByRole('button', { name: 'Read the storage series of /data' }).click()
        const hostPanel = page.locator(AGENT_HOST_PANEL)
        await expect(hostPanel.getByLabel('Metric series')).toHaveValue('disk_used_bytes')
        await expect(hostPanel.getByLabel('Mount path')).toHaveValue('/data')
        await expect
          .poll(() => askedForSeries(requests, 'disk_used_bytes', '/data'), {
            message: 'the chosen mount path is the series the panel reads',
          })
          .toBe(true)
        await expect(hostPanel.locator('[data-slot="metric-history-chart"]')).toBeVisible()
        await expect(hostPanel.locator('[data-slot="metric-history-samples"]')).toContainText(
          '512 B',
        )

        // The path the Agent stopped reporting is reachable exactly when it
        // matters: its stored readings are still there to read.
        await rows.nth(1).getByRole('button', { name: 'Read the storage series of /mnt/archive' }).click()
        await expect(hostPanel.getByLabel('Mount path')).toHaveValue('/mnt/archive')
        await expect
          .poll(() => askedForSeries(requests, 'disk_used_bytes', '/mnt/archive'), {
            message: 'a silent path is still readable',
          })
          .toBe(true)
        await expect(hostPanel.locator('[data-slot="metric-history-samples"]')).toContainText(
          '2.00 KiB',
        )
        // The list never asks for a series through a Node: it is the Agent's Host.
        expect(requests.some((url) => url.includes('/nodes/') && url.includes('metric='))).toBe(
          false,
        )
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('answers a path whose cadence it could not measure as unknown, never as a stopped path', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the flow runs once')
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    })
    try {
      const agent = await server.enrollAgent()
      const report = hostReporter(server, agent)
      // One Report is one observation: too few for the Server to measure a
      // cadence, so no silence may be declared for any path of this Agent.
      await report(canonicalAgo(10 * MINUTE_SECONDS), {
        cpuPercent: 11,
        mounts: [{ mountPath: '/data', totalBytes: 1024, usedBytes: 512 }],
      })

      const answer = await readMounts(server, agent.agentId)
      expect(answer.cadenceSeconds).toBe(0)
      expect(answer.silenceThresholdSeconds).toBe(0)
      expect(answer.mounts[0].observationState).toBe('unknown')
      // The age of the newest reading is real, and it is not called a silence.
      expect(answer.mounts[0].silentSeconds).toBeNull()

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)
        const rows = mountRows(page)
        await expect(rows).toHaveCount(1)
        await expect(rows.nth(0)).toHaveAttribute('data-mount-state', 'unknown')
        await expect(rows.nth(0)).toContainText('Unknown')
        await expect(rows.nth(0)).not.toContainText('Stale')
        await expect(rows.nth(0)).toContainText('neither reported nor silent')
        await expect(page.locator('[data-slot="storage-mounts-cadence"]')).toContainText(
          'could not measure a cadence',
        )
        await expect(page.locator('[data-slot="storage-mounts-cadence"]')).toContainText(
          'no silence can be declared',
        )
        await expect(page.locator('[data-slot="storage-mounts-silence"]')).toContainText(
          'Silence needs a measured cadence to compare against',
        )
        // The reading is still stored and shown: unknown state is not lost data.
        await expect(rows.nth(0)).toContainText('1 observation recorded')
        await expect(rows.nth(0)).toContainText('512 B')
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('states the paths the list left out instead of dropping older mounts silently', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the flow runs once')
    test.setTimeout(300_000)
    const server = await startDisposableServer({
      capacity: {
        pauseBelowBytes: CLEARED_FLOOR,
        resumeAboveBytes: CLEARED_FLOOR,
        sampleIntervalSeconds: 5,
      },
    })
    try {
      const agent = await server.enrollAgent()
      const report = hostReporter(server, agent)
      // One Report carries at most 128 mounts, so three Reports hold more paths
      // than the list carries: 300 paths against a limit of 256.
      const group = (prefix: string) =>
        Array.from({ length: 100 }, (_, index) => ({
          mountPath: '/' + prefix + '-' + String(index).padStart(3, '0'),
          totalBytes: 4096,
          usedBytes: 1024,
        }))
      await report(canonicalAgo(30 * MINUTE_SECONDS), { cpuPercent: 11, mounts: group('old') })
      await report(canonicalAgo(20 * MINUTE_SECONDS), { cpuPercent: 12, mounts: group('mid') })
      await report(canonicalAgo(10 * MINUTE_SECONDS), { cpuPercent: 13, mounts: group('new') })

      const answer = await readMounts(server, agent.agentId)
      expect(answer.mountLimit).toBe(256)
      expect(answer.truncated).toBe(true)
      expect(answer.mounts).toHaveLength(256)
      // The newest paths are answered and the oldest are the ones left out.
      expect(answer.mounts[0].mountPath).toBe('/new-000')
      expect(answer.mounts[199].mountPath).toBe('/mid-099')
      expect(answer.mounts[200].mountPath).toBe('/old-000')
      expect(answer.mounts[255].mountPath).toBe('/old-055')

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)
        const rows = mountRows(page)
        await expect(rows).toHaveCount(256)
        await expect(rows.nth(0)).toHaveAttribute('data-mount-path', '/new-000')
        await expect(rows.nth(255)).toHaveAttribute('data-mount-path', '/old-055')
        // The paths that are not in the answer are stated, not hidden.
        const truncation = page.locator('[data-slot="storage-mounts-truncation"]')
        await expect(truncation).toBeVisible()
        await expect(truncation).toContainText('holds more mount paths than the list carries')
        await expect(truncation).toContainText('oldest of them are not in this answer')
        await expect(page.locator('[data-slot="storage-mounts-limit"]')).toContainText(
          'at most 256 mount paths',
        )
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('holds the mount list on the Agent page across every fixed viewport and theme', async ({
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
      const agent = await server.enrollAgent()
      const report = hostReporter(server, agent)
      const at = [
        25 * MINUTE_SECONDS,
        20 * MINUTE_SECONDS,
        15 * MINUTE_SECONDS,
        10 * MINUTE_SECONDS,
        5 * MINUTE_SECONDS,
      ]
      for (const [index, seconds] of at.entries()) {
        const mounts = [{ mountPath: '/data', totalBytes: 1024, usedBytes: 512 }]
        if (index < 2) mounts.push({ mountPath: '/mnt/archive', totalBytes: 4096, usedBytes: 2048 })
        await report(canonicalAgo(seconds), { cpuPercent: 11 + index, mounts })
      }

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/agents/' + agent.agentId)
            await expectResolvedTheme(page, colorScheme)

            await expect(
              page.getByRole('heading', { level: 2, name: 'Storage by mount path' }),
            ).toBeVisible()
            const rows = mountRows(page)
            await expect(rows).toHaveCount(2)
            await expect(rows.nth(0)).toHaveAttribute('data-mount-state', 'reported')
            await expect(rows.nth(1)).toHaveAttribute('data-mount-state', 'silent')
            await expect(rows.nth(0)).toContainText('512 B of 1.00 KiB · 50%')

            // Whatever does not fit the card scrolls inside this container, never the
            // page, and what does not need to scroll is not clipped inside it either.
            const list = page.locator('[data-slot="storage-mounts-list"]')
            await expect(list).toHaveCSS('overflow-x', 'auto')
            await expect(list.locator('[data-slot="storage-mounts-table"]')).toHaveCount(1)
            await expect(list).toHaveJSProperty('scrollLeft', 0)
            if (await list.evaluate((node) => node.scrollWidth > node.clientWidth)) {
              await list.evaluate((node) => {
                node.scrollLeft = node.scrollWidth
              })
              await expect
                .poll(
                  async () =>
                    list.evaluate((node) => {
                      const reachable = node.scrollWidth - node.clientWidth
                      return reachable > 0 && node.scrollLeft >= reachable - 1
                    }),
                  { message: 'the mount list must scroll to its own last column' },
                )
                .toBe(true)
              // The last column is really reachable once the container is scrolled.
              await expect(rows.nth(0).locator('[data-label="Series"]')).toBeVisible()
              await list.evaluate((node) => {
                node.scrollLeft = 0
              })
              await expect.poll(() => list.evaluate((node) => node.scrollLeft)).toBe(0)
            } else {
              expect(await list.evaluate((node) => node.scrollWidth)).toBeLessThanOrEqual(
                await list.evaluate((node) => node.clientWidth),
              )
            }
            await expectNoHorizontalOverflow(page)

            // The 44px rule is the touch rule, so it is asserted where the page is
            // held to it; the keyboard path and the activation are asserted at
            // every viewport, because a wide screen is not a reason to accept a
            // control a keyboard cannot reach.
            if (viewport.width <= 768) await expectVisibleInteractiveTargets(page)
            const read = rows.nth(0).getByRole('button', {
              name: 'Read the storage series of /data',
            })
            const focused = await focusByKeyboard(page, read)
            await expect(focused).toBeFocused()
            await expectFocusedElementHasVisibleFocus(page)
            // A touch tap reads the series the path names; the row is stacked
            // rather than tabular on the narrow screens and this context reports
            // touch at every width.
            await read.tap()
            const hostPanel = page.locator(AGENT_HOST_PANEL)
            await expect(hostPanel.getByLabel('Mount path')).toHaveValue('/data')
            await expect(
              hostPanel.locator('[data-slot="metric-history-samples"]'),
            ).toContainText('512 B')
            await expectNoHorizontalOverflow(page)
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
