import { test, expect, type Locator, type Page } from '@playwright/test'
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

const NODE_C = '0195f2a1-0016-4016-8016-000000000016'
const NODE_D = '0195f2a1-0017-4017-8017-000000000017'
const NODE_E = '0195f2a1-0018-4018-8018-000000000018'
const LONG_NODE_NAME =
  'Archival validator sentinel with an unusually long operator-assigned display name for layout review'

/** The real Server response with Node C's display name lengthened (the same
 *  local fixture the layering evidence uses); no Server state changes. */
async function lengthenNodeC(page: Page) {
  await page.route(/\/api\/admin\/v1\/nodes(\?|$)/, async (route) => {
    const response = await route.fetch()
    const body = (await response.json()) as Array<Record<string, unknown>>
    for (const node of body) if (node.node_id === NODE_C) node.display_name = LONG_NODE_NAME
    await route.fulfill({ response, json: body })
  })
}

function nodeRow(page: Page, nodeId: string) {
  return page
    .locator('[data-slot="node-table"] tbody tr:not([data-slot="detail-row"])')
    .filter({ has: page.locator('a[href="/admin/nodes/' + nodeId + '"]') })
}

type Box = { x: number; y: number; width: number; height: number }
const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width - 0.5 && b.x < a.x + a.width - 0.5 && a.y < b.y + b.height - 0.5 && b.y < a.y + a.height - 0.5

/** Every point near the edges of a control hits that control itself, so no
 *  neighbour's target is drawn over it (and it over no neighbour). */
async function expectOwnHitArea(control: Locator) {
  // Centre it so the sticky Admin header never covers the probe points.
  await control.evaluate((element) => element.scrollIntoView({ block: 'center' }))
  const misses = await control.evaluate((element) => {
    const rect = element.getBoundingClientRect()
    const points = [
      [rect.left + rect.width / 2, rect.top + 2],
      [rect.left + rect.width / 2, rect.bottom - 2],
      [rect.left + 2, rect.top + rect.height / 2],
      [rect.right - 2, rect.top + rect.height / 2],
    ]
    return points
      .filter(([x, y]) => !element.contains(document.elementFromPoint(x, y)))
      .map(([x, y]) => `${Math.round(x)},${Math.round(y)} -> ${document.elementFromPoint(x, y)?.outerHTML.slice(0, 80)}`)
  })
  expect(misses, 'points inside the control hit it').toEqual([])
}

test('Node and Network links keep separate 44px targets without overlapping hit areas', async ({ page }) => {
  await lengthenNodeC(page)
  await page.goto('/admin/nodes')
  await expect(page.locator('[data-slot="node-table"]')).toBeVisible({ timeout: 20_000 })
  for (const nodeId of [NODE_A, NODE_B, NODE_C, NODE_D]) {
    const row = nodeRow(page, nodeId)
    const name = row.locator('[data-slot="node-name-link"]')
    const network = row.locator('[data-slot="node-network-link"]')
    const details = row.getByRole('button', { name: /^Details for / })
    const [nameBox, networkBox, detailsBox] = [await name.boundingBox(), await network.boundingBox(), await details.boundingBox()]
    for (const box of [nameBox!, networkBox!, detailsBox!]) {
      expect(Math.round(box.height)).toBeGreaterThanOrEqual(44)
      expect(Math.round(box.width)).toBeGreaterThanOrEqual(44)
    }
    // Stacked groups, not pulled over each other.
    expect(networkBox!.y).toBeGreaterThanOrEqual(nameBox!.y + nameBox!.height - 0.5)
    expect(overlaps(nameBox!, networkBox!)).toBe(false)
    expect(overlaps(nameBox!, detailsBox!)).toBe(false)
    expect(overlaps(networkBox!, detailsBox!)).toBe(false)
    await expectOwnHitArea(name)
    await expectOwnHitArea(network)
    await expectOwnHitArea(details)
  }
  // The long name wraps inside its cell (a taller target), never clipped.
  const longName = nodeRow(page, NODE_C).locator('[data-slot="node-name-link"]')
  const longBox = (await longName.boundingBox())!
  const cellBox = (await nodeRow(page, NODE_C).locator('[data-label="Node"]').boundingBox())!
  expect(longBox.x + longBox.width).toBeLessThanOrEqual(cellBox.x + cellBox.width + 1)
  expect(await longName.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true)
  // A click on the Network link opens the Network, not the Node above it.
  await nodeRow(page, NODE_A).locator('[data-slot="node-network-link"]').click()
  await expect(page).toHaveURL(/\/admin\/networks\/platon-e2e$/)
  await expectNoHorizontalOverflow(page)
})

test('Nodes phone cards group name, Network, Health beside Freshness, and Head beside Details', async ({ page }) => {
  await lengthenNodeC(page)
  await page.goto('/admin/nodes')
  await expect(page.locator('[data-slot="node-table"]')).toBeVisible({ timeout: 20_000 })
  const width = page.viewportSize()!.width
  const rowA = nodeRow(page, NODE_A)
  if (width >= 768) {
    // Tablet and desktop keep the five-column table.
    await expect(page.locator('[data-slot="node-table"] thead th')).toHaveCount(5)
    expect(await rowA.evaluate((row) => getComputedStyle(row).display)).toBe('table-row')
    await expectNoHorizontalOverflow(page)
    return
  }
  for (const nodeId of [NODE_A, NODE_B, NODE_C, NODE_D, NODE_E]) {
    const row = nodeRow(page, nodeId)
    const box = async (label: string) => (await row.locator('[data-label="' + label + '"]').boundingBox())!
    const [rowBox, node, health, freshness, head, details] = [
      (await row.boundingBox())!, await box('Node'), await box('Health'), await box('Freshness'), await box('Head / Sync'), await box('Details'),
    ]
    // Group 1/2: the Node cell spans the card.
    expect(node.width).toBeGreaterThan(rowBox.width * 0.8)
    // Group 3: Health and Freshness side by side.
    expect(Math.abs(health.y - freshness.y)).toBeLessThanOrEqual(1)
    expect(health.x + health.width).toBeLessThanOrEqual(freshness.x + 0.5)
    expect(health.y).toBeGreaterThanOrEqual(node.y + node.height - 0.5)
    // Group 4: Head / Sync and Details share the last line.
    expect(head.y).toBeGreaterThanOrEqual(Math.max(health.y + health.height, freshness.y + freshness.height) - 0.5)
    expect(details.y).toBeLessThan(head.y + head.height)
    expect(head.x + head.width).toBeLessThanOrEqual(details.x + 0.5)
    // Each dimension keeps its own visible label.
    for (const label of ['Health', 'Freshness', 'Head / Sync']) {
      const text = await row.locator('[data-label="' + label + '"]').evaluate((cell) => getComputedStyle(cell, '::before').content)
      expect(text).toBe('"' + label + '"')
    }
  }
  // Exceptions keep their reasons; normal rows stay short.
  const rowB = nodeRow(page, NODE_B)
  await expect(rowB.locator('[data-slot="node-identity-summary"]')).toContainText('Mismatched')
  await expect(rowB.locator('[data-label="Freshness"]')).toContainText('Stale')
  await expect(rowB.locator('[data-label="Freshness"]')).toContainText('Last good values')
  await expect(rowB.locator('[data-label="Health"] small')).not.toHaveText('')
  await expect(rowA.locator('[data-label="Health"] small')).toHaveCount(0)
  await expect(nodeRow(page, NODE_E).locator('[data-label="Health"]')).toContainText(/unknown/i)
  await expect(nodeRow(page, NODE_E).locator('[data-slot="node-identity-summary"]')).toContainText('Identity')
  await expect(nodeRow(page, NODE_D).locator('[data-slot="node-lifecycle-marker"]')).toHaveText('Retired')
  // Expanding keeps every field, the ID disclosure and copy.
  const toggle = nodeRow(page, NODE_B).getByRole('button', { name: /^Details for / })
  await toggle.click()
  const region = page.locator('[id="' + (await toggle.getAttribute('aria-controls')) + '"]')
  for (const field of ['Node ID', 'Redacted RPC Endpoint', 'Identity disposition', 'Visibility', 'Lifecycle', 'Resync state', 'Health reason', 'Lifecycle guidance']) {
    await expect(region).toContainText(field)
  }
  await region.getByRole('button', { name: /Show full\s*Node ID/ }).click()
  await expect(region.getByRole('button', { name: 'Copy Node ID' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('the Agent Details toggle sits below its summary text without covering it', async ({ page }) => {
  await page.goto('/admin/agents')
  const row = page
    .locator('[data-slot="agent-table"] tbody tr:not([data-slot="detail-row"])')
    .filter({ has: page.locator('a[href="/admin/agents/' + AGENT + '"]') })
  await expect(row).toBeVisible({ timeout: 20_000 })
  const summary = row.locator('[data-slot="agent-evidence-summary"]')
  const toggle = row.locator('button[aria-controls^="agent-diagnostics-"]')
  const toggleBox = (await toggle.boundingBox())!
  expect(Math.round(toggleBox.height)).toBeGreaterThanOrEqual(44)
  for (const text of await summary.locator(':scope > span').all()) {
    const box = (await text.boundingBox())!
    expect(overlaps(box, toggleBox)).toBe(false)
  }
  await expectOwnHitArea(toggle)
  const identity = row.locator('a[href="/admin/agents/' + AGENT + '"]')
  await expectOwnHitArea(identity)
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
