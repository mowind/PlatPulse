import type { ApiErrorBody } from './generated'
import { client } from './generated/client.gen'
import { useEffect, useState } from 'react'

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

export type RealtimeSurface = 'public' | 'admin'

const realtimeCursorHeaders: Record<RealtimeSurface, string> = {
  public: 'X-PlatPulse-Public-Realtime-Cursor',
  admin: 'X-PlatPulse-Admin-Realtime-Cursor',
}
const realtimeCursors: Record<RealtimeSurface, number | null> = {
  public: null,
  admin: null,
}
const realtimeCursorListeners: Record<RealtimeSurface, Set<(cursor: number) => void>> = {
  public: new Set(),
  admin: new Set(),
}

function captureRealtimeCursor(response: Response): void {
  for (const surface of ['public', 'admin'] as const) {
    const raw = response.headers.get(realtimeCursorHeaders[surface])
    const cursor = raw === null ? null : Number.parseInt(raw, 10)
    if (cursor === null || !Number.isSafeInteger(cursor) || cursor < 0) continue
    const previous = realtimeCursors[surface]
    if (previous !== null && cursor <= previous) continue
    realtimeCursors[surface] = cursor
    for (const listener of realtimeCursorListeners[surface]) listener(cursor)
  }
}

export function getRealtimeCursor(surface: RealtimeSurface): number | null {
  return realtimeCursors[surface]
}

export function useRealtimeCursor(surface: RealtimeSurface): number | null {
  const [cursor, setCursor] = useState(() => getRealtimeCursor(surface))
  useEffect(() => {
    setCursor(getRealtimeCursor(surface))
    const listener = (next: number) => setCursor(next)
    realtimeCursorListeners[surface].add(listener)
    return () => {
      realtimeCursorListeners[surface].delete(listener)
    }
  }, [surface])
  return cursor
}

/** Test seam for isolating transport state between in-process UI tests. */
export function resetRealtimeCursors(): void {
  realtimeCursors.public = null
  realtimeCursors.admin = null
}

function installGenerationInterceptor(): void {
  if (interceptorInstalled) return
  interceptorInstalled = true
  client.interceptors.request.use((request) => {
    if (request.headers.has('X-PlatPulse-Access-Generation')) return request
    const headers = new Headers(request.headers)
    headers.set('X-PlatPulse-Access-Generation', String(activeAccessGeneration))
    return new Request(request, { headers })
  })
  client.interceptors.response.use((response) => {
    captureRealtimeCursor(response)
    return response
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
