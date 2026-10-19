import { describe, expect, it } from 'vitest'
import { requestGenerated, TransportError } from './transport'

describe('generated API transport seam', () => {
  it('normalizes a typed domain error without exposing the response body', async () => {
    await expect(
      requestGenerated(
        async () => ({
          data: undefined,
          error: {
            error: {
              code: 'forbidden',
              message: 'Owner access required',
              requestId: 'req-1',
              fields: ['role'],
            },
          },
        }),
        'Unable to load Admin data',
      ),
    ).rejects.toMatchObject({
      name: 'ApiTransportError',
      kind: 'domain',
      code: 'forbidden',
      requestId: 'req-1',
      fields: ['role'],
      message: 'Owner access required',
    })
  })

  it('turns a generated-client network failure into a transport error', async () => {
    const failure = new TypeError('Failed to fetch')

    await expect(
      requestGenerated(async () => {
        throw failure
      }, 'Unable to load Public data'),
    ).rejects.toEqual(
      expect.objectContaining<Partial<TransportError>>({
        name: 'ApiTransportError',
        kind: 'transport',
        code: 'network_unavailable',
        message: 'Unable to load Public data',
      }),
    )
  })

  it('tells the Server\'s own refusal apart from an answer that is not the Server\'s', async () => {
    await expect(
      requestGenerated(
        async () => ({
          error: { error: { code: 'invalid_network_tuple', message: 'invalid', requestId: 'r1' } },
          response: new Response(null, { status: 400 }),
        }),
        'Unable to update the Network',
      ),
    ).rejects.toMatchObject({ kind: 'domain', code: 'invalid_network_tuple', status: 400, serverEnvelope: true })

    // A gateway page in the Server's place, and no response at all.
    await expect(
      requestGenerated(
        async () => ({ error: '<html>502 Bad Gateway</html>', response: new Response(null, { status: 502 }) }),
        'Unable to update the Network',
      ),
    ).rejects.toMatchObject({ kind: 'domain', code: 'request_failed', status: 502, serverEnvelope: false })
    await expect(
      requestGenerated(async () => {
        throw new TypeError('Failed to fetch')
      }, 'Unable to update the Network'),
    ).rejects.toMatchObject({ kind: 'transport', code: 'network_unavailable', serverEnvelope: false })
  })

  it('accepts bodyless generated success responses', async () => {
    await expect(
      requestGenerated(
        async () => ({ data: undefined, error: undefined, response: new Response(null, { status: 204 }) }),
        'Unable to sign out',
        { allowEmpty: true },
      ),
    ).resolves.toBeUndefined()

    await expect(
      requestGenerated(
        async () => ({ data: {}, error: undefined, response: new Response(null, { status: 204 }) }),
        'Unable to sign out',
        { allowEmpty: true },
      ),
    ).resolves.toBeUndefined()
  })
})
