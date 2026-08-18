// Thin typed wrapper over the generated browser client for the human
// session lifecycle (design §12.3/§12.4). Business code never copies DTO
// interfaces (design §13.4); it maps transport/API failures onto a single
// error type keyed by the Server's stable `code`.

import { loginHandler, logoutHandler, sessionHandler } from './generated'
import type { ApiErrorBody, SessionResponse } from './generated'

/** A failed auth call, keyed by the Server's stable error code. */
export class AuthApiError extends Error {
  readonly code: string

  constructor(code: string, message: string) {
    super(message)
    this.name = 'AuthApiError'
    this.code = code
  }
}

function toAuthError(error: unknown, fallbackCode: string): AuthApiError {
  const apiError = error as ApiErrorBody | undefined
  if (apiError?.error?.code && apiError.error.message) {
    return new AuthApiError(apiError.error.code, apiError.error.message)
  }
  return new AuthApiError(fallbackCode, 'the server is unreachable')
}

/** Current session and CSRF token, or null when unauthenticated. */
export async function fetchSession(signal?: AbortSignal): Promise<SessionResponse | null> {
  try {
    const { data } = await sessionHandler({ signal })
    if (data) return data
    return null
  } catch (caught) {
    if (caught instanceof DOMException && caught.name === 'AbortError') throw caught
    // Network failure: treat as unauthenticated; the login page will
    // surface a readable error if the Server is actually unreachable.
    return null
  }
}

/** Log in and return the new session plus CSRF token. */
export async function login(
  username: string,
  password: string,
): Promise<SessionResponse> {
  try {
    const { data, error } = await loginHandler({
      body: { username, password },
    })
    if (data) return data
    throw toAuthError(error, 'login_failed')
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
    const { error } = await logoutHandler({
      headers: { 'X-CSRF-Token': csrfToken },
    })
    // Any 2xx (including the bodyless 204) is success; only error
    // responses are meaningful here.
    if (!error) return
    const apiError = error as ApiErrorBody | undefined
    if (apiError?.error?.code === 'auth_required') return
    throw toAuthError(error, 'logout_failed')
  } catch (caught) {
    if (caught instanceof AuthApiError) throw caught
    throw toAuthError(caught, 'logout_failed')
  }
}
