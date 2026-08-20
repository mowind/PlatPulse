import type { ApiErrorBody } from './generated'
import { client } from './generated/client.gen'

export type ApiErrorKind = 'domain' | 'transport'

export class TransportError extends Error {
  readonly kind: ApiErrorKind
  readonly code: string
  readonly requestId: string | null
  readonly fields: string[]
  readonly status: number | null

  constructor(
    kind: ApiErrorKind,
    code: string,
    message: string,
    options: { requestId?: string | null; fields?: string[]; status?: number | null } = {},
  ) {
    super(message)
    this.name = 'ApiTransportError'
    this.kind = kind
    this.code = code
    this.requestId = options.requestId ?? null
    this.fields = options.fields ?? []
    this.status = options.status ?? null
  }
}

let activeAccessGeneration = 0
let interceptorInstalled = false

function installGenerationInterceptor(): void {
  if (interceptorInstalled) return
  interceptorInstalled = true
  client.interceptors.request.use((request) => {
    if (request.headers.has('X-PlatPulse-Access-Generation')) return request
    const headers = new Headers(request.headers)
    headers.set('X-PlatPulse-Access-Generation', String(activeAccessGeneration))
    return new Request(request, { headers })
  })
}

installGenerationInterceptor()

export function setActiveAccessGeneration(generation: number): void {
  activeAccessGeneration = generation
}

type GeneratedResult<T> = {
  data?: T
  error?: unknown
  response?: Response
}

function errorEnvelope(value: unknown): ApiErrorBody['error'] | undefined {
  const candidate = value as Partial<ApiErrorBody> | undefined
  return candidate?.error
}

/**
 * One boundary for generated OpenAPI operations. It preserves typed success
 * values, accepts generated 204/bodyless successes when requested, and maps
 * both server envelopes and browser transport failures to a safe error type.
 */
export async function requestGenerated<T>(
  operation: () => Promise<GeneratedResult<T>>,
  fallbackMessage: string,
  options: { allowEmpty?: boolean } = {},
): Promise<T> {
  let result: GeneratedResult<T>
  try {
    result = await operation()
  } catch (caught) {
    if (caught instanceof DOMException && caught.name === 'AbortError') throw caught
    throw new TransportError('transport', 'network_unavailable', fallbackMessage)
  }

  if (result.error !== undefined) {
    const envelope = errorEnvelope(result.error)
    if (!envelope && result.response === undefined) {
      throw new TransportError('transport', 'network_unavailable', fallbackMessage)
    }
    throw new TransportError(
      'domain',
      envelope?.code ?? 'request_failed',
      envelope?.message ?? fallbackMessage,
      {
        requestId: envelope?.requestId,
        fields: envelope?.fields,
        status: result.response?.status,
      },
    )
  }

  if (options.allowEmpty && result.response?.status === 204) return undefined as T
  if (result.data !== undefined) return result.data
  if (options.allowEmpty && result.response?.ok !== false) return undefined as T

  throw new TransportError(
    'domain',
    'empty_response',
    fallbackMessage,
    { status: result.response?.status },
  )
}

export function requestHeaders(
  generation: number,
  revision?: number,
): Record<string, string> {
  return {
    'X-PlatPulse-Access-Generation': String(generation),
    ...(revision === undefined ? {} : { 'X-PlatPulse-Revision': String(revision) }),
  }
}
