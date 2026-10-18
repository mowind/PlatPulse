import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test'
import { expectNoHorizontalOverflow, expectVisibleInteractiveTargets } from './helpers'

/**
 * Admin follow-up regression (webui.md §8.5.3, §11.1, §12.3): long non-secret
 * identifiers open on demand with copy feedback, and the Sessions list keeps
 * labelled, unclipped cards and confirmations on phones. Runs in every fixed
 * project, so 360/390/768 touch and 1280/1440 pointer widths each assert the
 * real browser geometry. Data comes from Playwright routes; no Server state
 * is touched.
 */

const SESSION = {
  session: {
    userId: 'followup-owner',
    username: 'admin',
    role: 'owner',
    createdAt: '2026-10-01T09:00:00Z',
    lastSeenAt: '2026-10-01T09:00:00Z',
    expiresAt: '2026-10-08T09:00:00Z',
  },
  csrfToken: 'followup-csrf',
}

const GENESIS = '0x' + '9f3c'.repeat(16)
const P2P_KEY = '0x' + 'ab12'.repeat(32)

const NETWORKS = [
  {
    network_key: 'platon-followup-network-with-a-long-key',
    display_name: 'PlatON follow-up Network with a deliberately long display name',
    genesis_hash: GENESIS,
    chain_id: 210425,
    p2p_network_id: 1,
    address_hrp: 'lat',
    created_at: '2026-10-01T08:00:00Z',
    updated_at: '2026-10-01T08:00:00Z',
    active_node_count: 2,
    retired_node_count: 1,
    mismatched_node_count: 0,
  },
]

const IDENTITIES = [
  {
    nodeId: '0195f2a1-0014-4014-8014-000000000014',
    nodeDisplayName: 'Observed Node',
    networkKey: 'platon-followup',
    lifecycle: 'active',
    state: 'network_identity_mismatch',
    reason: 'The observed P2P public key belongs to a different Network identity.',
    observedValidatorNodeKey: P2P_KEY,
    validatorId: null,
    validatorNodeKey: null,
    associationEffective: false,
    evaluatedAt: '2026-10-01T08:00:00Z',
  },
  {
    nodeId: '0195f2a1-0015-4015-8015-000000000015',
    nodeDisplayName: 'Unobserved Node',
    networkKey: 'platon-followup',
    lifecycle: 'active',
    state: 'not_evaluated',
    reason: null,
    observedValidatorNodeKey: null,
    validatorId: null,
    validatorNodeKey: null,
    associationEffective: false,
    evaluatedAt: null,
  },
]

const LONG_USER = 'operator-with-an-unusually-long-username-for-review'
const SESSIONS = {
  sessions: [
    {
      sessionId: 'session-current',
      userId: 'followup-owner',
      username: 'admin',
      role: 'owner',
      clientHint: 'Chrome · desktop',
      createdAt: '2026-10-01T08:00:00Z',
      lastSeenAt: '2026-10-01T09:00:00Z',
      expiresAt: '2026-10-08T08:00:00Z',
      current: true,
    },
    {
      sessionId: 'session-other',
      userId: 'followup-viewer',
      username: LONG_USER,
      role: 'viewer',
      clientHint: 'Firefox · mobile · a long coarse client hint that must wrap',
      createdAt: '2026-10-01T07:00:00Z',
      lastSeenAt: '2026-10-01T08:30:00Z',
      expiresAt: '2026-10-08T07:00:00Z',
      current: false,
    },
  ],
}

async function mockAdmin(page: Page) {
  // Most recently registered route wins: the catch-all goes first.
  await page.route('**/api/admin/v1/**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '[]' }),
  )
  await page.route('**/api/admin/v1/networks', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(NETWORKS) }),
  )
  await page.route('**/api/admin/v1/validator-identities', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(IDENTITIES) }),
  )
  await page.route('**/api/admin/v1/sessions', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(SESSIONS) }),
  )
  await page.route('**/api/admin/v1/sessions/session-other/revoke', (route) =>
    route.fulfill({
      status: 409,
      contentType: 'application/json',
      body: JSON.stringify({
        error: {
          code: 'session_already_revoked',
          message: 'This Session was already revoked by another operator.',
          requestId: 'followup',
          fields: [],
        },
      }),
    }),
  )
  await page.route('**/api/public/v1/session', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(SESSION) }),
  )
}

const isTouch = (testInfo: TestInfo) => testInfo.project.name.includes('touch')

/** Activate with the project's own input: tap on touch projects, keyboard
 * (focus + Enter) on pointer projects. */
async function activate(page: Page, control: Locator, testInfo: TestInfo) {
  await expect(control).toBeVisible()
  if (isTouch(testInfo)) {
    await control.tap()
  } else {
    await control.focus()
    await page.keyboard.press('Enter')
  }
}

async function stubClipboard(page: Page, mode: 'resolve' | 'reject' | 'missing') {
  await page.evaluate((mode) => {
    const value =
      mode === 'missing'
        ? undefined
        : {
            writeText: (text: string) => {
              ;(window as unknown as { copiedText?: string }).copiedText = text
              return mode === 'resolve' ? Promise.resolve() : Promise.reject(new Error('denied'))
            },
          }
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value })
  }, mode)
}

/** Expands one identifier, then proves copy success, copy failure with the
 * complete value selected for manual copy, no page/table widening, and focus
 * staying on the toggle when it collapses. */
async function exerciseIdentifier(
  page: Page,
  testInfo: TestInfo,
  cell: Locator,
  label: string,
  value: string,
  tableSlot: string,
) {
  const wrapper = page.locator('[data-slot="' + tableSlot + '"]').locator('xpath=..')
  const before = await wrapper.evaluate((element) => ({
    client: element.clientWidth,
    scroll: element.scrollWidth,
  }))
  const toggle = cell.getByRole('button', { name: 'Show full ' + label })
  await expect(toggle).toHaveAttribute('aria-expanded', 'false')
  await expect(cell.getByText(value, { exact: true })).toHaveCount(0)
  await activate(page, toggle, testInfo)

  const opened = cell.getByRole('button', { name: 'Hide full ' + label })
  await expect(opened).toHaveAttribute('aria-expanded', 'true')
  const panel = page.locator('#' + (await opened.getAttribute('aria-controls'))!.replace(/:/g, '\\:'))
  await expect(panel).toBeVisible()
  const full = panel.getByText(value, { exact: true })
  await expect(full).toBeVisible()
  // The complete value wraps inside its cell instead of widening anything.
  const fullBox = (await full.boundingBox())!
  const cellBox = (await cell.boundingBox())!
  expect(fullBox.x + fullBox.width).toBeLessThanOrEqual(cellBox.x + cellBox.width + 1)
  const after = await wrapper.evaluate((element) => ({
    client: element.clientWidth,
    scroll: element.scrollWidth,
  }))
  expect(after.scroll - after.client).toBeLessThanOrEqual(Math.max(0, before.scroll - before.client) + 1)
  await expectNoHorizontalOverflow(page)

  const copy = panel.getByRole('button', { name: 'Copy ' + label })
  await stubClipboard(page, 'resolve')
  await activate(page, copy, testInfo)
  await expect(panel.getByRole('status')).toHaveText('Copied to clipboard.')
  expect(await page.evaluate(() => (window as unknown as { copiedText?: string }).copiedText)).toBe(value)

  await stubClipboard(page, 'reject')
  await activate(page, copy, testInfo)
  await expect(panel.getByRole('status')).toHaveText(
    'Copy unavailable; select the full ' + label + ' shown here.',
  )
  await expect(full).toBeVisible()
  expect(await page.evaluate(() => window.getSelection()?.toString())).toBe(value)

  await stubClipboard(page, 'missing')
  await activate(page, copy, testInfo)
  await expect(panel.getByRole('status')).toHaveText(
    'Copy unavailable; select the full ' + label + ' shown here.',
  )
  await expectVisibleInteractiveTargets(page)

  await activate(page, opened, testInfo)
  await expect(toggle).toHaveAttribute('aria-expanded', 'false')
  await expect(panel).toHaveCount(0)
  if (!isTouch(testInfo)) await expect(toggle).toBeFocused()
}

test.describe('Admin follow-up: long identifiers and Sessions', () => {
  test.beforeEach(async ({ page }) => {
    await mockAdmin(page)
  })

  test('Networks Genesis hash opens, copies, and reports copy failure', async ({ page }, testInfo) => {
    await page.goto('/admin/networks')
    const row = page.locator('[data-slot="network-table"] tbody tr').first()
    await expect(row).toBeVisible()
    // The tuple is one Details step away (webui.md §8.7).
    await row.getByRole('button', { name: /^Details for / }).click()
    const cell = page.locator('[data-slot="network-table"] tr[data-slot="detail-row"] td')
    await expect(cell).toContainText('0x9f3c9f…9f3c')
    await exerciseIdentifier(page, testInfo, cell, 'Genesis hash', GENESIS, 'network-table')
  })

  test('Validators Observed P2P key opens, copies, and keeps a missing key Unknown', async ({ page }, testInfo) => {
    await page.goto('/admin/validators')
    // The identity coverage table opens from its summary (webui.md §8.7).
    await page.getByRole('button', { name: 'Show identity table' }).click()
    const table = page.locator('[data-slot="validator-identity-table"]')
    const observed = table
      .locator('tbody tr')
      .filter({ has: page.getByText('Observed Node', { exact: true }) })
    const cell = observed.locator('td[data-label="Observed P2P key"]')
    await expect(cell).toBeVisible()
    await exerciseIdentifier(page, testInfo, cell, 'Observed P2P key', P2P_KEY, 'validator-identity-table')

    const missing = table
      .locator('tbody tr')
      .filter({ hasText: 'Unobserved Node' })
      .locator('td[data-label="Observed P2P key"]')
    await expect(missing).toContainText('Unknown')
    await expect(missing.getByRole('button')).toHaveCount(0)
  })

  test('Sessions keeps labelled cards, confirmations and errors inside the viewport', async ({ page }, testInfo) => {
    await page.goto('/admin/access/sessions')
    await expect(page.getByRole('heading', { level: 1, name: 'Sessions' })).toBeVisible()
    const panel = page.locator('[data-slot="sessions-panel"]')
    await expect(panel.locator('[data-slot="sessions-count"]')).toHaveText('2 Sessions')
    const otherRow = panel.locator('tbody tr').filter({ hasText: LONG_USER })
    await expect(otherRow).toBeVisible()
    await expectNoHorizontalOverflow(page)

    const width = page.viewportSize()!.width
    const labels = await otherRow.locator('[data-label]').evaluateAll((cells) =>
      cells.map((cell) => ({
        label: cell.getAttribute('data-label'),
        before: getComputedStyle(cell, '::before').content,
        right: cell.getBoundingClientRect().right,
        rowRight: cell.closest('tr')!.getBoundingClientRect().right,
      })),
    )
    expect(labels.map((cell) => cell.label)).toEqual(['User', 'Client', 'Created', 'Last active', 'Expires', 'Action'])
    for (const cell of labels) {
      // Every cell stays inside its row/card.
      expect(cell.right).toBeLessThanOrEqual(cell.rowRight + 1)
      // Phones stack the row as a card and print each field label.
      if (width < 768) expect(cell.before).toBe('"' + cell.label + '"')
    }
    if (width >= 768) {
      // Table layout: no local horizontal scrolling is needed for the list.
      const scroll = await panel.locator('[data-slot="sessions-table"]').evaluate((element) => element.scrollWidth - element.clientWidth)
      expect(scroll).toBeLessThanOrEqual(1)
    }

    // Single-row revoke: confirmation stays in its card and focus lands on Cancel.
    await activate(page, otherRow.getByRole('button', { name: 'Revoke', exact: true }), testInfo)
    const confirm = otherRow.locator('[data-slot="session-revoke-confirm"]')
    await expect(confirm).toBeVisible()
    await expect(confirm).toContainText("Revoke now? The user's streams close immediately.")
    const confirmBox = (await confirm.boundingBox())!
    const rowBox = (await otherRow.boundingBox())!
    expect(confirmBox.x + confirmBox.width).toBeLessThanOrEqual(rowBox.x + rowBox.width + 1)
    await expectNoHorizontalOverflow(page)
    await expectVisibleInteractiveTargets(page)
    if (!isTouch(testInfo)) await expect(otherRow.getByRole('button', { name: 'Cancel' })).toBeFocused()
    await activate(page, otherRow.getByRole('button', { name: 'Cancel' }), testInfo)
    await expect(confirm).toHaveCount(0)
    if (!isTouch(testInfo)) await expect(otherRow.getByRole('button', { name: 'Revoke', exact: true })).toBeFocused()

    // Revoke-all-others keeps the current Session and is a distinct action.
    const others = panel.locator('[data-slot="sessions-others-actions"]')
    await activate(page, others.getByRole('button', { name: 'Revoke all other Sessions' }), testInfo)
    await expect(others).toContainText('The current Session stays active.')
    await expect(others.getByRole('button', { name: 'Confirm revoke all others' })).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await activate(page, others.getByRole('button', { name: 'Cancel' }), testInfo)

    // A rejected revoke shows the real error and keeps the list.
    await activate(page, otherRow.getByRole('button', { name: 'Revoke', exact: true }), testInfo)
    await activate(page, otherRow.getByRole('button', { name: 'Confirm revoke' }), testInfo)
    await expect(page.getByRole('alert')).toContainText('already revoked by another operator')
    await expect(otherRow).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })
})
