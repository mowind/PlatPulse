import { expect, test, type Locator, type Page } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import type { AdminStateHistoryResponse } from '../src/api/generated'
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
 * The recorded state history of one Node (issue #217, design §11.7).
 *
 * A state is evidence rather than a value, and the whole acceptance of this
 * surface is that the panel never turns evidence into a claim nobody made: an
 * unchanged state is re-recorded as an anchor instead of being drawn as a
 * transition, a failed probe is a failure rather than a false flag, the value a
 * failure still carries keeps the instant it was really observed at, and a
 * stretch nobody observed is a silence with its own kind and reason instead of a
 * state carried across it. The spec drives those cases through real Reports
 * against a disposable Server, checks the Server's own answer first so the panel
 * is judged against the record rather than against a number the spec picked, and
 * then reads the rendered panel across all five viewports the Admin matrix
 * fixes, in both resolved themes.
 */

/** A floor no real filesystem clears, and a floor every filesystem clears. */
const MAX_PERSISTED_BYTES = '9223372036854775807'
const CLEARED_FLOOR = 1
const MINUTE_SECONDS = 60
const HOUR_SECONDS = 3600

const REPORT_FIXTURE = '../crates/platpulse-core/tests/fixtures/report_v1_minimal.json'

/** The panel this spec drives, named by its surface so no other card is read. */
const NODE_STATE_PANEL = '[data-slot="state-history-panel"][data-surface="node-state"]'

/**
 * A clock whose instants are whole seconds a chosen age before one fixed now.
 * The spans the Server measures are then exactly the spans this spec states,
 * instead of drifting with the millisecond each call happens to land on.
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

/**
 * Declare a new low-space floor in the Server's configuration file. The capacity
 * policy is read at startup, so the Server is restarted on the same state
 * directory: a floor no real filesystem clears adopts protection, and a cleared
 * floor releases it. This is the Operator action the design describes for a
 * recorded pause, and it never pokes the database behind a running Server.
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

/** What one real Report says about the recorded state components it carries. */
type StateDelivery = {
  component: 'sync' | 'consensus'
  /** The flag a successful sync probe reads, omitted when the probe failed. */
  syncing?: boolean
  /** The failure the Agent reported, when the probe failed. */
  errorCode?: string
  /** The instant the carried value was really observed at, for a failed probe. */
  valueObservedAt?: string
}

/**
 * The fixture with its Node stamped at a chosen instant, so the recorded state
 * the Server writes is one this spec decided on. The consensus block is left
 * exactly as the fixture states it: a component that reports itself unsupported
 * still proves the Node was alive when the Report was generated, and the Server
 * records that as a state of its own.
 */
function stateReport(observed: string, sequence: number, delivery: StateDelivery) {
  return (report: Record<string, unknown>) => {
    report.report_sequence = sequence
    report.report_id = '0195f2a1-01b7-4091-8091-0000000000' + String(sequence).padStart(2, '0')
    report.generated_at = observed
    if (delivery.component === 'consensus') return
    const node = (report.nodes as Record<string, unknown>[])[0]
    const sync = (node.chain as Record<string, unknown>).sync as Record<string, unknown>
    sync.attempted_at = observed
    sync.latest_observed_at = delivery.valueObservedAt ?? observed
    ;(sync.latest as Record<string, unknown>).syncing = delivery.syncing === true
    if (delivery.errorCode !== undefined) {
      // The probe failed and the Agent says so, while still carrying the reading
      // it really observed earlier: a carried reading is a delivery, not an
      // observation, so the recorded row keeps the instant the value came from.
      sync.status = 'error'
      sync.error = { code: delivery.errorCode, message: 'the Node did not answer' }
    }
  }
}

/** Read the Owner-only recorded state history for one component over real HTTP. */
async function readStateHistory(
  server: DisposableServer,
  nodeId: string,
  component: string,
): Promise<AdminStateHistoryResponse> {
  const response = await server.adminGet(
    '/api/admin/v1/nodes/' + nodeId + '/state-history?component=' + component,
  )
  expect(response.status).toBe(200)
  return response.body as AdminStateHistoryResponse
}

function statePanel(page: Page): Locator {
  return page.locator(NODE_STATE_PANEL)
}

/** The facts the panel must state for the six recorded sync states below. */
async function expectRecordedSyncStates(panel: Locator): Promise<void> {
  await expect(
    panel.getByRole('heading', { level: 2, name: 'Sync and consensus history' }),
  ).toBeVisible()
  // The ledger behind the rows, in the Server's own figures.
  await expect(panel.locator('[data-slot="state-history-order"]')).toContainText(
    'Oldest recorded state first, in the order the Server answered them.',
  )
  await expect(panel.locator('[data-slot="state-history-anchor-rule"]')).toContainText(
    'keeps a constant state provable',
  )
  await expect(panel.locator('[data-slot="state-history-cadence"]')).toContainText(
    'This Node reported every 40 minutes',
  )
  await expect(panel.locator('[data-slot="state-history-coverage"]')).toContainText(
    'The record proves 1 hour 3 minutes of 24 hours',
  )
  await expect(panel.locator('[data-slot="state-history-newest"]')).toContainText(
    '10 minutes older than the end of this window',
  )
  // The chart draws the silence as a silence and the anchor as its own marker,
  // so an unchanged state is never drawn as a transition and a hole is never
  // drawn as a line.
  await expect(panel.locator('[data-slot="state-history-chart"]')).toBeVisible()
  await expect(
    panel.locator('[data-slot="state-history-chart"] [data-slot="state-history-entry"]'),
  ).toHaveCount(5)
  await expect(
    panel.locator('[data-slot="state-history-chart"] [data-slot="state-history-anchor"]'),
  ).toHaveCount(1)
  await expect(panel.locator('[data-slot="state-history-gap-band"]')).toHaveCount(1)
  // The silence is stated by kind, by its real length and by what it swallowed.
  await expect(panel.locator('[data-slot="state-history-gaps"]')).toContainText(
    'Silences in this window',
  )
  await expect(panel.locator('[data-slot="state-history-gap"]')).toHaveAttribute(
    'data-gap-kind',
    'collection_gap',
  )
  await expect(panel.locator('[data-slot="state-history-gap"]')).toContainText('Collection gap')
  await expect(panel.locator('[data-slot="state-history-gap"]')).toContainText('2 hours 17 minutes')
  await expect(panel.locator('[data-slot="state-history-gap"]')).toContainText(
    'Nobody observed this Node across this stretch',
  )
  // Nothing that never happened is claimed: no pause, no truncation, no failed
  // refresh, no clamped window and no component the Server never recorded.
  await expect(panel.locator('[data-slot="state-history-pause"]')).toHaveCount(0)
  await expect(panel.locator('[data-slot="state-history-truncation"]')).toHaveCount(0)
  await expect(panel.locator('[data-slot="state-history-page"]')).toHaveCount(0)
  await expect(panel.locator('[data-slot="state-history-refresh-error"]')).toHaveCount(0)
  await expect(panel.locator('[data-slot="state-history-availability"]')).toHaveCount(0)
  await expect(panel.locator('[data-slot="state-history-unasked"]')).toHaveCount(0)
  // The rows themselves: five changes and one re-recorded anchor.
  const samples = panel.locator('[data-slot="state-history-samples"]')
  await expect(samples.locator('[data-slot="state-history-entry"]')).toHaveCount(5)
  await expect(samples.locator('[data-slot="state-history-anchor"]')).toHaveCount(1)
  await expect(samples.locator('[data-entry-kind="anchor"]')).toContainText('Anchor')
  await expect(samples).toContainText('Collection succeeded')
  await expect(samples).toContainText('Value observed now')
  await expect(samples).toContainText('Current')
  await expect(samples).toContainText('None recorded')
  await expect(samples).toContainText('Clock in step')
  // The flag is stated only where a probe read it, and the claim says so.
  await expect(
    samples.getByText('The Node reported that it was syncing when this state was recorded.'),
  ).toHaveCount(3)
  await expect(
    samples.getByText('The Node reported that it was not syncing when this state was recorded.'),
  ).toHaveCount(3)
  await expect(
    samples.getByText(
      'Whether the Node was syncing is unknown: the collection did not succeed, so no flag was read and none is claimed.',
    ),
  ).toHaveCount(0)
}

test.describe('recorded state history', () => {
  test('records a change and an anchor, states a silence by kind, and never claims the stretch after the newest state', async ({
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
      const ago = clock()
      let sequence = 0
      const deliverSync = async (secondsAgo: number, syncing: boolean): Promise<void> => {
        sequence += 1
        await server.submitReport(
          agent.agentId,
          agent.credential,
          stateReport(ago(secondsAgo), sequence, { component: 'sync', syncing }),
        )
      }

      // Four deliveries a minute apart, alternating the one flag a successful
      // sync probe carries: every one of them changes the state vector, so every
      // one of them is a recorded change. Then the same vector again an hour and
      // three minutes after the last row, which is the anchor: the state did not
      // change, so no transition is invented, and the delivery still proves the
      // Node was reporting. The last delivery is ten minutes before the end of
      // the window, which leaves the hours between the anchor and it unobserved, so
      // the answer states that silence as a gap with its own seconds. The stretch
      // between two recorded states still counts as covered: the anchor window is
      // what keeps that claim honest, because an unchanged state that is delivered
      // at least once an hour writes a row. A silence shorter than the window plus
      // one cadence is not provable from the recorded log, and nothing here claims
      // it.
      await deliverSync(210 * MINUTE_SECONDS, true)
      await deliverSync(209 * MINUTE_SECONDS, false)
      await deliverSync(208 * MINUTE_SECONDS, true)
      await deliverSync(207 * MINUTE_SECONDS, false)
      await deliverSync(147 * MINUTE_SECONDS, false)
      await deliverSync(10 * MINUTE_SECONDS, true)

      // The Server's answer is the record the panel is judged against.
      const answer = await readStateHistory(server, nodeId, 'sync')
      expect(answer.component).toBe('sync')
      expect(answer.series.observed).toBe(true)
      expect(answer.series.entryCount).toBe(6)
      expect(answer.series.changeCount).toBe(5)
      expect(answer.series.anchorCount).toBe(1)
      expect(answer.series.replayedCount).toBe(0)
      expect(answer.entries.map((entry) => entry.entryKind)).toEqual([
        'change',
        'change',
        'change',
        'change',
        'anchor',
        'change',
      ])
      expect(answer.entries.map((entry) => entry.syncing)).toEqual([true, false, true, false, false, true])
      expect(answer.entries.map((entry) => entry.collectionState)).toEqual([
        'ok',
        'ok',
        'ok',
        'ok',
        'ok',
        'ok',
      ])
      expect(answer.anchorSeconds).toBe(HOUR_SECONDS)
      expect(answer.cadenceSeconds).toBe(40 * MINUTE_SECONDS)
      expect(answer.coverageSeconds).toBe(3 * MINUTE_SECONDS + HOUR_SECONDS)
      expect(answer.gaps.map((gap) => gap.kind)).toEqual(['collection_gap'])
      expect(answer.gaps[0].seconds).toBe(2 * HOUR_SECONDS + 17 * MINUTE_SECONDS)
      expect(answer.availability ?? null).toBeNull()
      expect(answer.truncated).toBe(false)
      expect(answer.continuation ?? null).toBeNull()

      // The other recorded component of the same Node: the fixture reports its
      // consensus as unsupported, which is still a state the Server recorded, so
      // the panel shows it as what it is rather than as a healthy card.
      const consensus = await readStateHistory(server, nodeId, 'consensus')
      expect(consensus.series.observed).toBe(true)
      expect(consensus.series.entryCount).toBe(6)
      expect(consensus.entries.map((entry) => entry.entryKind)).toEqual([
        'change',
        'anchor',
        'anchor',
      ])
      expect(consensus.entries.every((entry) => (entry.syncing ?? null) === null)).toBe(true)
      expect(consensus.entries.map((entry) => entry.collectionState)).toEqual([
        'unsupported',
        'unsupported',
        'unsupported',
      ])
      expect(consensus.entries.map((entry) => entry.valueSource)).toEqual(['none', 'none', 'none'])

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
            await expectResolvedTheme(page, colorScheme)
            const panel = statePanel(page)
            await expectRecordedSyncStates(panel)
            await expectLocalTableScroll(page, 'state-history-samples', NODE_STATE_PANEL)
            await expectNoHorizontalOverflow(page)

            if (viewport.width <= 768) {
              await expectVisibleInteractiveTargets(page)
              const componentSelect = await focusByKeyboard(page, panel.getByLabel('Component'))
              await expect(componentSelect).toBeFocused()
              await expectFocusedElementHasVisibleFocus(page)
              // A touch tap narrows the window, and the panel then answers the
              // window it was asked for: only the newest recorded state is
              // inside an hour, and no stretch between two recorded states is
              // proven there.
              await panel.getByRole('button', { name: '1 hour', exact: true }).tap()
              const samples = panel.locator('[data-slot="state-history-samples"]')
              await expect(samples.locator('[data-slot="state-history-entry"]')).toHaveCount(1)
              await expect(samples.locator('[data-slot="state-history-anchor"]')).toHaveCount(0)
              await expect(panel.locator('[data-slot="state-history-coverage"]')).toContainText(
                'none of it is covered',
              )
              await panel.getByRole('button', { name: '24 hours', exact: true }).tap()
              await expect(samples.locator('[data-slot="state-history-entry"]')).toHaveCount(5)
            }

            if (colorScheme === 'light') {
              // The component switcher asks the Server for the second recorded
              // component, which has its own evidence and its own anchors.
              await panel.getByLabel('Component').selectOption('consensus')
              const consensusRows = panel.locator('[data-slot="state-history-samples"]')
              await expect(consensusRows.locator('[data-slot="state-history-entry"]')).toHaveCount(1)
              await expect(consensusRows.locator('[data-slot="state-history-anchor"]')).toHaveCount(2)
              await expect(consensusRows).toContainText('Collection is unsupported')
              await expect(consensusRows).toContainText('No value')
              await expect(consensusRows).toContainText('Unsupported')
              // The column header always reads Syncing; what must never appear is
              // a flag read from a collection that did not succeed.
              await expect(consensusRows).toContainText('Unknown')
              await expect(consensusRows).not.toContainText('Not syncing')
              await expect(consensusRows).not.toContainText(
                'reported that it was syncing when this state was recorded',
              )
              await panel.getByLabel('Component').selectOption('sync')
              await expect(consensusRows.locator('[data-slot="state-history-entry"]')).toHaveCount(5)
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

  test('discloses a protection pause as a pause with the observations it skipped', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the pause flow runs once')
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
      const ago = clock()
      let sequence = 0
      const deliverSync = async (secondsAgo: number, syncing: boolean): Promise<void> => {
        sequence += 1
        await server.submitReport(
          agent.agentId,
          agent.credential,
          stateReport(ago(secondsAgo), sequence, { component: 'sync', syncing }),
        )
      }

      // A recorded state before the Operator pauses history collection.
      await deliverSync(3 * HOUR_SECONDS, true)

      // The floor no real filesystem clears: low-space protection pauses
      // optional history. The two deliveries that arrive while it is paused
      // record no state, and the loss is counted as skipped observations.
      declareFloor(server.stateDir, MAX_PERSISTED_BYTES)
      await server.restart()
      await deliverSync(150 * MINUTE_SECONDS, true)
      await deliverSync(120 * MINUTE_SECONDS, true)

      // The Operator clears the floor and protection releases: the next delivery
      // is recorded again, and the stretch between it and the older row is a
      // loss the Server knows about rather than a Node that went quiet.
      declareFloor(server.stateDir, CLEARED_FLOOR)
      await server.restart()
      await deliverSync(60 * MINUTE_SECONDS, false)

      const answer = await readStateHistory(server, nodeId, 'sync')
      expect(answer.series.entryCount).toBe(2)
      expect(answer.entries.map((entry) => entry.entryKind)).toEqual(['change', 'change'])
      expect(answer.gaps.map((gap) => gap.kind)).toEqual(['protection_pause'])
      expect(answer.gaps[0].skippedCount).toBe(2)
      expect(answer.gaps[0].seconds).toBe(2 * HOUR_SECONDS)
      expect(answer.coverageSeconds).toBe(0)

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await page.setViewportSize({ width: 1280, height: 800 })
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        const panel = statePanel(page)
        // The pause is disclosed as a pause, with the observations it swallowed,
        // and never as a state the Node held across the stretch.
        const pause = panel.locator('[data-slot="state-history-pause"]')
        await expect(pause).toContainText(
          'This window holds 1 stretch of paused history collection and 2 skipped observation(s) behind them',
        )
        await expect(pause).toContainText(
          'a paused stretch is not a state of the Node and not a change of one',
        )
        const gap = panel.locator('[data-slot="state-history-gap"]')
        await expect(gap).toHaveCount(1)
        await expect(gap).toHaveAttribute('data-gap-kind', 'protection_pause')
        await expect(gap).toContainText('Protection pause')
        await expect(gap).toContainText('low-space protection paused state recording')
        await expect(gap).toContainText('2 observations were skipped behind it.')
        await expect(gap).toContainText(
          'The operator paused history collection across this stretch, so the record shows no state here',
        )
        await expect(panel.locator('[data-slot="state-history-gap-band"]')).toHaveCount(1)
        // Two rows and no covered stretch: the record proves no state between
        // them, so the panel claims none.
        await expect(
          panel.locator('[data-slot="state-history-samples"] [data-slot="state-history-entry"]'),
        ).toHaveCount(2)
        await expect(panel.locator('[data-slot="state-history-coverage"]')).toContainText(
          'none of it is covered',
        )
        await expect(panel.locator('[data-slot="state-history-cadence"]')).toContainText(
          'This Node reported every 2 hours',
        )
        await expectNoHorizontalOverflow(page)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('records a failed probe as a failure with the age of the value it still carries', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the failed-probe flow runs once')
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
      const ago = clock()

      // One successful probe that observed the flag was off, then one that
      // failed an hour later while still carrying the reading it observed
      // before: the failure is a state of its own and the flag it could not read
      // is unknown rather than false.
      await server.submitReport(
        agent.agentId,
        agent.credential,
        stateReport(ago(2 * HOUR_SECONDS), 1, { component: 'sync', syncing: false }),
      )
      await server.submitReport(
        agent.agentId,
        agent.credential,
        stateReport(ago(HOUR_SECONDS), 2, {
          component: 'sync',
          errorCode: 'rpc_unreachable',
          valueObservedAt: ago(2 * HOUR_SECONDS),
        }),
      )

      const answer = await readStateHistory(server, nodeId, 'sync')
      expect(answer.entries.map((entry) => entry.collectionState)).toEqual(['ok', 'error'])
      expect(answer.entries.map((entry) => entry.valueSource)).toEqual(['current', 'last_good'])
      expect(answer.entries[1].errorCode).toBe('rpc_unreachable')
      expect(answer.entries[1].syncing ?? null).toBeNull()
      // The row keeps the instant the value really came from, so its age can be
      // read instead of a fresh-looking timestamp implying a fresh reading.
      expect(answer.entries[1].valueObservedAt).toBe(answer.entries[0].observedAt)
      // A failed probe is not a silence: the Node was heard from, and it said the
      // collection failed.
      expect(answer.gaps).toEqual([])
      expect(answer.coverageSeconds).toBe(HOUR_SECONDS)

      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        await page.setViewportSize({ width: 1280, height: 800 })
        await gotoAuthenticated(page, server.baseUrl, '/admin/nodes/' + nodeId)
        const panel = statePanel(page)
        const samples = panel.locator('[data-slot="state-history-samples"]')
        await expect(samples.locator('[data-slot="state-history-entry"]')).toHaveCount(2)
        await expect(samples).toContainText('Collection failed')
        await expect(samples).toContainText('Retained last good value')
        await expect(samples).toContainText('Last good')
        await expect(samples).toContainText('rpc_unreachable')
        await expect(samples).toContainText(
          'The value this state refers to was 1 hour old when the state was recorded.',
        )
        // The flag was never read, so the panel says unknown and claims neither
        // syncing nor not syncing.
        await expect(samples).toContainText('Unknown')
        await expect(samples).toContainText(
          'Whether the Node was syncing is unknown: the collection did not succeed, so no flag was read and none is claimed.',
        )
        await expect(panel.locator('[data-slot="state-history-gaps"]')).toHaveCount(0)
        // The Server's newest word about the component is the failure itself.
        await expect(panel.locator('[data-slot="state-history-latest"]')).toContainText(
          'and the failure rpc_unreachable',
        )
        await expectNoHorizontalOverflow(page)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
