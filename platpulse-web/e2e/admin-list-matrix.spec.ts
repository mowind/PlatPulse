import { expect, test, type Page } from '@playwright/test'
import { expectNoHorizontalOverflow } from './helpers'

/**
 * Acceptance matrix for the Agents summary, the Audit detail disclosure, and
 * the Overview attention-time labels (design §8.5.6 and §8.6).
 *
 * The shared e2e Server seeds one primary Agent plus one no-host Agent, which
 * cannot prove that six columns stay aligned across many different records.
 * This spec injects a deterministic twelve-Agent fixture through Playwright
 * routes (different receipt times, Node counts, credential counts, and
 * diagnostic states) and checks the real browser geometry at 1440, 1920, and
 * a narrow phone width. The authoritative Server contract itself stays
 * covered by agent-lifecycle.spec.ts against the real Server.
 */

const SESSION = {
  session: {
    userId: 'matrix-owner',
    username: 'admin',
    role: 'owner',
    createdAt: '2026-09-08T09:00:00Z',
    lastSeenAt: '2026-09-08T09:00:00Z',
    expiresAt: '2026-09-15T09:00:00Z',
  },
  csrfToken: 'matrix-csrf',
}

const SIX_HEADERS = [
  'Agent',
  'Reporting status',
  'Last received',
  'Node Inventory',
  'Credentials',
  'Diagnostics',
]

const RECEIPT_TIMES = [
  '2026-09-08T09:59:00Z',
  '2026-09-08T09:30:00Z',
  '2026-09-08T08:00:00Z',
  '2026-09-06T12:00:00Z',
  '2026-09-01T00:00:00Z',
  null,
  undefined,
  '2026-09-07T22:15:00Z',
  '2026-09-08T07:45:00Z',
  '2026-09-04T04:04:00Z',
  '2026-09-08T06:00:00Z',
  '2026-09-08T05:00:00Z',
]

function nodeFixture(agentIndex: number, nodeIndex: number) {
  return {
    node_id: 'node-' + agentIndex + '-' + nodeIndex,
    network_key: 'matrix-net',
    display_name: 'Node ' + agentIndex + '-' + nodeIndex,
    lifecycle: nodeIndex % 2 === 0 ? 'active' : 'retired',
    visibility: 'private',
    health: 'healthy',
    health_reason: 'current',
    freshness: 'current',
    current_head: 100 + nodeIndex,
    historical_high_watermark: 100 + nodeIndex,
    resync_state: 'idle',
    resync_progress: null,
    network_reference_head: null,
    network_reference_confidence: 'unknown',
    rpc: null,
    sync: null,
    consensus: null,
    process: null,
  }
}

function credentialFixture(agentIndex: number, credentialIndex: number) {
  const revoked = credentialIndex === 2
  const expired = credentialIndex === 1
  return {
    credential_id: 'cred-' + agentIndex + '-' + credentialIndex,
    created_at: '2026-08-01T00:00:00Z',
    revoked_at: revoked ? '2026-08-20T00:00:00Z' : null,
    revoke_after: expired ? '2026-08-10T00:00:00Z' : null,
    active: credentialIndex === 0,
  }
}

function hostFixture(agentIndex: number) {
  if (agentIndex % 7 === 0) return null
  const spoolObserved = agentIndex % 4 !== 0
  return {
    components:
      agentIndex % 6 === 0
        ? [
            {
              component: 'memory',
              state: 'error',
              error_code: 'mem_pressure',
              error_message: 'recorded component error',
              attempted_at: '2026-09-08T08:00:00Z',
              observed_at: '2026-09-08T08:00:00Z',
              received_at: '2026-09-08T08:00:00Z',
              state_revision: 2,
              value_revision: 1,
            },
          ]
        : [],
    updated_at: '2026-09-08T08:00:00Z',
    ...(spoolObserved
      ? {
          spool_queued_reports: agentIndex % 4,
          spool_in_flight: agentIndex % 2 === 0,
          spool_store_fatal: agentIndex % 3 === 0,
          spool_store_error: agentIndex % 5 === 0 ? 'recorded store error' : null,
          spool_last_delivery_error: agentIndex % 3 === 0 ? 'recorded delivery error' : null,
          spool_dropped_sequence_from: agentIndex % 3 === 0 ? 10 : null,
          spool_dropped_sequence_to: agentIndex % 3 === 0 ? 12 : null,
          spool_dropped_height_from: agentIndex % 3 === 0 ? 900 : null,
          spool_dropped_height_to: agentIndex % 3 === 0 ? 905 : null,
          spool_dropped_time_from: agentIndex % 3 === 0 ? '2026-09-08T07:00:00Z' : null,
          spool_dropped_time_to: agentIndex % 3 === 0 ? '2026-09-08T07:05:00Z' : null,
          spool_report_too_large: agentIndex % 4 === 0,
          spool_pending_history_gaps: agentIndex % 5,
        }
      : {}),
  }
}

function agentFixture(agentIndex: number) {
  const liveness =
    agentIndex % 5 === 0 ? 'unknown' : agentIndex % 4 === 0 ? 'offline' : 'online'
  return {
    agent_id: '0195f2a1-0011-4011-8011-' + String(agentIndex).padStart(12, '0'),
    agent_epoch: 1,
    last_report_sequence: 40 + agentIndex,
    active_boot_id: 'boot-' + agentIndex,
    boot_status: 'active',
    previous_boot_id: null,
    close_report_id: null,
    shutdown_state: 'running',
    shutdown_started_at: null,
    shutdown_deadline_at: null,
    shutdown_finished_at: null,
    shutdown_unresolved_range: null,
    shutdown_last_error: null,
    shutdown_forced: false,
    shutdown_report_id: null,
    shutdown_report_sequence: null,
    shutdown_updated_at: null,
    sequence_gap_count: agentIndex,
    security_event_count: agentIndex % 3,
    clock_status: 'ok',
    clock_skew_ms: 12,
    liveness,
    last_received_at: RECEIPT_TIMES[agentIndex - 1],
    capabilities: ['host', 'node_chain'],
    credentials: Array.from({ length: agentIndex % 4 }, (_, index) =>
      credentialFixture(agentIndex, index),
    ),
    host: hostFixture(agentIndex),
    nodes: Array.from({ length: agentIndex % 5 }, (_, index) =>
      nodeFixture(agentIndex, index),
    ),
  }
}

const LONG_AUDIT_NOTE =
  'operator supplied rename reason: ' +
  'the display name changed because the host was moved between sites and the old label no longer identifies it; '.repeat(
    3,
  )

const AUDIT_ITEMS = [
  {
    auditEventId: 901,
    eventKind: 'node_metadata_changed',
    actorUsername: 'admin',
    targetKind: 'node',
    targetId: '0195f2a1-0014-4014-8014-000000000014',
    createdAt: '2026-09-08T08:00:00Z',
    details: {
      username: 'admin',
      changed: ['display_name', 'visibility'],
      note: LONG_AUDIT_NOTE,
      before: { display_name: 'Node A', visibility: 'public' },
      after: {
        display_name:
          'Node A renamed with a very long operator supplied label that must wrap inside the detail row',
        visibility: 'private',
      },
    },
  },
  {
    auditEventId: 902,
    eventKind: 'agent_credential_revoked',
    actorUsername: 'admin',
    targetKind: 'agent',
    targetId: '0195f2a1-0011-4011-8011-000000000001',
    createdAt: '2026-09-08T07:00:00Z',
    details: { credential_id: 'cred-1', reason: LONG_AUDIT_NOTE },
  },
  {
    auditEventId: 903,
    eventKind: 'session_revoked',
    actorUsername: null,
    targetKind: 'session',
    targetId: '0195f2a1-0099-4099-8099-000000000099',
    createdAt: '2026-09-08T06:00:00Z',
    details: null,
  },
]

const OVERVIEW = {
  generated_at: '2026-09-08T09:59:30Z',
  summary: {
    agents: { total: 2, online: 1, offline: 1, unknown: 0 },
    nodes: { total: 1, active: 1, healthy: 0, unhealthy: 1, unknown: 0, retired: 0 },
    networks: { total: 1, with_identity_mismatch: 0 },
  },
  attention: [
    {
      id: 'agent_report_gap:agent:aaa',
      kind: 'agent_report_gap',
      severity: 'warning',
      subject_kind: 'agent',
      subject_id: '0195f2a1-0011-4011-8011-000000000001',
      subject_label: 'Agent One',
      message: '2 report sequence gaps were recorded',
      observed_at: '2026-09-08T07:59:30Z',
    },
    {
      id: 'agent_security_event:agent:bbb',
      kind: 'agent_security_event',
      severity: 'critical',
      subject_kind: 'agent',
      subject_id: '0195f2a1-0011-4011-8011-000000000002',
      subject_label: 'Agent Two',
      message: '1 security event was recorded',
      observed_at: null,
    },
  ],
}

async function mockAdminApi(page: Page, agents: () => unknown[]) {
  // Playwright matches the most recently registered route first, so the
  // catch-all is registered before the specific endpoints.
  await page.route('**/api/admin/v1/**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '[]' }),
  )
  await page.route('**/api/admin/v1/agents', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(agents()) }),
  )
  await page.route('**/api/admin/v1/nodes', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '[]' }),
  )
  await page.route('**/api/admin/v1/overview', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(OVERVIEW),
    }),
  )
  await page.route('**/api/admin/v1/audit*', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ items: AUDIT_ITEMS, nextBefore: null }),
    }),
  )
  await page.route('**/api/public/v1/session', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(SESSION) }),
  )
}

type RowGeometry = {
  id: string
  labels: Array<string | null>
  cells: Array<{ left: number; width: number; top: number }>
  headers: Array<{ left: number; width: number }>
  text: string
}

async function readAgentsGeometry(page: Page): Promise<RowGeometry[]> {
  return page
    .locator('table.agent-table tbody tr:not(.node-detail-row)')
    .evaluateAll((rows) =>
      rows.map((row) => {
        const box = (element: Element) => {
          const rect = element.getBoundingClientRect()
          return {
            left: Math.round(rect.left),
            width: Math.round(rect.width),
            top: Math.round(rect.top),
          }
        }
        const table = row.closest('table') as HTMLTableElement
        return {
          id:
            (row.querySelector('a[href^="/admin/agents/"]')?.getAttribute('href') ?? '').split(
              '/',
            ).pop() ?? '',
          labels: [...row.children].map((cell) => cell.getAttribute('data-label')),
          cells: [...row.children].map(box),
          headers: [...table.querySelectorAll('thead th')].map(box),
          text: (row.textContent ?? '').replace(/\s+/g, ' ').trim(),
        }
      }),
    )
}

test.describe('SCN-AGENTS-PRIORITY-SUMMARY acceptance matrix', () => {
  test('keeps six aligned columns for twelve varied records at 1440/1920/390', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'explicit-width matrix runs once')
    let agents = Array.from({ length: 12 }, (_, index) => agentFixture(index + 1))
    await mockAdminApi(page, () => agents)

    for (const width of [1440, 1920, 390]) {
      await page.setViewportSize({ width, height: 1000 })
      await page.goto('/admin/agents')
      await expect(page.locator('table.agent-table thead th')).toHaveText(SIX_HEADERS)
      await expect(page.locator('table.agent-table tbody tr:not(.node-detail-row)')).toHaveCount(12)

      const rows = await readAgentsGeometry(page)
      expect(rows).toHaveLength(12)
      for (const row of rows) {
        expect(row.labels).toEqual(SIX_HEADERS)
        if (width > 767) {
          for (let index = 0; index < 6; index += 1) {
            expect(Math.abs(row.cells[index].left - row.headers[index].left)).toBeLessThanOrEqual(1)
            expect(Math.abs(row.cells[index].width - row.headers[index].width)).toBeLessThanOrEqual(1)
          }
        }
        const agentIndex = Number(row.id.slice(-12))
        const numberFor = (label: string) => {
          const match = row.text.match(new RegExp(label + '\\s*(\\d+)'))
          return match ? Number(match[1]) : Number.NaN
        }
        expect(numberFor('Recorded gap intervals')).toBe(agentIndex)
        expect(numberFor('Accumulated recorded security events')).toBe(agentIndex % 3)
        const retained = row.text.match(/(\d+) retained Nodes?/)
        expect(retained ? Number(retained[1]) : Number.NaN).toBe(agentIndex % 5)
        const credentials = agentIndex % 4
        expect(row.text).toContain(credentials === 0 ? 'None issued' : credentials + ' total')
      }

      if (width <= 390) {
        const cardOrder = [...rows[0].cells]
          .map((cell, index) => ({ label: rows[0].labels[index], top: cell.top }))
          .sort((left, right) => left.top - right.top)
          .map((cell) => cell.label)
        expect(cardOrder).toEqual([
          'Agent',
          'Reporting status',
          'Last received',
          'Diagnostics',
          'Node Inventory',
          'Credentials',
        ])
      }

      // Full Agent ID stays reachable without disturbing the six columns.
      const firstRow = page.locator('table.agent-table tbody tr:not(.node-detail-row)').first()
      await firstRow.getByRole('button', { name: 'Show full Agent ID' }).click()
      await expect(firstRow.locator('code.agent-full-id')).toHaveText(rows[0].id)
      await expect(firstRow.locator('> *')).toHaveCount(6)

      // Diagnostics keeps only the summary; the full findings open below.
      const riskyRow = page
        .locator('table.agent-table tbody tr:not(.node-detail-row)')
        .filter({ hasText: 'store fatal' })
        .first()
      await expect(riskyRow).toContainText('Recorded evidence')
      await expect(riskyRow).not.toContainText('Dropped sequence range')
      const toggle = riskyRow.getByRole('button', { name: /diagnostics/ })
      await toggle.click()
      await expect(toggle).toHaveAttribute('aria-expanded', 'true')
      const detailRow = riskyRow.locator('xpath=following-sibling::tr[1]')
      await expect(detailRow).toHaveClass(/node-detail-row/)
      await expect(detailRow.locator('td')).toHaveAttribute('colspan', '6')
      await expect(detailRow).toContainText('Dropped sequence range')
      await expect(detailRow).toContainText('Host snapshot')
      await expect(riskyRow.locator('> *')).toHaveCount(6)
      await page.keyboard.press('Escape')
      await expect(toggle).toHaveAttribute('aria-expanded', 'false')
      await expect(page.locator('table.agent-table tr.node-detail-row')).toHaveCount(0)

      await expectNoHorizontalOverflow(page)
    }

    // Refresh correspondence: the same Agent identity renders the refreshed
    // values, and every row still owns its own cells.
    agents = agents.map((agent, index) =>
      index === 0
        ? { ...agent, sequence_gap_count: 777, nodes: agent.nodes.slice(0, 1) }
        : agent,
    )
    await page.reload()
    await expect(page.locator('table.agent-table tbody tr:not(.node-detail-row)')).toHaveCount(12)
    const refreshed = (await readAgentsGeometry(page)).find((row) => row.id.endsWith('000000000001'))
    expect(refreshed?.text).toMatch(/Recorded gap intervals\s*777/)
    expect(refreshed?.text).toContain('1 retained Node')
    expect(refreshed?.labels).toEqual(SIX_HEADERS)
  })

  test('Audit renders multi-field long details in a cross-column row at 1440/1920/390', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'explicit-width matrix runs once')
    await mockAdminApi(page, () => [])

    for (const width of [1440, 1920, 390]) {
      await page.setViewportSize({ width, height: 1000 })
      await page.goto('/admin/access/audit')
      await expect(page.locator('table.audit-table thead th')).toHaveText([
        'Time',
        'Event',
        'Actor',
        'Target',
        'Details',
      ])
      const items = page.locator('table.audit-table tbody tr:not(.node-detail-row)')
      await expect(items).toHaveCount(3)
      for (let index = 0; index < 3; index += 1) {
        await expect(items.nth(index).locator('> *')).toHaveCount(5)
      }
      const firstItem = items.first()
      await expect(firstItem).not.toContainText('operator supplied rename reason')
      const actionCell = firstItem.locator('td[data-label="Details"]')
      const widthBefore = (await actionCell.boundingBox())?.width ?? 0

      // Keyboard: focus the disclosure and open it with Enter.
      await actionCell.getByRole('button', { name: 'Show details' }).focus()
      await page.keyboard.press('Enter')
      const detailRow = firstItem.locator('xpath=following-sibling::tr[1]')
      await expect(detailRow).toHaveClass(/node-detail-row/)
      await expect(detailRow.locator('td')).toHaveAttribute('colspan', '5')
      await expect(detailRow).toContainText('operator supplied rename reason')
      await expect(detailRow).toContainText(
        'Node A renamed with a very long operator supplied label that must wrap',
      )
      const wrapped = await detailRow
        .locator('dd')
        .evaluateAll((values) => values.every((value) => value.scrollWidth <= value.clientWidth + 1))
      expect(wrapped).toBe(true)
      const widthAfter = (await actionCell.boundingBox())?.width ?? 0
      expect(Math.abs(widthAfter - widthBefore)).toBeLessThanOrEqual(1)

      await page.keyboard.press('Escape')
      await expect(firstItem.getByRole('button', { name: 'Show details' })).toBeVisible()
      await expect(page.locator('table.audit-table tr.node-detail-row')).toHaveCount(0)
      await expectNoHorizontalOverflow(page)
    }
  })

  test('Overview labels the observation time separately from the snapshot time', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'explicit-width matrix runs once')
    await mockAdminApi(page, () => [])
    await page.goto('/admin')

    const header = page.locator('.header-status')
    await expect(header).toContainText('Last good snapshot')
    await expect(header.locator('time')).toHaveAttribute('datetime', OVERVIEW.generated_at)
    const snapshotRelative = ((await header.textContent()) ?? '')
      .replace('Last good snapshot ·', '')
      .replace('Refresh', '')
      .trim()

    const known = page.locator('.attention-item', { hasText: 'agent_report_gap' })
    await expect(known).toContainText('Last observed')
    await expect(known.locator('time')).toHaveAttribute(
      'datetime',
      '2026-09-08T07:59:30Z',
    )

    const unknown = page.locator('.attention-item', { hasText: 'agent_security_event' })
    await expect(unknown).toContainText('Observation time unknown')
    await expect(unknown.locator('time')).toHaveCount(0)
    if (snapshotRelative.length > 0) {
      await expect(unknown).not.toContainText(snapshotRelative)
    }
    await expectNoHorizontalOverflow(page)
  })
})
