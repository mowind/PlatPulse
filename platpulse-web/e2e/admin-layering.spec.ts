import { test, expect } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

// Admin information layering (webui.md §8.7) against the seeded local e2e
// Server: lighter default views, one-step expansion, detail sections with
// preserved drafts, and deep links. Read-only apart from an unsaved draft.
const NODE_A = '0195f2a1-0014-4014-8014-000000000014'
const NODE_B = '0195f2a1-0015-4015-8015-000000000015'
const AGENT = '0195f2a1-0011-4011-8011-000000000011'

test.beforeEach(async ({ page }) => {
  await loginAs(page)
})

test('Overview previews five Nodes and Agents without Host, Spool or Clock diagnostics', async ({ page }) => {
  await page.goto('/admin')
  const nodePanel = page.locator('article[data-slot="overview-panel"]').filter({ has: page.getByRole('heading', { name: 'Node Health Summary' }) })
  const agentPanel = page.locator('article[data-slot="overview-panel"]').filter({ has: page.getByRole('heading', { name: 'Agent inventory' }) })
  await expect(nodePanel.getByRole('listitem').first()).toBeVisible({ timeout: 20_000 })
  expect(await nodePanel.getByRole('listitem').count()).toBeLessThanOrEqual(5)
  await expect(nodePanel.getByText(/^Showing \d+ of \d+ Active Nodes/)).toBeVisible()
  // The seeded inventory has more exceptional Nodes than the preview shows.
  await expect(nodePanel.locator('[data-slot="preview-hidden"]')).toContainText('Not shown:')
  await expect(agentPanel.getByRole('listitem').first()).toBeVisible()
  // Host resources, Spool capacity/queue, Clock and report sequence stay on the
  // Agent detail; a Spool state the summary must name may still appear.
  await expect(agentPanel).not.toContainText(/CPU|RX \/ TX|Spool capacity|queued bytes|Clock|Report #|Server liveness|Server-owned freshness/)
  await expect(agentPanel.getByText(/^Recorded history:/).first()).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('Nodes shows five columns and opens identity, endpoint and lifecycle in one step', async ({ page }) => {
  await page.goto('/admin/nodes')
  const table = page.locator('[data-slot="node-table"]')
  await expect(table).toBeVisible({ timeout: 20_000 })
  const row = table.locator('tbody tr:not([data-slot="detail-row"])').filter({ has: page.locator('a[href="/admin/nodes/' + NODE_A + '"]') })
  await expect(row).not.toContainText('ws://')
  const toggle = row.getByRole('button', { name: 'Details for Node A' })
  await toggle.click()
  await expect(toggle).toHaveAttribute('aria-expanded', 'true')
  const region = page.locator('[id="' + (await toggle.getAttribute('aria-controls')) + '"]')
  await expect(region).toContainText('ws://[REDACTED_IP]:****')
  await expect(region).toContainText('Inventory revision')
  await expect(region.getByRole('button', { name: /Show full\s*Node ID/ })).toBeVisible()
  // Node B keeps its identity mismatch visible without expanding.
  const rowB = table.locator('tbody tr').filter({ has: page.locator('a[href="/admin/nodes/' + NODE_B + '"]') })
  await expect(rowB.locator('[data-slot="node-identity-summary"]')).toContainText('Mismatched')
  await expectNoHorizontalOverflow(page)
})

test('More filters keeps an active Lifecycle filter named and clearable while collapsed', async ({ page }) => {
  await page.goto('/admin/nodes?lifecycle=retired')
  const chips = page.getByRole('list', { name: 'Active filters from More filters' })
  await expect(chips).toContainText('Lifecycle: Retired', { timeout: 20_000 })
  await expect(page.getByRole('button', { name: /More filters\s*1\s*active/ })).toHaveAttribute('aria-expanded', 'false')
  await chips.getByRole('button', { name: 'Clear Lifecycle: Retired' }).click()
  await expect(page).not.toHaveURL(/lifecycle=/)
  await page.goBack()
  await expect(page).toHaveURL(/lifecycle=retired/)
})

test('Node detail opens on Overview, switches sections, and keeps an unsaved draft', async ({ page }) => {
  await page.goto('/admin/nodes/' + NODE_A)
  await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible({ timeout: 20_000 })
  await expect(page.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true')
  await expect(page.getByRole('heading', { name: 'Metric history', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Permanently delete Node' })).toHaveCount(0)

  await page.getByRole('tab', { name: 'History' }).click()
  await expect(page).toHaveURL(/section=history/)
  await expect(page.getByRole('heading', { name: 'Metric history', exact: true })).toBeVisible()

  await page.getByRole('tab', { name: /^Manage/ }).click()
  await page.getByRole('button', { name: 'Edit' }).click()
  await page.getByRole('textbox', { name: 'Display name' }).fill('Unsaved layering draft')
  await expect(page.getByRole('tab', { name: /^Manage/ })).toContainText('Unsaved')
  await page.getByRole('tab', { name: /^Diagnostics/ }).click()
  await expect(page.getByRole('textbox', { name: 'Display name' })).toHaveCount(0)
  await expect(page.getByRole('heading', { name: 'RPC diagnostics' })).toBeVisible()
  await page.getByRole('tab', { name: /^Manage/ }).click()
  await expect(page.getByRole('textbox', { name: 'Display name' })).toHaveValue('Unsaved layering draft')
  await page.getByRole('button', { name: 'Cancel' }).click()
  await expectNoHorizontalOverflow(page)

  // A reload keeps the section.
  await page.reload()
  await expect(page.getByRole('tab', { name: /^Manage/ })).toHaveAttribute('aria-selected', 'true', { timeout: 20_000 })
})

test('Node detail summarizes a current finding above every section', async ({ page }) => {
  await page.goto('/admin/nodes/' + NODE_B)
  const findings = page.getByRole('list', { name: 'Current Node findings' })
  await expect(findings).toContainText('Network identity contradicts the Registry', { timeout: 20_000 })
  await findings.getByRole('button', { name: 'Open Diagnostics' }).first().click()
  await expect(page.getByRole('tab', { name: /^Diagnostics/ })).toHaveAttribute('aria-selected', 'true')
  await expect(page.getByText(/Contradicts the Registry:/)).toBeVisible()
})

test('Agent detail anchors open their sections and the Danger zone stays in Manage', async ({ page }) => {
  await page.goto('/admin/agents/' + AGENT)
  await expect(page.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true', { timeout: 20_000 })
  await expect(page.getByRole('heading', { name: 'Reporting and runtime' })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Danger zone' })).toHaveCount(0)
  await expect(page.getByText(/^0[1-8]$/)).toHaveCount(0)

  await page.goto('/admin/agents/' + AGENT + '#audit')
  await expect(page.getByRole('tab', { name: 'History' })).toHaveAttribute('aria-selected', 'true', { timeout: 20_000 })
  await expect(page.getByRole('heading', { name: 'Audit trail' })).toBeVisible()

  await page.goto('/admin/agents/' + AGENT + '#credentials')
  await expect(page.getByRole('tab', { name: /^Manage/ })).toHaveAttribute('aria-selected', 'true', { timeout: 20_000 })
  await expect(page.getByRole('heading', { name: 'Credential records' })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Danger zone' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('Agents rows summarize evidence and open the grouped details', async ({ page }) => {
  await page.goto('/admin/agents')
  const row = page
    .locator('[data-slot="agent-table"] tbody tr:not([data-slot="detail-row"])')
    .filter({ has: page.locator('a[href="/admin/agents/' + AGENT + '"]') })
  await expect(row).toBeVisible({ timeout: 20_000 })
  await expect(row).not.toContainText(/Server liveness|Server receipt time|Server validity|Accumulated recorded/)
  const toggle = row.locator('button[aria-controls^="agent-diagnostics-"]')
  await toggle.click()
  const region = page.locator('[id="' + (await toggle.getAttribute('aria-controls')) + '"]')
  await expect(region.getByRole('region', { name: 'Recorded evidence' })).toContainText('Recorded gap intervals')
  await expect(region.getByRole('region', { name: 'Nodes and credentials' })).toContainText('retained')
  await expectNoHorizontalOverflow(page)
})

test('Validators lead with the registry and keep identity coverage visible while collapsed', async ({ page }) => {
  await page.goto('/admin/validators')
  const identity = page.locator('[data-slot="validator-identity-section"]')
  await expect(identity.locator('[data-slot="identity-coverage"]')).toContainText(/evaluated · \d+ identified · \d+ unresolved/, { timeout: 20_000 })
  await expect(page.locator('[data-slot="validator-identity-table"]')).toHaveCount(0)
  await page.getByLabel('Search Validators and Nodes').fill('Node A')
  await expect(identity.locator('[data-slot="identity-search-matches"]')).toContainText('this search in the identity table')
  await identity.getByRole('button', { name: 'Show identity table' }).click()
  await expect(page.locator('[data-slot="validator-identity-table"]')).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('Networks keep the tuple one step away', async ({ page }) => {
  await page.goto('/admin/networks')
  const row = page.locator('[data-slot="network-table"] tbody tr').filter({ hasText: 'platon-e2e' }).first()
  await expect(row).toBeVisible({ timeout: 20_000 })
  await expect(row.getByRole('button', { name: /Show full\s*Genesis hash/ })).toHaveCount(0)
  await row.getByRole('button', { name: /^Details for / }).click()
  await expect(page.getByRole('button', { name: /Show full\s*Genesis hash/ }).first()).toBeVisible()
  await expectNoHorizontalOverflow(page)
})
