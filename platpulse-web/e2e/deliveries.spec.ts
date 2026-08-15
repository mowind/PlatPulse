import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

async function openDeliveries(page: Parameters<typeof loginAs>[0], section: string) {
  await loginAs(page)
  await page.getByRole('link', { name: 'Admin', exact: true }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible({ timeout: 15_000 })
  const menu = page.getByRole('button', { name: 'Menu' })
  const usesDrawer = await menu.isVisible()
  if (usesDrawer) await menu.click()
  await page.getByRole('link', { name: section }).click()
  if (usesDrawer) {
    await expect(page.getByRole('navigation', { name: 'Admin' })).not.toBeInViewport()
  }
}

const DEAD_LETTER_DELIVERY = '0195f2a1-0051-4051-8051-000000000051'
const RETRY_TARGET_DELIVERY = '0195f2a1-0055-4055-8055-000000000055'
const SUPPRESSED_DELIVERY = '0195f2a1-0053-4053-8053-000000000053'
const DELIVERED_DELIVERY = '0195f2a1-0052-4052-8052-000000000052'

/** The Outbox row whose Delivery link targets a specific id. */
function deliveryRow(page: Parameters<typeof loginAs>[0], deliveryId: string) {
  return page
    .locator('tr')
    .filter({ has: page.locator(`a[href="/admin/alerts/deliveries/${deliveryId}"]`) })
}

test.describe('Notification delivery (PAGE-ADMIN-DELIVERIES/DELIVERY/CHANNELS)', () => {
  test('channels are configured with redacted destination and provider reference', async ({ page }) => {
    await openDeliveries(page, 'Channels')
    await expect(page.getByRole('heading', { level: 1, name: 'Channels' })).toBeVisible({ timeout: 15_000 })

    const row = page.getByRole('row', { name: /telegram/ })
    await expect(row).toBeVisible({ timeout: 15_000 })
    await expect(row).toContainText('telegram')
    await expect(row).toContainText('****4321')
    await expect(row).toContainText('telegram-token')
    await expect(row).toContainText('2 attempts · 1s backoff')
    // Raw provider secrets and the full destination never reach the page.
    await expect(page.getByText('fake-e2e-telegram-token')).toHaveCount(0)
    await expect(page.getByText('987654321')).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('sends a test notification: separate Event, audited, reported outcome', async ({ page }) => {
    await openDeliveries(page, 'Channels')
    await page.getByRole('row', { name: /telegram/ }).getByRole('link', { name: 'telegram' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Channel telegram' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByRole('button', { name: 'Send test notification' })).toBeVisible()

    page.on('dialog', (dialog) => void dialog.accept())
    await page.getByRole('button', { name: 'Send test notification' }).click()
    // The test Event is sent synchronously through the same at-least-once
    // path; with the fixture token the provider fails, and the response
    // reports the failed Delivery state (never a business Incident).
    await expect(page.getByText(/Test Event .* sent — Delivery .*: failed/)).toBeVisible({ timeout: 15_000 })

    // The test Event is clearly separate from Incidents and appears under
    // the Test filter on the Deliveries page.
    await page.getByRole('link', { name: '← Channels' }).click()
    const menu = page.getByRole('button', { name: 'Menu' })
    const usesDrawer = await menu.isVisible()
    if (usesDrawer) await menu.click()
    await page.getByRole('link', { name: 'Deliveries' }).click()
    if (usesDrawer) {
      await expect(page.getByRole('navigation', { name: 'Admin' })).not.toBeInViewport()
    }
    await expect(page.getByRole('heading', { level: 1, name: 'Deliveries' })).toBeVisible({ timeout: 15_000 })
    await page.getByLabel('Kind').selectOption({ label: 'Test' })
    await expect(page.getByText(/Test notification via telegram/).first()).toBeVisible({ timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })

  test('lists the Outbox with per-channel states and redacted destinations', async ({ page }) => {
    await openDeliveries(page, 'Deliveries')
    await expect(page.getByRole('heading', { level: 1, name: 'Deliveries' })).toBeVisible({ timeout: 15_000 })

    // Dead letter filter surfaces the exhausted Delivery with its attempts.
    await page.getByLabel('State').selectOption({ label: 'Dead letter' })
    const dead = deliveryRow(page, DEAD_LETTER_DELIVERY)
    await expect(dead).toBeVisible({ timeout: 15_000 })
    await expect(dead).toContainText('telegram')
    await expect(dead).toContainText('****4321')
    await expect(dead).toContainText('Dead letter')
    await expect(dead).toContainText('2')

    // One failed destination never erases successful Delivery state: the
    // delivered row keeps its own state under the filter.
    await page.getByLabel('State').selectOption({ label: 'Delivered' })
    const delivered = deliveryRow(page, DELIVERED_DELIVERY)
    await expect(delivered).toBeVisible({ timeout: 15_000 })
    await expect(delivered).toContainText('Delivered')

    // Suppressed rows stay visible and are never retryable.
    await page.getByLabel('State').selectOption({ label: 'Suppressed' })
    const suppressed = deliveryRow(page, SUPPRESSED_DELIVERY)
    await expect(suppressed).toBeVisible({ timeout: 15_000 })
    await expect(suppressed).toContainText('Suppressed')

    // Events are distinguishable from Delivery attempts.
    await page.getByLabel('State').selectOption({ label: 'All states' })
    await expect(page.getByText(/Incident opened: node\.rpc_unreachable/).first()).toBeVisible({ timeout: 15_000 })
    // Redaction holds on the whole page: no raw destination, no token.
    await expect(page.getByText('987654321')).toHaveCount(0)
    await expect(page.getByText('fake-e2e-telegram-token')).toHaveCount(0)
    await expectNoHorizontalOverflow(page)
  })

  test('manual retry re-arms the same Delivery and never duplicates the Event', async ({ page }) => {
    await openDeliveries(page, 'Deliveries')
    await page.getByLabel('State').selectOption({ label: 'Dead letter' })
    const row = deliveryRow(page, RETRY_TARGET_DELIVERY)
    await expect(row).toBeVisible({ timeout: 15_000 })
    await row
      .locator(`a[href="/admin/alerts/deliveries/${RETRY_TARGET_DELIVERY}"]`)
      .first()
      .click()

    await expect(page.getByRole('heading', { level: 1, name: `Delivery ${RETRY_TARGET_DELIVERY.slice(0, 8)}` })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Dead letter')).toBeVisible()
    // Attempt history with redacted provider results and Retry-After.
    await expect(page.getByText('telegram_network_error').first()).toBeVisible()
    await expect(page.getByText('telegram_api_error 400')).toBeVisible()
    await expect(page.getByText('****4321')).toBeVisible()
    // The Event belongs to the same delivery; no duplicate Event exists.
    await expect(page.getByText(/Incident opened: node\.process_not_running/)).toBeVisible()

    // Manual retry: confirmed, re-arms the row, and the worker records a
    // new attempt that exhausts the bounded retries again (Dead letter).
    // The suite runs this test on every project in parallel against one
    // Server, so the attempt number is shared state: assert the behavior,
    // not the exact counter.
    page.on('dialog', (dialog) => void dialog.accept())
    await page.getByRole('button', { name: 'Retry delivery' }).click()
    // A parallel project may already have re-armed the same Delivery: the
    // Server either queues the retry or refuses the duplicate (409), and
    // both outcomes prove the same-Delivery contract.
    await expect(
      page
        .getByText(/Retry queued — attempt \d+/)
        .or(page.getByText(/already queued or in flight/)),
    ).toBeVisible({ timeout: 15_000 })
    // The same Delivery row is reused: still exactly one row for this Event.
    await expect(page.getByText('Delivery ' + RETRY_TARGET_DELIVERY.slice(0, 8))).toBeVisible()
    // The worker's next attempt fails and re-dead-letters (bounded retry);
    // the attempt history keeps growing, never duplicating the Event.
    await expect(page.getByText('Dead letter').first()).toBeVisible({ timeout: 45_000 })
    await expect
      .poll(async () => page.locator('table.node-table tbody tr').count(), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(3)
    // The Event count is unchanged: retry never created a duplicate Event.
    await page.getByRole('link', { name: '← Deliveries' }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Deliveries' })).toBeVisible({ timeout: 15_000 })
    await page.getByLabel('Kind').selectOption({ label: 'All kinds' })
    const events = page.locator('ul.node-list li')
    await expect(events.filter({ hasText: 'node.process_not_running' })).toHaveCount(1, { timeout: 15_000 })
    await expectNoHorizontalOverflow(page)
  })

  test('suppressed Deliveries are not retryable', async ({ page }) => {
    await openDeliveries(page, 'Deliveries')
    await page.getByLabel('State').selectOption({ label: 'Suppressed' })
    const row = deliveryRow(page, SUPPRESSED_DELIVERY)
    await expect(row).toBeVisible({ timeout: 15_000 })
    await row
      .locator(`a[href="/admin/alerts/deliveries/${SUPPRESSED_DELIVERY}"]`)
      .first()
      .click()

    await expect(page.getByRole('heading', { level: 1, name: 'Delivery 0195f2a1' })).toBeVisible({ timeout: 15_000 })
    await expect(page.getByText('Suppressed').first()).toBeVisible()
    await expect(page.getByText(/suppressed by a Silence or Maintenance Window/)).toBeVisible()
    await expect(page.getByRole('button', { name: 'Retry delivery' })).toBeDisabled()
    await expectNoHorizontalOverflow(page)
  })
})
