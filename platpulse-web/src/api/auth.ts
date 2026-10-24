// Thin typed wrapper over the generated browser client for the human
// session lifecycle (design §10.2). Business code never copies DTO
// interfaces (design §8.4); it maps transport/API failures onto a single
// error type keyed by the Server's stable `code`.

import { loginHandler, logoutHandler, sessionHandler } from './generated'
import type { ApiErrorBody, SessionResponse } from './generated'
import { requestGenerated, TransportError } from './transport'

/** A failed auth call, keyed by the Server's stable error code. */
export class AuthApiError extends TransportError {
  constructor(code: string, message: string) {
    super('domain', code, message)
    this.name = 'AuthApiError'
  }
}

function toAuthError(error: unknown, fallbackCode: string): AuthApiError {
  if (error instanceof TransportError) return new AuthApiError(error.code, error.message)
  const apiError = error as ApiErrorBody | undefined
  if (apiError?.error?.code && apiError.error.message) {
    return new AuthApiError(apiError.error.code, apiError.error.message)
  }
  return new AuthApiError(fallbackCode, 'the server is unreachable')
}

/**
 * What one session probe established about the reader (issue #267).
 *
 * A probe only retires an access when the Server says so: a `401` whose
 * error envelope carries `auth_required` refuses the cookie. A Server that
 * cannot answer at all — restarting, a `502`/`503` from the reverse proxy, a
 * dropped connection, a database failure — and a `401` that is not the
 * Server's own refusal (an intermediary's login page, an unreadable body)
 * establishes nothing about the Session, so a caller keeps the access it
 * already has instead of signing the reader out (webui.md §3.3).
 */
export type SessionProbe =
  /** The Server reported a live session. */
  | { kind: 'session'; response: SessionResponse }
  /** The Server refused the cookie (`401` + `auth_required`): not signed in. */
  | { kind: 'unauthenticated' }
  /** The Server could not be asked. Access is unknown, not lost. */
  | { kind: 'unavailable'; error: TransportError }

/** Ask the Server whether this document holds a live session. */
export async function probeSession(signal?: AbortSignal): Promise<SessionProbe> {
  try {
    const response = await requestGenerated(
      () => sessionHandler({ signal }),
      'Unable to check the current session',
    )
    return { kind: 'session', response }
  } catch (caught) {
    if (caught instanceof DOMException && caught.name === 'AbortError') throw caught
    // Access is retired by the Server's own answer and nothing else: the
    // status alone is not that answer, because a proxy in front of a
    // restarting Server can also send a 401 (webui.md §3.3).
    if (
      caught instanceof TransportError &&
      caught.status === 401 &&
      caught.code === 'auth_required'
    ) {
      return { kind: 'unauthenticated' }
    }
    return {
      kind: 'unavailable',
      error:
        caught instanceof TransportError
          ? caught
          : new TransportError(
              'transport',
              'network_unavailable',
              'Unable to check the current session',
            ),
    }
  }
}

/** Log in and return the new session plus CSRF token. */
export async function login(
  username: string,
  password: string,
): Promise<SessionResponse> {
  try {
    return await requestGenerated(
      () => loginHandler({ body: { username, password } }),
      'Unable to sign in',
    )
  } catch (caught) {
    if (caught instanceof AuthApiError) throw caught
    throw toAuthError(caught, 'login_failed')
  }
}

/** Revoke the current session. Failures propagate so the client never
 * pretends to be signed out while the session is still valid server-side;
 * a missing/expired session (`auth_required`) counts as already logged
 * out. */
export async function logout(csrfToken: string): Promise<void> {
  try {
    await requestGenerated(
      () => logoutHandler({ headers: { 'X-CSRF-Token': csrfToken } }),
      'Unable to sign out',
      { allowEmpty: true },
    )
  } catch (caught) {
    if (caught instanceof AuthApiError) throw caught
    if (caught instanceof TransportError && caught.code === 'auth_required') return
    throw toAuthError(caught, 'logout_failed')
  }
}
