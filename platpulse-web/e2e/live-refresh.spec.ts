import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'

async function installControlledRealtime(page: Parameters<typeof loginAs>[0]) {
  await page.addInitScript(() => {
    type Handler = (event: { data?: string }) => void
    class ControlledEventSource {
      static instances: ControlledEventSource[] = []
      static closed = 0
      static opened = 0
      handlers = new Map<string, Handler[]>()
      onopen: (() => void) | null = null
      onerror: (() => void) | null = null
      constructor() {
        ControlledEventSource.instances.push(this)
        queueMicrotask(() => {
          this.onopen?.()
          ControlledEventSource.opened += 1
        })
      }
      addEventListener(type: string, handler: Handler) {
        this.handlers.set(type, [...(this.handlers.get(type) ?? []), handler])
      }
      removeEventListener(type: string, handler: Handler) {
        this.handlers.set(type, (this.handlers.get(type) ?? []).filter((candidate) => candidate !== handler))
      }
      close() { ControlledEventSource.closed += 1 }
      emit(type: string, data?: string) {
        for (const handler of this.handlers.get(type) ?? []) handler({ data })
      }
    }
    const browserWindow = window as typeof window & {
      __emitRealtime: (type: string, data?: string) => void
    }
    Object.defineProperty(window, 'EventSource', { configurable: true, value: ControlledEventSource })
    browserWindow.__emitRealtime = (type, data) => {
      for (const source of ControlledEventSource.instances) source.emit(type, data)
    }
    Object.defineProperty(browserWindow, '__realtimeClosed', { get: () => ControlledEventSource.closed })
    Object.defineProperty(browserWindow, '__realtimeOpened', { get: () => ControlledEventSource.opened })
  })
}

/** Emit a controlled realtime event into every open layout stream. */
async function emitRealtime(page: Parameters<typeof loginAs>[0], type: 'invalidation' | 'reset', payload: unknown) {
  await page.evaluate(({ type, payload }) => {
    const browserWindow = window as typeof window & { __emitRealtime: (eventType: string, data?: string) => void }
    browserWindow.__emitRealtime(type, JSON.stringify(payload))
  }, { type, payload })
}

/** Count controlled EventSource instances created by the shell. */
async function realtimeOpened(page: Parameters<typeof loginAs>[0]): Promise<number> {
  return page.evaluate(() => (window as typeof window & { __realtimeOpened: number }).__realtimeOpened)
}

/** Count controlled EventSource instances closed by the shell. */
async function realtimeClosed(page: Parameters<typeof loginAs>[0]): Promise<number> {
  return page.evaluate(() => (window as typeof window & { __realtimeClosed: number }).__realtimeClosed)
}

test.describe('SCN-HOME-RESPONSIVE-ACCESSIBILITY / live refresh transport state', () => {
  test('announces SSE pause and browser offline separately on Home and Admin', async ({ page }) => {
    await page.route('**/api/public/v1/events**', (route) => route.abort())
    await page.route('**/api/admin/v1/events**', (route) => route.abort())
    await loginAs(page)
    await expect(page.getByText('Live updates paused', { exact: true })).toBeVisible()
    await page.context().setOffline(true)
    await expect(page.getByText('You are offline', { exact: true })).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.context().setOffline(false)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expect(page.getByText('Live updates paused', { exact: true })).toBeVisible()
    await page.context().setOffline(true)
    await expect(page.getByText('You are offline', { exact: true })).toBeVisible()
    await expectNoHorizontalOverflow(page)
  })

  test('SCN-NODE-LAST-GOOD-REFRESH preserves the routed tab and last-good Node', async ({ page }) => {
    await installControlledRealtime(page)
    await loginAs(page)
    const nodeId = PUBLIC_NODE_ID

    let nodeCalls = 0
    await page.route('**/api/public/v1/nodes/*', async (route) => {
      const path = new URL(route.request().url()).pathname
      if (!path.endsWith(`/nodes/${nodeId}`)) return route.continue()
      nodeCalls += 1
      if (nodeCalls === 1) return route.continue()
      await route.fulfill({
        status: 503,
        contentType: 'application/json',
        body: JSON.stringify({ error: { code: 'unavailable', message: 'refresh failed' } }),
      })
    })
    await page.goto(`/nodes/${nodeId}`)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
    await page.getByRole('tab', { name: 'Network' }).click()
    await emitRealtime(page, 'invalidation', { resource: 'node', resourceId: nodeId, revision: 2 })
    await expect(page.getByText(/last successful Node data/i)).toBeVisible()
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
    await expect(page.getByRole('tab', { name: 'Network' })).toHaveAttribute('aria-selected', 'true')
    await expectNoHorizontalOverflow(page)
  })

  test('SCN-AUTH-SESSION-REVOKED clears and reloads the Public route', async ({ page }) => {
    await installControlledRealtime(page)
    await loginAs(page)
    await page.goto(`/nodes/${PUBLIC_NODE_ID}`)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
    const closedBefore = await realtimeClosed(page)
    // Hold one authoritative revalidation request briefly so the transient
    // privacy-clearing state is deterministic even on a fast CI runner.
    await page.route('**/api/public/v1/access', async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 500))
      await route.continue()
    })
    await emitRealtime(page, 'reset', { reset: true })
    await expect(page.getByText('Revalidating Home access…')).toBeVisible()
    await expect.poll(async () => realtimeClosed(page)).toBeGreaterThan(closedBefore)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
  })

  test('preserves an Admin route through exact-key invalidation and reset', async ({ page }) => {
    await installControlledRealtime(page)
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await page.goto('/admin/nodes?visibility=all&health=all')
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()
    await expect.poll(async () => realtimeOpened(page)).toBeGreaterThan(0)

    await emitRealtime(page, 'invalidation', { resource: 'node', resourceId: PUBLIC_NODE_ID, revision: 2 })
    await expect(page).toHaveURL(/\/admin\/nodes\?visibility=all&health=all/)
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()

    const closedBeforeReset = await realtimeClosed(page)
    await emitRealtime(page, 'reset', { reset: true })
    await expect.poll(async () => realtimeClosed(page)).toBeGreaterThan(closedBeforeReset)
    await expect.poll(async () => page.url()).toMatch(/\/admin\/nodes\?visibility=all&health=all/)
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()
  })

  test('SCN-HOME-VALIDATOR-ACTIVITY-REFRESH refetches Home through one layout-owned stream', async ({ page }) => {
    await installControlledRealtime(page)
    await loginAs(page)
    await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
    // Wait until the seeded projection is rendered before installing the
    // REST spy so only invalidation-driven refetches are counted.
    await expect(page.getByRole('link', { name: /Node H/ })).toBeVisible({ timeout: 15_000 })
    await expect.poll(async () => realtimeOpened(page)).toBeGreaterThan(0)

    let networksCalls = 0
    await page.route('**/api/public/v1/networks', async (route) => {
      networksCalls += 1
      const response = await route.fetch()
      const networks = (await response.json()) as Array<{
        networkKey: string
        nodes: Array<{ nodeId: string; validator?: { activity?: string; activityState?: string } }>
      }>
      // Simulate an authoritative Provider refresh: Node H's canonical
      // Activity changes from Producing to Active, so the refetched
      // projection must render the updated badge on Home.
      for (const network of networks) {
        for (const node of network.nodes) {
          if (node.nodeId === '0195f2a1-0060-4060-8060-000000000060' && node.validator) {
            node.validator.activity = 'active'
            node.validator.activityState = 'current'
          }
        }
      }
      await route.fulfill({ response, json: networks })
    })

    // Mirror the Server's Provider-refresh invalidation sequence: a changed
    // Validator publishes both a per-Validator event and the Network event
    // that refreshes Home's Public networks query (design §3.3).
    await emitRealtime(page, 'invalidation', { resource: 'validator', resourceId: '0195f2a1-0070-4070-8070-000000000070', eventId: 2 })
    await emitRealtime(page, 'invalidation', { resource: 'network', resourceId: 'home-convergence', eventId: 3 })
    await expect.poll(() => networksCalls).toBe(1)

    // The refetched projection renders the updated Provider Activity on the
    // same layout-owned stream: no per-card or second Home SSE connection
    // was opened, and the badge switched from Producing to Active (the Node
    // display name still contains "Producing", so assert the badge content
    // and its header order through the card's accessible name).
    await expect(
      page.getByRole('link', { name: /Node H — Producing Card[\s\S]*Active[\s\S]*Healthy/ }),
    ).toHaveCount(1)
    await expect(page.getByText('Active', { exact: true })).toHaveCount(1)
    expect(await realtimeOpened(page)).toBe(1)
    await expectNoHorizontalOverflow(page)
  })
})
