// Thin typed wrapper over the generated browser client for the human
// session lifecycle (design §12.3/§12.4). Business code never copies DTO
// interfaces (design §13.4); it maps transport/API failures onto a single
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

/** Current session and CSRF token, or null when unauthenticated. */
export async function fetchSession(signal?: AbortSignal): Promise<SessionResponse | null> {
  try {
    return await requestGenerated(
      () => sessionHandler({ signal }),
      'Unable to check the current session',
    )
  } catch (caught) {
    if (caught instanceof DOMException && caught.name === 'AbortError') throw caught
    // Session probes fail closed. The route gate will show the safe Guest
    // path while login still gets the normalized transport error.
    return null
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
