import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { publicQueryClient } from '../api/public'

const SESSION = {
  session: {
    userId: 'owner-1',
    username: 'owner',
    role: 'owner',
    createdAt: '2026-08-20T00:00:00Z',
    lastSeenAt: '2026-08-20T00:00:00Z',
    expiresAt: '2026-08-27T00:00:00Z',
  },
  csrfToken: 'csrf-test',
}

const HISTORY_WINDOW = {
  windowDays: 7,
  defaultDays: 14,
  minDays: 1,
  maxDays: 30,
  updatedAt: '2026-08-20T00:00:00Z',
  updatedBy: 'owner-1',
}

const GEO_STATUS = {
  provider: 'disabled',
  provider_label: 'Disabled',
  provider_generation: 1,
  providers: [
    {
      provider: 'disabled',
      label: 'Disabled',
      available: true,
      unavailable_reason: null,
      sends_peer_addresses: false,
      disclosure: null,
    },
    {
      provider: 'local_mmdb',
      label: 'Local MMDB',
      available: true,
      unavailable_reason: null,
      sends_peer_addresses: false,
      disclosure: null,
    },
    {
      provider: 'ipinfo',
      label: 'IPinfo',
      available: true,
      unavailable_reason: null,
      sends_peer_addresses: true,
      disclosure:
        'IPinfo asks the fixed HTTPS endpoint https://ipinfo.io/{ip}/json for each observed Peer public IP and keeps only the returned two-letter country code. It carries no token and sends no other Peer data.',
    },
    {
      provider: 'geojs',
      label: 'GeoJS',
      available: true,
      unavailable_reason: null,
      sends_peer_addresses: true,
      disclosure:
        'GeoJS asks the fixed HTTPS endpoint https://get.geojs.io/v1/ip/geo/{ip}.json for each observed Peer public IP and keeps only the returned two-letter country code. It carries no token and sends no other Peer data.',
    },
  ],
  state: 'disabled',
  configured: true,
  build_epoch: 1700000000,
  digest: 'digest',
  loaded_at: '2026-08-20T00:00:00Z',
  last_error: null,
  cache_country_count: 0,
  cache_entry_count: 0,
  pending_lookup_count: null,
  last_success_at: null,
  rate_limited_until: null,
  refresh: null,
  refresh_unavailable_reason: 'Geo is Disabled, so no Peer address is resolved',
}

const TEST_ORIGIN = 'http://platpulse.test'

type RouteHandler = (request: Request) => Response | Promise<Response>

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function apiError(message: string, fields: string[] = [], status = 422): Response {
  return response({ error: { code: 'invalid_request', message, requestId: 'request-1', fields } }, status)
}

function mockFetch(routes: Record<string, RouteHandler>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init)
    const path = request.url.replace(TEST_ORIGIN, '').split('?')[0]
    const handler = routes[path]
    return handler ? handler(request) : response({ error: { code: 'not_found' } }, 404)
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

function successfulRoutes(overrides: Record<string, RouteHandler> = {}) {
  return {
    '/api/public/v1/session': () => response(SESSION),
    '/api/admin/v1/history-window': () => response(HISTORY_WINDOW),
    '/api/admin/v1/history-window/impact': async (request: Request) => {
      const body = await request.json() as { windowDays: number }
      return response({
        windowDays: body.windowDays,
        estimatedRows: body.windowDays < HISTORY_WINDOW.windowDays ? 12 : 0,
        minDays: HISTORY_WINDOW.minDays,
        maxDays: HISTORY_WINDOW.maxDays,
        notes: [],
      })
    },
    '/api/admin/v1/access-mode': () => response({ mode: 'private', authorizationGeneration: 0 }),
    '/api/admin/v1/geo': () => response(GEO_STATUS),
    ...overrides,
  }
}

async function renderSettings() {
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', '/admin/settings')
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
  await screen.findByRole('heading', { level: 1, name: 'Settings' })
}

beforeEach(() => {
  window.history.replaceState({}, '', '/')
  client.setConfig({ baseUrl: TEST_ORIGIN })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  adminQueryClient.clear()
  publicQueryClient.clear()
})

describe('Admin Settings workflows', () => {
  it('renders one Settings page with ordered independent configuration cards', async () => {
    mockFetch(successfulRoutes())

    await renderSettings()

    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1)
    const cards = screen.getAllByRole('article')
    expect(cards).toHaveLength(3)
    expect(within(cards[0]).getByRole('heading', { level: 2, name: 'History Window' })).toBeTruthy()
    expect(within(cards[1]).getByRole('heading', { level: 2, name: 'Site Access Mode' })).toBeTruthy()
    expect(within(cards[2]).getByRole('heading', { level: 2, name: 'Geo provider' })).toBeTruthy()

    const historyCard = within(cards[0])
    expect(historyCard.getByText('7 days')).toBeTruthy()
    expect(historyCard.getByText('14 days')).toBeTruthy()
    expect(historyCard.getByText('1 day')).toBeTruthy()
    expect(historyCard.getByText('30 days')).toBeTruthy()
    expect(historyCard.getByText(/2026-08-20T00:00:00Z by owner-1/)).toBeTruthy()
    expect(historyCard.getByText(/Shortening removes expired history asynchronously/)).toBeTruthy()
    expect(historyCard.getByText(/Lengthening cannot recover deleted or missed history/)).toBeTruthy()

    const accessCard = within(cards[1])
    expect(accessCard.getByText('Private')).toBeTruthy()
    expect(accessCard.getByText(/Public permits anonymous Home reads/)).toBeTruthy()
    expect(accessCard.getByText(/Private requires Owner login/)).toBeTruthy()

    // Only implemented providers are offered, and the privacy consequence is
    // explicit (issue #132).
    const geoCard = within(cards[2])
    expect(geoCard.getByLabelText(/Disabled/)).toBeTruthy()
    expect(geoCard.getByLabelText(/Local MMDB/)).toBeTruthy()
    // IPinfo is offered because the Server really implements it, and it
    // states the outbound consequence before any selection (issue #134).
    expect(geoCard.getByLabelText(/IPinfo/)).toBeTruthy()
    expect(geoCard.getAllByText(/Sends observed Peer public IPs to a third party/).length).toBe(2)
    expect(geoCard.getByText(/ipinfo\.io\/\{ip\}\/json/)).toBeTruthy()
    // GeoJS is the second external provider this Server really implements
    // (issue #135), and it states its own fixed destination before selection.
    expect(geoCard.getByLabelText(/GeoJS/)).toBeTruthy()
    expect(geoCard.getByText(/get\.geojs\.io\/v1\/ip\/geo\/\{ip\}\.json/)).toBeTruthy()
    // Only the four implemented options exist, and the global refresh
    // (issue #136) is present but explicitly unavailable while Geo is
    // Disabled: the Server's reason is rendered instead of a fake success.
    expect(geoCard.getAllByRole('radio')).toHaveLength(4)
    const refreshButton = geoCard.getByRole('button', { name: 'Refresh Peer geolocation' })
    expect((refreshButton as HTMLButtonElement).disabled).toBe(true)
    expect(geoCard.getByText(/Geo is Disabled, so no Peer address is resolved/)).toBeTruthy()
    expect(geoCard.getByText(/One lookup per distinct IP address/)).toBeTruthy()
    expect(geoCard.getByText(/Peer addresses never leave the Server/)).toBeTruthy()
    expect(geoCard.getByText('Not scheduled')).toBeTruthy()
  })

  it('forces a real Geo refresh and reports per-address progress with terminal counts', async () => {
    let postCount = 0
    const enabled = {
      ...GEO_STATUS,
      provider: 'local_mmdb',
      provider_label: 'Local MMDB',
      provider_generation: 2,
      state: 'current',
      pending_lookup_count: 0,
      refresh_unavailable_reason: null,
    }
    let statusBody: Record<string, unknown> = enabled
    const run = {
      run_id: 'run-1',
      provider: 'local_mmdb',
      provider_label: 'Local MMDB',
      provider_generation: 2,
      state: 'running',
      abort_code: null,
      abort_reason: null,
      total_lookups: 4,
      peer_records_in_scope: 6,
      completed_lookups: 1,
      resolved_lookups: 1,
      no_country_lookups: 0,
      failed_lookups: 0,
      rate_limited_lookups: 0,
      started_at: '2026-08-20T02:00:00Z',
      finished_at: null,
    }
    const running = { ...enabled, refresh: run }
    const completed = {
      ...enabled,
      refresh: {
        ...run,
        state: 'completed',
        completed_lookups: 4,
        resolved_lookups: 2,
        no_country_lookups: 1,
        failed_lookups: 1,
        finished_at: '2026-08-20T02:00:05Z',
      },
    }
    mockFetch(successfulRoutes({
      '/api/admin/v1/geo': () => response(statusBody),
      '/api/admin/v1/geo/refresh': () => {
        postCount += 1
        statusBody = running
        return response({ refresh: run, started: true, audit_event_id: 91 })
      },
    }))

    await renderSettings()
    const geoCard = within(screen.getAllByRole('article')[2])
    expect(geoCard.queryByText(/Refresh complete/)).toBeNull()

    fireEvent.click(geoCard.getByRole('button', { name: 'Refresh Peer geolocation' }))

    await waitFor(() => expect(postCount).toBe(1))
    // Progress comes from the Server diagnostic rather than from the POST
    // having been accepted, and the counts are clearly IP lookups.
    expect(await geoCard.findByText(/Refreshing: 1 of 4 IP lookups done/)).toBeTruthy()
    expect(geoCard.getByText(/1 resolved, 0 without a country, 0 failed/)).toBeTruthy()
    expect((geoCard.getByRole('button', { name: /Refreshing/ }) as HTMLButtonElement).disabled).toBe(true)
    // One address can serve several Peer records, so the two denominators are
    // stated separately: 4 IP lookups, 6 Peer records.
    expect(
      geoCard.getByText('Peer records referencing them').closest('div')?.textContent,
    ).toContain('6')

    statusBody = completed
    await waitFor(
      () => expect(geoCard.getByText(/Refresh complete: 2 resolved/)).toBeTruthy(),
      { timeout: 4_000 },
    )
    expect(geoCard.getByText(/1 without a country, 1 failed/)).toBeTruthy()
    expect(geoCard.getByText('4 of 4 complete')).toBeTruthy()
    expect(geoCard.getByRole('button', { name: 'Refresh Peer geolocation' })).toBeTruthy()
  }, 20_000)


  it('changes the Geo provider through the audited Admin API', async () => {
    let putBody: unknown = null
    let provider = 'disabled'
    const enabledStatus = {
      ...GEO_STATUS,
      provider: 'local_mmdb',
      provider_label: 'Local MMDB',
      provider_generation: 2,
      state: 'current',
      pending_lookup_count: 3,
      cache_country_count: 2,
      last_success_at: '2026-08-20T01:00:00Z',
    }
    mockFetch(successfulRoutes({
      '/api/admin/v1/geo': () => response(provider === 'disabled' ? GEO_STATUS : enabledStatus),
      '/api/admin/v1/geo/provider': async (request) => {
        putBody = await request.json()
        provider = 'local_mmdb'
        return response({ geo: enabledStatus, audit_event_id: 77 })
      },
    }))

    await renderSettings()
    const geoCard = within(screen.getAllByRole('article')[2])
    const save = geoCard.getByRole('button', { name: 'Save Geo provider' })
    expect((save as HTMLButtonElement).disabled).toBe(true)

    fireEvent.click(geoCard.getByLabelText(/Local MMDB/))
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(save)

    await waitFor(() => expect(putBody).toEqual({ provider: 'local_mmdb' }))
    expect(
      await geoCard.findByText(/Geo provider is now Local MMDB \(Audit #77\)/),
    ).toBeTruthy()
    // The Server-side switch is reflected by the refreshed status badge and
    // detail values, not only by the optimistic notice.
    await waitFor(() => {
      expect(geoCard.getAllByText('Current').length).toBeGreaterThanOrEqual(2)
      expect(geoCard.getByText('3')).toBeTruthy()
      expect(geoCard.getByText('2')).toBeTruthy()
    })
  }, 20_000)

  it('keeps Local MMDB unavailable when the Server has no local database', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/geo': () => response({
        ...GEO_STATUS,
        configured: false,
        providers: [
          {
            provider: 'disabled',
            label: 'Disabled',
            available: true,
            unavailable_reason: null,
            sends_peer_addresses: false,
            disclosure: null,
          },
          {
            provider: 'local_mmdb',
            label: 'Local MMDB',
            available: false,
            unavailable_reason: 'No local GeoLite2 Country database is configured on this Server',
            sends_peer_addresses: false,
            disclosure: null,
          },
          {
            provider: 'ipinfo',
            label: 'IPinfo',
            available: true,
            unavailable_reason: null,
            sends_peer_addresses: true,
            disclosure:
              'IPinfo asks the fixed HTTPS endpoint https://ipinfo.io/{ip}/json for each observed Peer public IP and keeps only the returned two-letter country code. It carries no token and sends no other Peer data.',
          },
          {
            provider: 'geojs',
            label: 'GeoJS',
            available: true,
            unavailable_reason: null,
            sends_peer_addresses: true,
            disclosure:
              'GeoJS asks the fixed HTTPS endpoint https://get.geojs.io/v1/ip/geo/{ip}.json for each observed Peer public IP and keeps only the returned two-letter country code. It carries no token and sends no other Peer data.',
          },
        ],
      }),
    }))

    await renderSettings()
    const local = screen.getByLabelText(/Local MMDB/) as HTMLInputElement
    expect(local.disabled).toBe(true)
    expect(screen.getByText(/No local GeoLite2 Country database/)).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Save Geo provider' }) as HTMLButtonElement).disabled).toBe(true)
    // Neither external provider needs a local database, so a deployment
    // without one can still make an informed choice for or against them.
    expect((screen.getByLabelText(/IPinfo/) as HTMLInputElement).disabled).toBe(false)
    expect((screen.getByLabelText(/GeoJS/) as HTMLInputElement).disabled).toBe(false)
  })

  it('offers IPinfo with the third-party consequence and only enables it on request', async () => {
    let putBody: unknown = null
    const ipinfoStatus = {
      ...GEO_STATUS,
      provider: 'ipinfo',
      provider_label: 'IPinfo',
      provider_generation: 2,
      state: 'current',
      build_epoch: null,
      digest: null,
      loaded_at: null,
      pending_lookup_count: 2,
      last_success_at: '2026-08-20T01:00:00Z',
    }
    let provider = 'disabled'
    mockFetch(successfulRoutes({
      '/api/admin/v1/geo': () => response(provider === 'disabled' ? GEO_STATUS : ipinfoStatus),
      '/api/admin/v1/geo/provider': async (request) => {
        putBody = await request.json()
        provider = 'ipinfo'
        return response({ geo: ipinfoStatus, audit_event_id: 91 })
      },
    }))

    await renderSettings()
    const geoCard = within(screen.getAllByRole('article')[2])
    const ipinfo = geoCard.getByLabelText(/IPinfo/) as HTMLInputElement
    // The Server never enables an external provider by itself.
    expect(ipinfo.checked).toBe(false)
    expect(geoCard.getByText(/Peer addresses never leave the Server/)).toBeTruthy()

    fireEvent.click(ipinfo)
    const save = geoCard.getByRole('button', { name: 'Save Geo provider' })
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(save)

    await waitFor(() => expect(putBody).toEqual({ provider: 'ipinfo' }))
    expect(await geoCard.findByText(/Geo provider is now IPinfo \(Audit #91\)/)).toBeTruthy()
    // The refreshed diagnostic states where Peer addresses go and reports the
    // real backlog; no database metadata is presented for a provider that
    // reads no database.
    await waitFor(() => {
      expect(geoCard.getByText('Sent to IPinfo')).toBeTruthy()
      expect(geoCard.getByText('2')).toBeTruthy()
      expect(geoCard.queryByText('Database age')).toBeNull()
    })
  }, 20_000)


  it('offers GeoJS with its own fixed destination and only enables it on request', async () => {
    let putBody: unknown = null
    const geojsStatus = {
      ...GEO_STATUS,
      provider: 'geojs',
      provider_label: 'GeoJS',
      provider_generation: 2,
      state: 'current',
      build_epoch: null,
      digest: null,
      loaded_at: null,
      pending_lookup_count: 4,
      cache_country_count: 3,
      last_success_at: '2026-08-20T02:00:00Z',
    }
    let provider = 'disabled'
    mockFetch(successfulRoutes({
      '/api/admin/v1/geo': () => response(provider === 'disabled' ? GEO_STATUS : geojsStatus),
      '/api/admin/v1/geo/provider': async (request) => {
        putBody = await request.json()
        provider = 'geojs'
        return response({ geo: geojsStatus, audit_event_id: 93 })
      },
    }))

    await renderSettings()
    const geoCard = within(screen.getAllByRole('article')[2])
    const geojs = geoCard.getByLabelText(/GeoJS/) as HTMLInputElement
    // The Server never enables an external provider by itself, and the
    // destination is stated before any selection.
    expect(geojs.checked).toBe(false)
    expect(geoCard.getByText(/get\.geojs\.io\/v1\/ip\/geo\/\{ip\}\.json/)).toBeTruthy()

    fireEvent.click(geojs)
    const save = geoCard.getByRole('button', { name: 'Save Geo provider' })
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(save)

    await waitFor(() => expect(putBody).toEqual({ provider: 'geojs' }))
    expect(await geoCard.findByText(/Geo provider is now GeoJS \(Audit #93\)/)).toBeTruthy()
    await waitFor(() => {
      expect(geoCard.getByText('Sent to GeoJS')).toBeTruthy()
      expect(geoCard.getByText('4')).toBeTruthy()
      expect(geoCard.getByText('3')).toBeTruthy()
      expect(geoCard.queryByText('Database age')).toBeNull()
    })
  }, 20_000)

  it('reports a rejected Geo provider change without hiding the other cards', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/geo/provider': () =>
        apiError('this Server has no configured local GeoLite2 Country database', ['provider']),
    }))

    await renderSettings()
    fireEvent.click(screen.getByLabelText(/Local MMDB/))
    const save = screen.getByRole('button', { name: 'Save Geo provider' })
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(save)

    expect(await screen.findByText(/no configured local GeoLite2 Country database/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Make Home Public' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Save History Window' })).toBeTruthy()
  })


  it.each([
    {
      failedPath: '/api/admin/v1/history-window',
      errorText: 'Unable to load the History Window.',
      otherControl: 'Make Home Public',
    },
    {
      failedPath: '/api/admin/v1/access-mode',
      errorText: 'Unable to load Site Access Mode.',
      otherControl: 'Save History Window',
    },
    {
      failedPath: '/api/admin/v1/geo',
      errorText: 'Unable to load Geo provider status.',
      otherControl: 'Save History Window',
    },
  ])('keeps the other Settings card usable when $failedPath fails', async ({ failedPath, errorText, otherControl }) => {
    mockFetch(successfulRoutes({
      [failedPath]: () => apiError('Configuration unavailable', [], 503),
    }))

    await renderSettings()

    expect(await screen.findByText(errorText)).toBeTruthy()
    expect(screen.getByRole('button', { name: otherControl })).toBeTruthy()
  })

  it('requires the authoritative impact preview and typed confirmation before saving History Window', async () => {
    let putBody: unknown = null
    let accessReads = 0
    let historyWindow = HISTORY_WINDOW
    mockFetch(successfulRoutes({
      '/api/admin/v1/access-mode': () => {
        accessReads += 1
        return response({ mode: 'private', authorizationGeneration: 0 })
      },
      '/api/admin/v1/history-window': async (request) => {
        if (request.method === 'PUT') {
          putBody = await request.json()
          historyWindow = {
            ...HISTORY_WINDOW,
            windowDays: 14,
            updatedAt: '2026-08-20T01:00:00Z',
          }
          return response({ window: historyWindow, auditEventId: 41 })
        }
        return response(historyWindow)
      },
    }))

    await renderSettings()
    const save = screen.getByRole('button', { name: 'Save History Window' })
    expect((save as HTMLButtonElement).disabled).toBe(true)

    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })
    expect((save as HTMLButtonElement).disabled).toBe(true)
    await screen.findByText('0 rows')
    await waitFor(() => expect((save as HTMLButtonElement).disabled).toBe(false))

    fireEvent.click(save)
    expect(await screen.findByText(/Audit #41/)).toBeTruthy()
    expect(putBody).toEqual({ windowDays: 14, confirmed: true })
    expect(accessReads).toBe(1)
  })

  it('rejects blank, non-integer, and out-of-bounds History Window values without requests or clamping', async () => {
    let putCalls = 0
    let impactCalls = 0
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': (request) => {
        if (request.method === 'PUT') putCalls += 1
        return response(HISTORY_WINDOW)
      },
      '/api/admin/v1/history-window/impact': async (request) => {
        impactCalls += 1
        const body = await request.json() as { windowDays: number }
        return response({ windowDays: body.windowDays, estimatedRows: 0, minDays: 1, maxDays: 30, notes: [] })
      },
    }))

    await renderSettings()
    const days = screen.getByLabelText('New window (days)')
    const confirmation = screen.getByLabelText('Type the change to confirm')
    const save = screen.getByRole('button', { name: 'Save History Window' })
    await screen.findByText('0 rows')
    expect(impactCalls).toBe(1)

    fireEvent.change(days, { target: { value: '' } })
    expect(screen.getByRole('alert').textContent).toContain('Enter a number of days')
    expect((days as HTMLInputElement).value).toBe('')
    expect(impactCalls).toBe(1)

    fireEvent.change(days, { target: { value: '1.5' } })
    expect(screen.getByRole('alert').textContent).toContain('whole number')
    fireEvent.change(confirmation, { target: { value: 'history-window 1.5' } })
    expect((save as HTMLButtonElement).disabled).toBe(true)

    fireEvent.change(days, { target: { value: '31' } })
    expect(screen.getByRole('alert').textContent).toContain('between 1 and 30 days')
    fireEvent.change(confirmation, { target: { value: 'history-window 31' } })
    fireEvent.click(save)
    expect((save as HTMLButtonElement).disabled).toBe(true)
    expect(putCalls).toBe(0)
    expect(impactCalls).toBe(1)
  })

  it('keeps History Window disabled when its impact preview fails', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window/impact': async (request) => {
        const body = await request.json() as { windowDays: number }
        return body.windowDays === 14
          ? apiError('Impact service unavailable', [], 503)
          : response({ windowDays: body.windowDays, estimatedRows: 0, minDays: 1, maxDays: 30, notes: [] })
      },
    }))

    await renderSettings()
    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })

    expect(await screen.findByText(/Unable to preview the impact/)).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Save History Window' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText('Private')).toBeTruthy()
  })

  it('cancels Site Access Mode without mutation and clears Public cache after confirmation', async () => {
    let putCalls = 0
    let historyReads = 0
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': () => {
        historyReads += 1
        return response(HISTORY_WINDOW)
      },
      '/api/admin/v1/access-mode': async (request) => {
        if (request.method === 'PUT') {
          putCalls += 1
          return response({ mode: 'public', authorizationGeneration: 1 })
        }
        return response({ mode: 'private', authorizationGeneration: 0 })
      },
    }))

    await renderSettings()
    const toggle = screen.getByRole('button', { name: 'Make Home Public' })
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true)

    fireEvent.click(toggle)
    expect(putCalls).toBe(0)
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('Anonymous visitors'))

    publicQueryClient.setQueryData(['public', 'networks', 0], { stale: true })
    fireEvent.click(toggle)
    expect(await screen.findByText(/Site Access Mode is now Public/)).toBeTruthy()
    expect(putCalls).toBe(1)
    expect(historyReads).toBe(1)
    expect(publicQueryClient.getQueryData(['public', 'networks', 0])).toBeUndefined()
  })

  it('shows a History Window page error without changing Site Access state', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': (request) => request.method === 'PUT'
        ? apiError('History Window save unavailable', [], 500)
        : response(HISTORY_WINDOW),
    }))

    await renderSettings()
    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })
    await screen.findByText('0 rows')
    await waitFor(() => expect(
      (screen.getByRole('button', { name: 'Save History Window' }) as HTMLButtonElement).disabled,
    ).toBe(false))
    fireEvent.click(screen.getByRole('button', { name: 'Save History Window' }))

    expect(await screen.findByText('History Window save unavailable')).toBeTruthy()
    expect(screen.getByText('Private')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Make Home Public' })).toBeTruthy()
  })

  it('reports each mutation error in its own card without hiding the other workflow', async () => {
    mockFetch(successfulRoutes({
      '/api/admin/v1/history-window': (request) => request.method === 'PUT'
        ? apiError('History Window rejected by Server', ['windowDays'])
        : response(HISTORY_WINDOW),
      '/api/admin/v1/access-mode': (request) => request.method === 'PUT'
        ? apiError('Site Access transition failed', [], 500)
        : response({ mode: 'private', authorizationGeneration: 0 }),
    }))

    await renderSettings()
    fireEvent.change(screen.getByLabelText('New window (days)'), { target: { value: '14' } })
    fireEvent.change(screen.getByLabelText('Type the change to confirm'), {
      target: { value: 'history-window 14' },
    })
    await screen.findByText('0 rows')
    await waitFor(() => expect((screen.getByRole('button', { name: 'Save History Window' }) as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(screen.getByRole('button', { name: 'Save History Window' }))
    expect(await screen.findByText('History Window rejected by Server')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Make Home Public' })).toBeTruthy()

    vi.spyOn(window, 'confirm').mockReturnValue(true)
    fireEvent.click(screen.getByRole('button', { name: 'Make Home Public' }))
    expect(await screen.findByText('Site Access transition failed')).toBeTruthy()
    expect((screen.getByLabelText('New window (days)') as HTMLInputElement).value).toBe('14')
    expect(screen.getByRole('heading', { level: 2, name: 'History Window' })).toBeTruthy()
  })
})
