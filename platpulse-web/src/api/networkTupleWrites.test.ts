import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { adminQueryClient } from './admin'
import { client } from './generated/client.gen'
import {
  checkNetworkTupleWrite,
  networkTupleWrite,
  resetNetworkTupleWrites,
  startNetworkTupleWrite,
  type NetworkTupleUpdate,
} from './networkTupleWrites'

const TEST_ORIGIN = 'http://platpulse.test'
const NETWORK_URL = '/api/admin/v1/networks/platon-e2e'
const GENERATION = 3
const UPDATE: NetworkTupleUpdate = {
  displayName: 'PlatON E2E Network',
  genesisHash: '0x' + '1'.repeat(64),
  chainId: 210426,
  p2pNetworkId: 1,
  addressHrp: 'lat',
}

function json(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

/** The complete error envelope every update_network refusal carries. */
function refusal(status: number, code: string): Response {
  return json({ error: { code, message: 'refused: ' + code, requestId: 'req-' + code, fields: [] } }, status)
}

/** Serves the update with `answer`; every read of the Network fails, so an
 *  unknown outcome cannot be settled by a read during the test. */
function serve(answer: () => Response | Promise<Response>) {
  const requests: string[] = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '')
      requests.push(request.method + ' ' + url)
      if (url === NETWORK_URL && request.method === 'PUT') return answer()
      return json({ error: { code: 'unavailable', message: 'read failed', requestId: 'r', fields: [] } }, 503)
    }),
  )
  return requests
}

async function send() {
  const pending = startNetworkTupleWrite({
    generation: GENERATION,
    networkKey: 'platon-e2e',
    update: UPDATE,
    csrfToken: 'csrf-token',
  })
  expect(pending).not.toBeNull()
  return pending!
}

beforeEach(() => {
  client.setConfig({ baseUrl: TEST_ORIGIN })
})

afterEach(() => {
  vi.unstubAllGlobals()
  adminQueryClient.clear()
  resetNetworkTupleWrites()
})

describe('Network tuple update outcome', () => {
  // Every refusal update_network (and the guards in front of it) answers
  // before anything is committed: status, code, and the complete envelope.
  it.each([
    [400, 'invalid_network_tuple'],
    [400, 'empty_update'],
    [400, 'invalid_json'],
    [401, 'auth_required'],
    [403, 'csrf_validation_failed'],
    [403, 'owner_required'],
    [403, 'untrusted_proxy_headers'],
    [403, 'proxy_scheme_required'],
    [404, 'not_found'],
    [503, 'unavailable'],
    [503, 'inventory_cutover_required'],
  ])('takes the Server refusal %i %s as not applied and lifts the hold', async (status, code) => {
    const requests = serve(() => refusal(status, code))
    const outcome = await send()
    expect(outcome.kind).toBe('rejected')
    expect(networkTupleWrite(GENERATION, 'platon-e2e')).toBeNull()
    expect(requests.filter((request) => request.startsWith('PUT'))).toHaveLength(1)
  })

  it.each([
    ['a JSON gateway timeout', () => json({ error: { code: 'upstream_timeout', message: 'Gateway timed out' } }, 504)],
    ['a JSON bad gateway', () => json({ error: { code: 'bad_gateway' } }, 502)],
    ['an HTML gateway page', () => new Response('<html>502 Bad Gateway</html>', { status: 502, headers: { 'Content-Type': 'text/html' } })],
    ['a known code without the rest of the envelope', () => json({ error: { code: 'unavailable', message: 'Service Unavailable' } }, 503)],
    ['a known code whose fields are not a list', () => json({ error: { code: 'invalid_network_tuple', message: 'm', requestId: 'r', fields: 'chainId' } }, 400)],
    ['an unknown code in a complete envelope', () => refusal(400, 'invalid_display_name')],
    ['a gateway code in a complete envelope with a requestId', () => refusal(504, 'upstream_timeout')],
    ['a known code at a status the Server never sends it with', () => refusal(502, 'invalid_network_tuple')],
    ['an empty 500', () => new Response(null, { status: 500 })],
    ['no response at all', () => Promise.reject(new TypeError('Failed to fetch'))],
  ])('keeps %s unknown, held, and never re-sent', async (_name, answer) => {
    const requests = serve(answer)
    const outcome = await send()
    expect(outcome.kind).toBe('uncertain')
    await vi.waitFor(() =>
      expect(networkTupleWrite(GENERATION, 'platon-e2e')).toMatchObject({ phase: 'uncertain', checking: false, lastCheck: 'failed' }),
    )
    // Checking again only reads; a second update cannot start while held.
    await checkNetworkTupleWrite(GENERATION, 'platon-e2e')
    expect(
      startNetworkTupleWrite({ generation: GENERATION, networkKey: 'platon-e2e', update: UPDATE, csrfToken: 'csrf-token' }),
    ).toBeNull()
    expect(networkTupleWrite(GENERATION, 'platon-e2e')?.phase).toBe('uncertain')
    expect(requests.filter((request) => !request.startsWith('GET'))).toEqual(['PUT ' + NETWORK_URL])
  })
})
