import { expect, test, type Page } from '@playwright/test'
import {
  HARNESS_OWNER_PASSWORD,
  HARNESS_OWNER_USERNAME,
  startDisposableServer,
} from './server-harness'

/**
 * Management acceptance on a disposable Server (issue #166, part of #165).
 *
 * Every other e2e spec shares one long-lived Server seeded by
 * `e2e/start-server.sh`. This suite instead boots its own throwaway Server
 * (own port, own state directory, own SQLite) so it can onboard an Agent
 * through the real Admin/Agent HTTP API, submit a real report with the minted
 * credential, restart the process in place, and prove the persisted state
 * without leaving a trace for the shared fixtures.
 *
 * Because the Server is disposable, this flow has no viewport-specific UI
 * contract; running the full enroll/report/restart sequence per fixed viewport
 * would only multiply process boots. It runs once on desktop-1280, like the
 * suite's other process-heavy acceptance flows.
 */
test.describe('Disposable Server management acceptance', () => {
  test('onboards a real Agent, submits a report, and survives a restart', async ({
    page,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== 'desktop-1280',
      'the disposable Server flow runs once',
    )
    const server = await startDisposableServer()
    try {
      // Real enrollment: Admin Enrollment Token -> Agent Credential.
      const { agentId, credential } = await server.enrollAgent()
      expect(credential.startsWith('pp_agent_')).toBe(true)

      // Real report submission with that credential.
      const receipt = await server.submitReport(agentId, credential)
      expect(receipt.receipt.disposition).toBe('accepted')
      expect(receipt.receipt.nodes[0].current).toBe('accepted')

      // The Admin REST projection sees the enrolled identity.
      const detail = (await server.expectAdminGet(
        `/api/admin/v1/agents/${agentId}`,
        200,
      )) as { agent_id: string }
      expect(detail.agent_id).toBe(agentId)

      // The production WebUI served by the disposable Server renders it.
      await loginToDisposableServer(page, server.baseUrl)
      await page.goto(`${server.baseUrl}/admin/agents`)
      await expect(page.locator(`a[href="/admin/agents/${agentId}"]`)).toBeVisible()

      // Controllable in-test restart: the same Server resumes the same state.
      await server.restart()
      await loginToDisposableServer(page, server.baseUrl)
      await page.goto(`${server.baseUrl}/admin/agents`)
      await expect(page.locator(`a[href="/admin/agents/${agentId}"]`)).toBeVisible()

      // The immutable receipt survived the restart: an exact replay is served
      // from storage instead of re-applying the report.
      const replay = await server.submitReport(agentId, credential)
      expect(replay.receipt).toEqual(receipt.receipt)
    } finally {
      // No residual state: the process stops and the state directory is gone.
      await server.dispose()
    }
  })
})

async function loginToDisposableServer(page: Page, baseUrl: string) {
  await page.context().clearCookies()
  await page.goto(`${baseUrl}/login`)
  await page.getByLabel('Username').fill(HARNESS_OWNER_USERNAME)
  await page.getByLabel('Password').fill(HARNESS_OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('region', { name: 'Home' })).toBeVisible()
}
