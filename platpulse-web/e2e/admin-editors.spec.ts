import { test, expect, type Page, type Route } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { expectNoHorizontalOverflow, loginAs } from './helpers'
import { findRepoRoot } from './repo-root'

// Admin Node / Network editor state round against the seeded local e2e
// Server: the read view follows the Server, Cancel discards a round, a
// confirmation is withdrawn by any change, and a slow write is sent once and
// keeps its editor locked across a section switch; a successful write and a
// failed read-back after it are stated apart, and the next successful read
// lifts the read-back notice. Every write here is a local Playwright route
// (delayed and failing, or answered as applied) and every read after an
// "applied" write is a local route fixture, so the shared seeded Server state
// is never changed. It runs on every fixed project, Light and
// Dark; ADMIN_EDITORS_EVIDENCE=1 additionally captures the edited panels at
// 390 and 1280.
const NODE_A = '0195f2a1-0014-4014-8014-000000000014'
const NETWORK = 'platon-e2e'
const evidence = Boolean(process.env.ADMIN_EDITORS_EVIDENCE)
const OUT = join(findRepoRoot(), 'docs/visual-migration/emerald/admin-editors')


/** A write that answers only when released, and then fails. */
function slowFailingWrite(page: Page, pattern: RegExp, method: string) {
  const calls: string[] = []
  let release: () => void = () => {}
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  void page.route(pattern, async (route: Route) => {
    if (route.request().method() !== method) return route.fallback()
    calls.push(route.request().postData() ?? '')
    await gate
    await route.fulfill({
      status: 503,
      contentType: 'application/json',
      body: JSON.stringify({ error: { code: 'unavailable', message: 'local fixture: write unavailable' } }),
    })
  })
  return { calls, release }
}

/** A write answered as applied by a local fixture, after which the detail
 *  read fails until `readable` is set and then serves the written value. The
 *  Server is never written; the detail reads pass through and are rewritten. */
async function appliedWriteThenUnreadable(
  page: Page,
  options: {
    write: RegExp
    read: RegExp
    answer: (body: Record<string, unknown>) => Record<string, unknown>
    rewrite: (detail: Record<string, unknown>, body: Record<string, unknown>) => Record<string, unknown>
  },
) {
  const state = { writes: [] as Record<string, unknown>[], readable: false }
  // No realtime refetch may stand in for the read under test.
  await page.route('**/api/admin/v1/events**', (route) => route.abort())
  await page.route(options.read, async (route) => {
    if (route.request().method() !== 'GET') return route.fallback()
    const written = state.writes.at(-1)
    if (written && !state.readable) {
      return route.fulfill({
        status: 503,
        contentType: 'application/json',
        body: JSON.stringify({ error: { code: 'unavailable', message: 'local fixture: read unavailable' } }),
      })
    }
    const response = await route.fetch()
    const detail = (await response.json()) as Record<string, unknown>
    return route.fulfill({ response, json: written ? options.rewrite(detail, written) : detail })
  })
  await page.route(options.write, async (route) => {
    if (route.request().method() !== 'PUT') return route.fallback()
    const body = JSON.parse(route.request().postData() ?? '{}') as Record<string, unknown>
    state.writes.push(body)
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(options.answer(body)) })
  })
  return state
}

async function shot(page: Page, selector: string, name: string, theme: string) {
  const width = page.viewportSize()?.width ?? 0
  // Region captures: one phone and one desktop width are enough for a
  // button-state and copy change.
  if (!evidence || ![390, 1280].includes(width)) return
  await mkdir(OUT, { recursive: true })
  await page.locator(selector).first().screenshot({ path: join(OUT, name + '-' + width + '-' + theme + '.png'), animations: 'disabled' })
}

for (const theme of ['light', 'dark'] as const) {
  test.describe(theme, () => {
    test.beforeEach(async ({ page }) => {
      await page.addInitScript((value) => localStorage.setItem('platpulse.themeMode', value), theme)
      await loginAs(page)
    })

    test('Node rename: one slow write, locked across a section switch, then a kept draft and error', async ({ page }) => {
      const write = slowFailingWrite(page, new RegExp('/api/admin/v1/nodes/' + NODE_A + '/metadata$'), 'PUT')
      await page.goto('/admin/nodes/' + NODE_A + '?section=manage')
      await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 20_000 })
      const panel = page.locator('[data-slot="card-x"]', { has: page.getByRole('heading', { name: 'Server-owned metadata' }) })

      await panel.getByRole('button', { name: 'Edit' }).click()
      const input = panel.getByLabel('Display name')
      await expect(input).toBeFocused()
      await expect(input).toHaveValue('Node A')
      await input.fill('Node A draft')
      await input.press('Enter')
      await expect(panel.getByText(/Rename this Node in the Server-owned metadata\?/)).toBeFocused()
      await panel.getByRole('button', { name: 'Confirm rename' }).click()

      const saving = panel.getByRole('button', { name: 'Saving…' })
      await expect(saving).toHaveAttribute('aria-disabled', 'true')
      await expect(saving).toBeFocused()
      await expect(panel.locator('form')).toHaveAttribute('aria-busy', 'true')
      await expect(input).toBeDisabled()
      await expect(panel.getByRole('button', { name: 'Cancel' })).toBeDisabled()
      await expect(panel.getByRole('button', { name: 'Keep editing' })).toBeDisabled()
      await shot(page, '[data-slot="card-x"]:has(h2:text-is("Server-owned metadata"))', 'node-rename-saving', theme)
      await saving.click({ force: true })
      await saving.press('Enter')
      await saving.press('Space')

      await page.getByRole('tab', { name: 'Overview' }).click()
      await expect(page).not.toHaveURL(/section=manage/)
      await page.getByRole('tab', { name: /^Manage/ }).click()
      await expect(panel.getByRole('button', { name: 'Saving…' })).toBeVisible()
      await panel.getByRole('button', { name: 'Saving…' }).click({ force: true })
      expect(write.calls).toHaveLength(1)
      expect(JSON.parse(write.calls[0])).toEqual({ displayName: 'Node A draft' })

      write.release()
      await expect(panel.getByRole('alert')).toHaveText('local fixture: write unavailable')
      await expect(input).toBeEnabled()
      await expect(input).toHaveValue('Node A draft')
      await expect(panel.getByRole('button', { name: 'Confirm rename' })).not.toHaveAttribute('aria-disabled', 'true')
      await expect(panel.getByRole('button', { name: 'Confirm rename' })).toBeFocused()
      await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
      await expect(panel.getByText(/Display name is now/)).toHaveCount(0)
      expect(write.calls).toHaveLength(1)
      await shot(page, '[data-slot="card-x"]:has(h2:text-is("Server-owned metadata"))', 'node-rename-failed', theme)

      await panel.getByRole('button', { name: 'Cancel' }).click()
      await expect(panel.getByRole('button', { name: 'Edit' })).toBeFocused()
      // The read view is the Server's name again, not the discarded draft.
      await expect(panel.locator('dd').first()).toHaveText(/^Node A\s*Edit$/)
      await expect(panel.getByRole('alert')).toHaveCount(0)
      await panel.getByRole('button', { name: 'Edit' }).click()
      await expect(panel.getByLabel('Display name')).toHaveValue('Node A')
      await expectNoHorizontalOverflow(page)
    })

    test('Network tuple: Cancel discards, a change withdraws the confirmation, and one slow write fails cleanly', async ({ page }) => {
      const write = slowFailingWrite(page, new RegExp('/api/admin/v1/networks/' + NETWORK + '$'), 'PUT')
      await page.goto('/admin/networks/' + NETWORK)
      await expect(page.getByRole('heading', { level: 1, name: /PlatON E2E Network/ })).toBeVisible({ timeout: 20_000 })
      const panel = page.locator('[data-slot="card-x"]', { has: page.getByRole('heading', { name: 'Expected identity tuple' }) })

      await panel.getByRole('button', { name: 'Edit tuple' }).click()
      await expect(panel.getByLabel('Display name')).toBeFocused()
      await expect(panel.getByRole('button', { name: 'Close editor' })).toHaveCount(0)
      await panel.getByLabel('Display name').fill('Cancelled draft')
      await panel.getByRole('button', { name: 'Save tuple' }).click()
      await expect(panel.getByRole('button', { name: 'Confirm tuple update' })).toBeVisible()
      // Any change withdraws the confirmation.
      await panel.getByLabel('P2P network ID').fill('9')
      await expect(panel.getByRole('button', { name: 'Confirm tuple update' })).toHaveCount(0)
      await expect(panel.getByRole('button', { name: 'Save tuple' })).toBeVisible()
      // The final step keeps the form's own validity rules.
      await panel.getByLabel('Chain ID').fill('')
      await panel.getByRole('button', { name: 'Save tuple' }).click()
      await expect(panel.getByRole('button', { name: 'Confirm tuple update' })).toHaveCount(0)

      // Cancel discards the round; reopening reads the Server tuple again.
      await panel.getByRole('button', { name: 'Cancel' }).click()
      await expect(panel.getByRole('button', { name: 'Edit tuple' })).toBeFocused()
      await expect(panel.getByText('PlatON E2E Network', { exact: true })).toBeVisible()
      await panel.getByRole('button', { name: 'Edit tuple' }).click()
      await expect(panel.getByLabel('Display name')).toHaveValue('PlatON E2E Network')
      await expect(panel.getByLabel('Chain ID')).toHaveValue('210425')
      await expect(panel.getByRole('button', { name: 'Confirm tuple update' })).toHaveCount(0)

      await panel.getByLabel('Display name').fill('Slow tuple draft')
      await panel.getByRole('button', { name: 'Save tuple' }).click()
      await expect(panel.getByText(/Block History already rejected on a mismatch is not reclaimed/)).toBeVisible()
      await panel.getByRole('button', { name: 'Confirm tuple update' }).click()
      const saving = panel.getByRole('button', { name: 'Saving…' })
      await expect(saving).toHaveAttribute('aria-disabled', 'true')
      await expect(saving).toBeFocused()
      await expect(panel.getByLabel('Display name')).toBeDisabled()
      await expect(panel.getByRole('button', { name: 'Cancel' })).toBeDisabled()
      await shot(page, '[data-slot="card-x"]:has(h2:text-is("Expected identity tuple"))', 'network-tuple-saving', theme)
      await saving.click({ force: true })
      await saving.press('Enter')
      // The rest of the page stays usable while this editor is locked.
      await expect(page.getByRole('link', { name: 'All Networks' })).toBeEnabled()
      expect(write.calls).toHaveLength(1)
      expect(JSON.parse(write.calls[0])).toMatchObject({ displayName: 'Slow tuple draft', chainId: 210425 })

      write.release()
      await expect(panel.getByRole('alert')).toHaveText('local fixture: write unavailable')
      await expect(panel.getByLabel('Display name')).toBeEnabled()
      await expect(panel.getByLabel('Display name')).toHaveValue('Slow tuple draft')
      await expect(panel.getByRole('button', { name: 'Confirm tuple update' })).toBeFocused()
      await expect(page.getByRole('heading', { level: 1, name: /PlatON E2E Network/ })).toBeVisible()
      expect(write.calls).toHaveLength(1)
      await shot(page, '[data-slot="card-x"]:has(h2:text-is("Expected identity tuple"))', 'network-tuple-failed', theme)
      await panel.getByRole('button', { name: 'Cancel' }).click()
      await expect(panel.getByText('PlatON E2E Network', { exact: true })).toBeVisible()
      await expectNoHorizontalOverflow(page)
    })

    test('Node rename: the written name and a failed read-back stand apart until the Node is read again', async ({ page }) => {
      const fixture = await appliedWriteThenUnreadable(page, {
        write: new RegExp('/api/admin/v1/nodes/' + NODE_A + '/metadata$'),
        read: new RegExp('/api/admin/v1/nodes/' + NODE_A + '(\\?.*)?$'),
        answer: (body) => ({ nodeId: NODE_A, displayName: body.displayName }),
        rewrite: (detail, body) => ({ ...detail, display_name: body.displayName }),
      })
      await page.goto('/admin/nodes/' + NODE_A + '?section=manage')
      await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 20_000 })
      const panel = page.locator('[data-slot="card-x"]', { has: page.getByRole('heading', { name: 'Server-owned metadata' }) })

      await panel.getByRole('button', { name: 'Edit' }).click()
      await panel.getByLabel('Display name').fill('Node A written')
      await panel.getByRole('button', { name: 'Save' }).click()
      await panel.getByRole('button', { name: 'Confirm rename' }).click()

      const written = panel.getByText('Display name is now "Node A written".', { exact: true })
      await expect(written).toBeVisible()
      await expect(written).toHaveClass(/text-success/)
      const notice = panel.locator('[data-slot="read-back-failed"]')
      await expect(notice).toContainText('could not be read back')
      await expect(notice).not.toHaveClass(/text-success/)
      await expect(notice).not.toHaveAttribute('role', /.+/)
      // One page-level refresh alert, not a second copy in the editor.
      await expect(page.getByText(/Failed to refresh; showing the last/)).toHaveCount(1)
      await expect(panel.getByText(/save failed/i)).toHaveCount(0)
      // The read view is still the last Server value; the editor is closed.
      await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
      await expect(panel.getByRole('textbox')).toHaveCount(0)
      await expect(panel.getByRole('button', { name: 'Edit' })).toBeFocused()
      await expectNoHorizontalOverflow(page)
      await shot(page, '[data-slot="card-x"]:has(h2:text-is("Server-owned metadata"))', 'node-rename-unread', theme)

      fixture.readable = true
      await notice.getByRole('button', { name: 'Read the Node again' }).click()
      await expect(page.getByRole('heading', { level: 1, name: 'Node A written' })).toBeVisible()
      await expect(notice).toHaveCount(0)
      await expect(page.getByText(/Failed to refresh; showing the last/)).toHaveCount(0)
      await expect(written).toBeVisible()
      await expect(panel.getByRole('button', { name: 'Edit' })).toBeFocused()
      expect(fixture.writes).toEqual([{ displayName: 'Node A written' }])
    })

    test('Network tuple: the applied update and a failed read-back stand apart until the Network is read again', async ({ page }) => {
      const fixture = await appliedWriteThenUnreadable(page, {
        write: new RegExp('/api/admin/v1/networks/' + NETWORK + '$'),
        read: new RegExp('/api/admin/v1/networks/' + NETWORK + '(\\?.*)?$'),
        answer: (body) => ({ networkKey: NETWORK, displayName: body.displayName }),
        rewrite: (detail, body) => ({ ...detail, display_name: body.displayName }),
      })
      await page.goto('/admin/networks/' + NETWORK)
      await expect(page.getByRole('heading', { level: 1, name: /PlatON E2E Network/ })).toBeVisible({ timeout: 20_000 })
      const panel = page.locator('[data-slot="card-x"]', { has: page.getByRole('heading', { name: 'Expected identity tuple' }) })

      await panel.getByRole('button', { name: 'Edit tuple' }).click()
      await panel.getByLabel('Display name').fill('PlatON E2E written')
      await panel.getByRole('button', { name: 'Save tuple' }).click()
      await panel.getByRole('button', { name: 'Confirm tuple update' }).click()

      const written = panel.getByText('Updated PlatON E2E written.', { exact: true })
      await expect(written).toBeVisible()
      await expect(written).toHaveClass(/text-success/)
      const notice = panel.locator('[data-slot="read-back-failed"]')
      await expect(notice).toContainText('could not be read back')
      await expect(notice).not.toHaveClass(/text-success/)
      await expect(page.getByText(/Failed to refresh; showing the last/)).toHaveCount(1)
      await expect(panel.getByText('PlatON E2E Network', { exact: true })).toBeVisible()
      await expect(panel.getByRole('textbox')).toHaveCount(0)
      await expectNoHorizontalOverflow(page)
      await shot(page, '[data-slot="card-x"]:has(h2:text-is("Expected identity tuple"))', 'network-tuple-unread', theme)

      fixture.readable = true
      await notice.getByRole('button', { name: 'Read the Network again' }).click()
      await expect(page.getByRole('heading', { level: 1, name: /PlatON E2E written/ })).toBeVisible()
      await expect(notice).toHaveCount(0)
      await expect(page.getByText(/Failed to refresh; showing the last/)).toHaveCount(0)
      await expect(written).toBeVisible()
      await expect(panel.getByRole('button', { name: 'Edit tuple' })).toBeFocused()
      expect(fixture.writes).toHaveLength(1)
      expect(fixture.writes[0]).toMatchObject({ displayName: 'PlatON E2E written', chainId: 210425 })
    })

    test('Network tuple: a draft begun before the read-back is kept but not sent until the Network is read', async ({ page }) => {
      const fixture = await appliedWriteThenUnreadable(page, {
        write: new RegExp('/api/admin/v1/networks/' + NETWORK + '$'),
        read: new RegExp('/api/admin/v1/networks/' + NETWORK + '(\\?.*)?$'),
        answer: (body) => ({ networkKey: NETWORK, displayName: body.displayName }),
        rewrite: (detail, body) => ({ ...detail, display_name: body.displayName, chain_id: body.chainId }),
      })
      await page.goto('/admin/networks/' + NETWORK)
      await expect(page.getByRole('heading', { level: 1, name: /PlatON E2E Network/ })).toBeVisible({ timeout: 20_000 })
      const panel = page.locator('[data-slot="card-x"]', { has: page.getByRole('heading', { name: 'Expected identity tuple' }) })

      // First round: an identity field; the write is applied, the read-back fails.
      await panel.getByRole('button', { name: 'Edit tuple' }).click()
      await panel.getByLabel('Chain ID').fill('210426')
      await panel.getByRole('button', { name: 'Save tuple' }).click()
      await panel.getByRole('button', { name: 'Confirm tuple update' }).click()
      await expect(panel.getByText('Updated PlatON E2E Network.', { exact: true })).toBeVisible()
      const notice = panel.locator('[data-slot="read-back-failed"]')
      await expect(notice).toContainText('could not be read back')

      // A new round from the tuple this page last read; only the name changes.
      await panel.getByRole('button', { name: 'Edit tuple' }).click()
      await expect(panel.getByLabel('Chain ID')).toHaveValue('210425')
      const name = panel.getByLabel('Display name')
      await name.fill('Renamed after write')
      const save = panel.getByRole('button', { name: 'Save tuple' })
      await expect(save).toBeDisabled()
      const reason = panel.locator('[data-slot="tuple-read-back-pending"]')
      await expect(reason).toBeVisible()
      await expect(reason).toContainText('Save tuple is unavailable until the latest Network is read successfully')
      await expect(save).toHaveAttribute('aria-describedby', (await reason.getAttribute('id')) ?? 'missing')
      await save.click({ force: true })
      await name.press('Enter')
      await panel.locator('form').evaluate((form: HTMLFormElement) => form.requestSubmit())
      await expect(panel.getByRole('button', { name: 'Confirm tuple update' })).toHaveCount(0)
      await expect(name).toHaveValue('Renamed after write')
      await expectNoHorizontalOverflow(page)
      await shot(page, '[data-slot="card-x"]:has(h2:text-is("Expected identity tuple"))', 'network-tuple-read-back-pending', theme)
      expect(fixture.writes).toHaveLength(1)

      // Reading again only reads: the draft stays and the Server-moved rule applies.
      fixture.readable = true
      await notice.getByRole('button', { name: 'Read the Network again' }).click()
      await expect(notice).toHaveCount(0)
      await expect(reason).toHaveCount(0)
      await expect(name).toHaveValue('Renamed after write')
      await expect(panel.getByLabel('Chain ID')).toHaveValue('210425')
      const moved = panel.locator('[data-slot="tuple-server-moved"]')
      await expect(moved).toContainText('changed on the Server while you were editing')
      await expect(moved).toContainText('Chain ID 210426')
      await expect(save).toBeDisabled()
      await name.press('Enter')
      await expect(panel.getByRole('button', { name: 'Confirm tuple update' })).toHaveCount(0)
      expect(fixture.writes).toHaveLength(1)

      // From the latest tuple, the next update carries the first one's identity fields.
      await panel.getByRole('button', { name: 'Cancel' }).click()
      await panel.getByRole('button', { name: 'Edit tuple' }).click()
      await expect(panel.getByLabel('Chain ID')).toHaveValue('210426')
      await name.fill('Renamed after write')
      await panel.getByRole('button', { name: 'Save tuple' }).click()
      await panel.getByRole('button', { name: 'Confirm tuple update' }).click()
      await expect(panel.getByText('Updated Renamed after write.', { exact: true })).toBeVisible()
      expect(fixture.writes).toHaveLength(2)
      expect(fixture.writes[1]).toMatchObject({ displayName: 'Renamed after write', chainId: 210426 })
    })
  })
}
