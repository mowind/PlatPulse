import { expect, test, type Locator, type Page } from '@playwright/test'
import type {
  NotificationDeliveriesResponse,
  NotificationDeliveryDetail,
  NotificationEventsResponse,
  NotificationRequestResult,
} from '../src/api/generated'
import {
  expectFocusedElementHasVisibleFocus,
  expectNoHorizontalOverflow,
  expectVisibleInteractiveTargets,
} from './helpers'
import {
  HARNESS_OWNER_PASSWORD,
  HARNESS_OWNER_USERNAME,
  HARNESS_TELEGRAM_CHAT_ID,
  HARNESS_TELEGRAM_TOKEN,
  startDisposableServer,
} from './server-harness'

/**
 * Controlled Telegram test acceptance (issue #206, part of #202 Stage 1).
 * Every test owns one throwaway Server, port, and temporary SQLite database,
 * and drives the production WebUI build against real Server HTTP: no mock, no
 * test-only seam, and no jsdom.
 *
 * `development = true` keeps the harness off the network by swapping the
 * provider for the fixed-failure `DevNullProvider`, so a driven test always
 * ends `failed` with a 401 provider result. That is the point: the acceptance
 * criterion is the *command* semantics (accept once, deduplicate, cool down,
 * record, reconcile), not a Telegram delivery that a CI machine cannot make.
 * A deduplicated replay must therefore be proven by the Server recording
 * exactly one Event and Delivery, not by a message arriving.
 *
 * Process-heavy flows run once on desktop-1280. The second test crosses the
 * five fixed viewports with both themes in one logged-in touch-capable context
 * so the matrix does not multiply Server boots or logins.
 */

const VIEWPORTS = [
  { width: 360, height: 800 },
  { width: 390, height: 844 },
  { width: 768, height: 1024 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
]

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

/**
 * A retry re-arms the delivery as unsent work, so the recorded result carries
 * 'pending' until the worker claims it. Every state below means "the Server
 * accepted one retry"; 'suppressed' and 'cancelled' are never in it.
 */
const POST_RETRY_STATES = ['pending', 'retry_scheduled', 'in_flight', 'failed']

async function loginToDisposableServer(page: Page, baseUrl: string) {
  await page.context().clearCookies()
  await page.goto(baseUrl + '/login')
  await page.getByLabel('Username').fill(HARNESS_OWNER_USERNAME)
  await page.getByLabel('Password').fill(HARNESS_OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
}

/** Retry the established fail-closed idle-connection session-probe race. */
async function gotoAuthenticated(page: Page, baseUrl: string, path: string): Promise<void> {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await page.goto(baseUrl + path)
    const heading = page.locator('h1').first()
    await heading.waitFor({ state: 'visible', timeout: 15_000 })
    if (!/^Sign in\b/i.test((await heading.innerText()).trim())) return
    await loginToDisposableServer(page, baseUrl)
  }
  await page.goto(baseUrl + path)
}

/** Read one `<dt>`/ `<dd>` pair from a DetailList by its exact label. */
async function detailValue(scope: Locator | Page, label: string): Promise<string> {
  const exact = label.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const term = scope.locator('dt').filter({ hasText: new RegExp('^' + exact + '$') })
  await expect(term).toHaveCount(1)
  const value = term.locator('xpath=following-sibling::dd[1]')
  await expect(value).toHaveCount(1)
  return (await value.innerText()).trim()
}

/** Tab until the target control holds focus, so focus styling is exercised. */
async function focusByKeyboard(page: Page, target: Locator): Promise<Locator> {
  for (let press = 0; press < 120; press += 1) {
    await page.keyboard.press('Tab')
    const focused = await target
      .evaluate((element) => element === document.activeElement)
      .catch(() => false)
    if (focused) return target
  }
  throw new Error('keyboard focus never reached the control ' + target)
}

async function readEvents(
  server: { adminGet(path: string): Promise<{ status: number; body: unknown }> },
): Promise<NotificationEventsResponse> {
  const response = await server.adminGet('/api/admin/v1/notifications/events?limit=50')
  expect(response.status).toBe(200)
  return response.body as NotificationEventsResponse
}

async function readDeliveries(
  server: { adminGet(path: string): Promise<{ status: number; body: unknown }> },
): Promise<NotificationDeliveriesResponse> {
  const response = await server.adminGet('/api/admin/v1/notifications/deliveries?limit=50')
  expect(response.status).toBe(200)
  return response.body as NotificationDeliveriesResponse
}

async function readRequest(
  server: { adminGet(path: string): Promise<{ status: number; body: unknown }> },
  requestId: string,
): Promise<NotificationRequestResult> {
  const response = await server.adminGet(
    '/api/admin/v1/notifications/requests/' + encodeURIComponent(requestId),
  )
  expect(response.status, 'the ledger must keep the recorded command result').toBe(200)
  return response.body as NotificationRequestResult
}

/** Look up a Server command result without asserting what comes back. */
async function lookupRequestInUi(page: Page, requestId: string): Promise<Locator> {
  await page.getByLabel('Request id').fill(requestId)
  await page.getByRole('button', { name: 'Look up request', exact: true }).click()
  return page.locator('[data-slot="notification-request-result"]')
}

test.describe('Disposable Server notification test acceptance', () => {
  // Booting a disposable Server, restarting it, and crossing ten viewport/theme
  // passes is process work the 30s default cannot cover.
  test.setTimeout(600_000)
  test('a controlled test is accepted once, deduplicated, cooled down, reconciled, and retried through the production WebUI', async ({
    page,
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the disposable Server flow runs once')
    const server = await startDisposableServer({
      notifications: { telegram: { testCooldownSeconds: 30 } },
    })
    try {
      await loginToDisposableServer(page, server.baseUrl)

      // The overview states the guarantee boundary and shows only masked data.
      await gotoAuthenticated(page, server.baseUrl, '/admin/notifications')
      await expect(
        page.getByRole('heading', { level: 1, name: 'Notifications', exact: true }),
      ).toBeVisible()
      await expect(
        page.getByText('Request dedup is not an end-to-end delivery guarantee', { exact: true }),
      ).toBeVisible()
      await expect(page.getByText(/It does not make Telegram delivery exactly-once/)).toBeVisible()
      expect(await detailValue(page, 'Destination (masked)')).toBe('****7890')
      expect(await detailValue(page, 'Provider reference')).toBe('telegram-token')

      await gotoAuthenticated(page, server.baseUrl, '/admin/notifications/channels')
      await expect(
        page.getByRole('heading', { level: 1, name: 'Notification Channels', exact: true }),
      ).toBeVisible()
      const channels = page.getByRole('table', { name: 'Notification channels', exact: true })
      const telegramRow = channels.getByRole('row').filter({ hasText: 'telegram' })
      await expect(telegramRow.getByText('Enabled', { exact: true })).toBeVisible()
      await expect(telegramRow.getByText('****7890', { exact: true })).toBeVisible()

      // The operator-side secret never reaches the browser at all.
      const rendered = await page.content()
      expect(rendered).not.toContain(HARNESS_TELEGRAM_TOKEN)
      expect(rendered).not.toContain(HARNESS_TELEGRAM_CHAT_ID)

      // --- The first command is accepted, recorded, and sent to the provider.
      const requestId = await detailValue(page, 'Request id')
      expect(requestId).toMatch(UUID)
      await page.getByRole('button', { name: 'Send test notification', exact: true }).click()
      const outcome = page.locator('[data-slot="notification-test-outcome"]')
      await expect(outcome).toBeVisible()
      expect(await detailValue(outcome, 'Deduplicated')).toBe('No')
      expect(await detailValue(outcome, 'Delivery state')).toBe('failed')
      await expect(
        page.getByText('The Server accepted the test command and recorded its Audit entry.', {
          exact: false,
        }),
      ).toBeVisible()
      const auditEntry = await detailValue(outcome, 'Audit entry')
      expect(Number(auditEntry)).toBeGreaterThan(0)

      const events = await readEvents(server)
      expect(events.items).toHaveLength(1)
      expect(events.items[0]).toMatchObject({ eventKind: 'test' })
      expect(events.items[0].deliveries).toHaveLength(1)
      expect(events.items[0].deliveries[0]).toMatchObject({
        channelKind: 'telegram',
        destination: '****7890',
        state: 'failed',
        attemptCount: 1,
      })
      const deliveryId = events.items[0].deliveries[0].deliveryId
      const deliveryEventId = events.items[0].eventId

      // The recorded command is queryable by its request id.
      const recorded = await readRequest(server, requestId)
      expect(recorded).toMatchObject({ requestId, commandKind: 'test' })
      expect(recorded.delivery.deliveryId).toBe(deliveryId)

      // --- Replaying the same request id returns the recorded command.
      await page.getByRole('button', { name: 'Send test notification', exact: true }).click()
      await expect(outcome).toBeVisible()
      expect(await detailValue(outcome, 'Deduplicated')).toBe('Yes')
      await expect(
        page.getByText('it returned the recorded command instead of sending another test.', {
          exact: false,
        }),
      ).toBeVisible()
      const replayedEvents = await readEvents(server)
      expect(replayedEvents.items).toHaveLength(1)
      expect(replayedEvents.items[0].deliveries[0].deliveryId).toBe(deliveryId)

      // --- A new request id inside the cooldown is refused, not queued.
      const cooldownRequestId = await (async () => {
        await page.getByRole('button', { name: 'New request id', exact: true }).click()
        return detailValue(page, 'Request id')
      })()
      expect(cooldownRequestId).toMatch(UUID)
      expect(cooldownRequestId).not.toBe(requestId)
      await page.getByRole('button', { name: 'Send test notification', exact: true }).click()
      await expect(
        page.getByRole('alert').filter({ hasText: 'sent recently; retry after' }),
      ).toBeVisible()
      await expect(
        page.getByText('The cooldown bounds how often the channel can be exercised;', { exact: false }),
      ).toBeVisible()
      expect((await readEvents(server)).items).toHaveLength(1)
      const refused = await server.adminGet(
        '/api/admin/v1/notifications/requests/' + encodeURIComponent(cooldownRequestId),
      )
      expect(refused.status, 'a refused command is not recorded').toBe(404)

      // --- The Owner reconciles the original request from the lookup panel.
      const result = await lookupRequestInUi(page, requestId)
      await expect(result).toBeVisible()
      expect(await detailValue(result, 'Request id')).toBe(requestId)
      expect(await detailValue(result, 'Command kind')).toBe('test')
      expect(await detailValue(result, 'Delivery state')).toBe('failed')
      await expect(
        page.getByText('This is the Server command result, not a claim that Telegram accepted', {
          exact: false,
        }),
      ).toBeVisible()

      // --- An unknown request id is reported as unknown, never as a resend.
      await lookupRequestInUi(page, '00000000-0000-4000-8000-000000000206')
      await expect(result).toHaveCount(0)
      await expect(page.getByText('no unexpired record for this request id', { exact: false })).toBeVisible()
      expect((await readEvents(server)).items).toHaveLength(1)

      // --- The retry command carries its own request id and no new Event.
      await gotoAuthenticated(page, server.baseUrl, '/admin/notifications/deliveries/' + deliveryId)
      await expect(
        page.getByRole('heading', { level: 1, name: /Notification Delivery/, exact: false }),
      ).toBeVisible()
      const retry = page.getByRole('button', { name: 'Queue retry', exact: true })
      await expect(retry).toBeEnabled()
      const retryRequestId = await detailValue(page, 'Request id')
      expect(retryRequestId).toMatch(UUID)
      await retry.press('Enter')
      await expect(
        page.getByText('The Server accepted the retry command and queued the delivery.', { exact: false }),
      ).toBeVisible()
      const retryRecord = await readRequest(server, retryRequestId)
      expect(retryRecord.commandKind).toBe('retry')
      expect(retryRecord.delivery.deliveryId).toBe(deliveryId)
      expect(POST_RETRY_STATES).toContain(retryRecord.delivery.state)
      expect(retryRecord.eventId, 'a retry stays attached to the recorded Event').toBe(deliveryEventId)
      expect((await readEvents(server)).items, 'a retry creates no new notification event').toHaveLength(1)
      const detail = (await server.adminGet(
        '/api/admin/v1/notifications/deliveries/' + encodeURIComponent(deliveryId),
      )).body as NotificationDeliveryDetail
      expect(POST_RETRY_STATES).toContain(detail.state)
      expect((await readDeliveries(server)).items).toHaveLength(1)

      // Replaying the same retry request id must return the recorded result
      // instead of queueing a second attempt. The Owner's button state depends on
      // the delivery's live state (the worker may already be retrying it), so the
      // replay is asserted on the wire where the identity is explicit.
      const replayedRetry = (
        await server.adminPost(
          '/api/admin/v1/notifications/deliveries/' + encodeURIComponent(deliveryId) + '/retry',
          { requestId: retryRequestId },
        )
      ).body as { deliveryId: string; requestId: string; deduplicated: boolean }
      expect(replayedRetry.deduplicated, 'a replayed retry must not queue a second attempt').toBe(true)
      expect(replayedRetry.requestId).toBe(retryRequestId)
      expect(replayedRetry.deliveryId).toBe(deliveryId)
      expect((await readRequest(server, retryRequestId)).delivery.deliveryId).toBe(deliveryId)

      // --- The ledger survives a process restart on the same SQLite file.
      await server.restart()
      await loginToDisposableServer(page, server.baseUrl)
      await gotoAuthenticated(page, server.baseUrl, '/admin/notifications/channels')
      const afterRestart = await lookupRequestInUi(page, requestId)
      await expect(afterRestart).toBeVisible()
      expect(await detailValue(afterRestart, 'Request id')).toBe(requestId)
      expect(await detailValue(afterRestart, 'Command kind')).toBe('test')
      // The ledger replays the delivery as it stands now: a state the worker
      // already advanced past 'failed' is still a recorded Server result, not a
      // re-run of the test command.
      expect(POST_RETRY_STATES).toContain(await detailValue(afterRestart, 'Delivery state'))
      expect((await readEvents(server)).items, 'a restart must not re-run the recorded command').toHaveLength(1)

      // A never-authenticated Guest hits the real route-level Owner gate.
      const anonymous = await browser.newContext()
      try {
        const anonymousPage = await anonymous.newPage()
        await anonymousPage.goto(server.baseUrl + '/admin/notifications/channels')
        await expect(
          anonymousPage.getByRole('heading', { level: 1, name: 'Owner access required' }),
        ).toBeVisible()
        await expect(anonymousPage.getByText('****7890', { exact: true })).toHaveCount(0)
        await expect(anonymousPage.getByRole('table', { name: 'Notification channels', exact: true })).toHaveCount(0)
      } finally {
        await anonymous.close()
      }
    } finally {
      await server.dispose()
    }
  })

  test('the notification surface holds across the fixed viewport and theme matrix with keyboard and touch', async ({
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop-1280', 'the matrix runs once against one disposable Server')
    const server = await startDisposableServer({
      notifications: { telegram: { testCooldownSeconds: 30 } },
    })
    try {
      const context = await browser.newContext({ hasTouch: true, colorScheme: 'light' })
      const page = await context.newPage()
      try {
        await loginToDisposableServer(page, server.baseUrl)
        // One recorded test gives every list page a real row to lay out.
        await gotoAuthenticated(page, server.baseUrl, '/admin/notifications/channels')
        await page.getByRole('button', { name: 'Send test notification', exact: true }).click()
        await expect(page.locator('[data-slot="notification-test-outcome"]')).toBeVisible()

        for (const viewport of VIEWPORTS) {
          for (const colorScheme of ['light', 'dark'] as const) {
            await page.setViewportSize(viewport)
            await page.emulateMedia({ colorScheme })
            for (const surface of [
              {
                path: '/admin/notifications',
                heading: 'Notifications',
                channel: 'Overview',
              },
              {
                path: '/admin/notifications/events',
                heading: 'Notification Events',
                channel: 'Events',
                table: 'Notification events',
                filter: { label: 'Event kind', param: 'kind', value: 'test' },
              },
              {
                path: '/admin/notifications/deliveries',
                heading: 'Notification Deliveries',
                channel: 'Deliveries',
                table: 'Notification deliveries',
                filter: { label: 'State', param: 'state', value: 'failed' },
              },
              {
                path: '/admin/notifications/channels',
                heading: 'Notification Channels',
                channel: 'Channels',
                table: 'Notification channels',
              },
            ]) {
              await gotoAuthenticated(page, server.baseUrl, surface.path)
              await expect(
                page.getByRole('heading', { level: 1, name: surface.heading, exact: true }),
              ).toBeVisible()
              const sections = page.getByRole('navigation', { name: 'Notification sections' })
              await expect(sections.getByRole('link')).toHaveCount(4)
              await expect(
                sections.getByRole('link', { name: surface.channel, exact: true }),
              ).toHaveAttribute('aria-current', 'page')
              if (surface.table) {
                const table = page.getByRole('table', { name: surface.table, exact: true })
                await expect(table.getByRole('row')).toHaveCount(2)
              }
              if (surface.filter) {
                // The label wraps its select, so the accessible name also carries
                // the option text: substring matching is the stable contract.
                await page.getByLabel(surface.filter.label).selectOption(surface.filter.value)
                await expect(page).toHaveURL(
                  new RegExp(surface.filter.param + '=' + surface.filter.value),
                )
                await expect(page.getByRole('table').getByRole('row')).toHaveCount(2)
              }
              await expectNoHorizontalOverflow(page)
              if (viewport.width <= 768) await expectVisibleInteractiveTargets(page)
            }

            if (viewport.width === 360) {
              const test = page.getByRole('button', { name: 'Send test notification', exact: true })
              await test.tap()
              // A fresh mount carries a fresh request id, so the tap hits the
              // cooldown rather than sending a second test: touch must reach
              // the command and surface the Server refusal, not skip it.
              await expect(
                page.getByRole('alert').filter({ hasText: 'sent recently; retry after' }),
              ).toBeVisible()
              const sections = page.getByRole('navigation', { name: 'Notification sections' })
              await sections.getByRole('link', { name: 'Deliveries', exact: true }).tap()
              await expect(
                page.getByRole('heading', { level: 1, name: 'Notification Deliveries', exact: true }),
              ).toBeVisible()
              await expectNoHorizontalOverflow(page)
              await expectVisibleInteractiveTargets(page)
            }
          }
        }

        // Keyboard reachability is a theme question, not a viewport one:
        // walking the same tab order ten times only repeats the same stops.
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.setViewportSize(VIEWPORTS[3])
          await page.emulateMedia({ colorScheme })
          await gotoAuthenticated(page, server.baseUrl, '/admin/notifications/channels')
          // Type the request id with the keyboard: the submit button stays
          // disabled until a field holds one, so keyboard reachability has to
          // walk the real order instead of jumping to a pre-enabled control.
          const field = page.getByLabel('Request id')
          await focusByKeyboard(page, field)
          await field.pressSequentially('00000000-0000-4000-8000-000000000206')
          const lookup = await focusByKeyboard(page, page.getByRole('button', { name: 'Look up request' }))
          await expectFocusedElementHasVisibleFocus(page)
          await lookup.press('Enter')
          await expect(
            page.getByText('no unexpired record for this request id', { exact: false }),
          ).toBeVisible()
          await expectNoHorizontalOverflow(page)
        }

        // Rendering and inspecting never sends a second test: the recorded
        // command is still the only event the Server holds.
        expect((await readEvents(server)).items).toHaveLength(1)
      } finally {
        await context.close()
      }
    } finally {
      await server.dispose()
    }
  })
})
