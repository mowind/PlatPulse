import { expect, test } from '@playwright/test'
import { expectNoHorizontalOverflow, loginAs } from './helpers'

const PUBLIC_NODE_ID = '0195f2a1-0014-4014-8014-000000000014'

async function installControlledRealtime(page: Parameters<typeof loginAs>[0]) {
  await page.addInitScript(() => {
    type Handler = (event: { data?: string }) => void
    class ControlledEventSource {
      static instances: ControlledEventSource[] = []
      static closed = 0
      handlers = new Map<string, Handler[]>()
      onopen: (() => void) | null = null
      onerror: (() => void) | null = null
      constructor() {
        ControlledEventSource.instances.push(this)
        queueMicrotask(() => this.onopen?.())
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
  })
}

test.describe('SCN-HOME-RESPONSIVE-ACCESSIBILITY / live refresh transport state', () => {
  test('announces SSE pause and browser offline separately on Home and Admin', async ({ page }) => {
    await page.route('**/api/public/v1/events', (route) => route.abort())
    await page.route('**/api/admin/v1/events', (route) => route.abort())
    await loginAs(page)
    await expect(page.getByText('Live updates paused', { exact: true })).toBeVisible()
    await page.context().setOffline(true)
    await expect(page.getByText('You are offline', { exact: true })).toBeVisible()
    await expectNoHorizontalOverflow(page)

    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await expect(page.getByText('Live updates paused', { exact: true })).toBeVisible()
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
    await page.evaluate(({ nodeId }) => {
      const browserWindow = window as typeof window & { __emitRealtime: (type: string, data?: string) => void }
      browserWindow.__emitRealtime('invalidation', JSON.stringify({ resource: 'node', resourceId: nodeId, revision: 2 }))
    }, { nodeId })
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
    const closedBefore = await page.evaluate(() => (window as typeof window & { __realtimeClosed: number }).__realtimeClosed)
    await page.evaluate(() => {
      const browserWindow = window as typeof window & { __emitRealtime: (type: string, data?: string) => void }
      browserWindow.__emitRealtime('reset', JSON.stringify({ reset: true }))
    })
    await expect(page.getByText('Revalidating Home access…')).toBeVisible()
    await expect.poll(async () => page.evaluate(() => (window as typeof window & { __realtimeClosed: number }).__realtimeClosed)).toBeGreaterThan(closedBefore)
    await expect(page.getByRole('heading', { level: 1, name: 'Node A' })).toBeVisible()
  })

  test('preserves an Admin route through exact-key invalidation and reset', async ({ page }) => {
    await installControlledRealtime(page)
    await loginAs(page)
    await page.getByRole('link', { name: 'Admin', exact: true }).click()
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible()
    await page.goto('/admin/nodes?visibility=all&health=all')
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()

    await page.evaluate(({ nodeId }) => {
      const browserWindow = window as typeof window & { __emitRealtime: (type: string, data?: string) => void }
      browserWindow.__emitRealtime('invalidation', JSON.stringify({ resource: 'node', resourceId: nodeId, revision: 2 }))
    }, { nodeId: PUBLIC_NODE_ID })
    await expect(page).toHaveURL(/\/admin\/nodes\?visibility=all&health=all/)
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()

    const closedBeforeReset = await page.evaluate(() => (window as typeof window & { __realtimeClosed: number }).__realtimeClosed)
    await page.evaluate(() => {
      const browserWindow = window as typeof window & { __emitRealtime: (type: string, data?: string) => void }
      browserWindow.__emitRealtime('reset', JSON.stringify({ reset: true }))
    })
    await expect.poll(async () => page.evaluate(() => (window as typeof window & { __realtimeClosed: number }).__realtimeClosed)).toBeGreaterThan(closedBeforeReset)
    await expect.poll(async () => page.url()).toMatch(/\/admin\/nodes\?visibility=all&health=all/)
    await expect(page.getByRole('heading', { level: 1, name: 'Nodes' })).toBeVisible()
  })
})
