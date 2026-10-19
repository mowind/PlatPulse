import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import { adminQueryClient } from '../api/admin'
import { client } from '../api/generated/client.gen'
import { ADMIN_SIDEBAR_QUERY } from './AdminLayout'

const TEST_ORIGIN = 'http://platpulse.test'
const OWNER_SESSION = {
  session: {
    userId: 'u1',
    username: 'admin',
    role: 'owner',
    createdAt: '2026-08-12T00:00:00Z',
    lastSeenAt: '2026-08-12T00:00:00Z',
    expiresAt: '2026-08-19T00:00:00Z',
  },
  csrfToken: 'csrf-token',
}

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

/** A controllable viewport: only the sidebar query follows `setWide`. */
function installViewport(initialWide: boolean) {
  let wide = initialWide
  const listeners = new Set<() => void>()
  vi.stubGlobal(
    'matchMedia',
    vi.fn((query: string) => ({
      get matches() {
        return query === ADMIN_SIDEBAR_QUERY ? wide : false
      },
      media: query,
      onchange: null,
      addEventListener: (_type: string, listener: () => void) => {
        if (query === ADMIN_SIDEBAR_QUERY) listeners.add(listener)
      },
      removeEventListener: (_type: string, listener: () => void) => listeners.delete(listener),
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => true,
    })),
  )
  return (next: boolean) =>
    act(() => {
      wide = next
      for (const listener of [...listeners]) listener()
    })
}

async function renderAdmin() {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init)
      const url = request.url.replace(TEST_ORIGIN, '').split('?')[0]
      if (url === '/api/public/v1/session') return jsonResponse(OWNER_SESSION, 200)
      return jsonResponse({ error: { code: 'not_found' } }, 404)
    }),
  )
  render(<App />)
  await act(async () => {
    window.history.pushState({}, '', '/admin/access/sessions')
    window.dispatchEvent(new PopStateEvent('popstate'))
    await Promise.resolve()
  })
  return screen.findByRole('button', { name: 'Menu' }, { timeout: 5000 })
}

beforeEach(() => {
  window.history.replaceState({}, '', '/')
  client.setConfig({ baseUrl: TEST_ORIGIN })
  document.body.style.overflow = 'scroll'
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  adminQueryClient.clear()
  document.body.style.overflow = ''
})

describe('Admin navigation drawer across the sidebar breakpoint', () => {
  it('ends the drawer when the sidebar takes over, restores the earlier scroll setting, and does not reopen it', async () => {
    const setWide = installViewport(false)
    const menu = await renderAdmin()
    const nav = screen.getByRole('navigation', { name: 'Admin' })
    fireEvent.click(menu)
    expect(menu.getAttribute('aria-expanded')).toBe('true')
    expect(document.body.style.overflow).toBe('hidden')
    const first = nav.querySelector<HTMLElement>('a[href]')!
    await waitFor(() => expect(document.activeElement).toBe(first))

    setWide(true)
    expect(menu.getAttribute('aria-expanded')).toBe('false')
    // The value set before the drawer opened, not a blanket reset.
    expect(document.body.style.overflow).toBe('scroll')
    // Focus stays on the link, now in the visible sidebar; the opener is
    // hidden at this width and is not focused.
    expect(document.activeElement).toBe(first)

    // The sidebar is ordinary navigation: Escape and Tab are not intercepted.
    const last = Array.from(nav.querySelectorAll<HTMLElement>('a[href]')).at(-1)!
    last.focus()
    const tab = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true })
    last.dispatchEvent(tab)
    expect(tab.defaultPrevented).toBe(false)
    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    let escapeReachedDocument = false
    const onDocumentKey = () => (escapeReachedDocument = true)
    document.addEventListener('keydown', onDocumentKey)
    last.dispatchEvent(escape)
    document.removeEventListener('keydown', onDocumentKey)
    expect(escapeReachedDocument).toBe(true)

    // Back to the narrow layout: the drawer stays closed.
    setWide(false)
    expect(menu.getAttribute('aria-expanded')).toBe('false')
    expect(document.body.style.overflow).toBe('scroll')
  })

  it('keeps the narrow-screen drawer contract: Escape closes, unlocks and returns focus to Menu', async () => {
    installViewport(false)
    const menu = await renderAdmin()
    const nav = screen.getByRole('navigation', { name: 'Admin' })
    fireEvent.click(menu)
    await waitFor(() => expect(document.activeElement).toBe(nav.querySelector('a[href]')))
    const last = Array.from(nav.querySelectorAll<HTMLElement>('a[href]')).at(-1)!
    last.focus()
    fireEvent.keyDown(last, { key: 'Tab' })
    expect(document.activeElement).toBe(nav.querySelector('a[href]'))
    fireEvent.keyDown(nav, { key: 'Escape' })
    expect(menu.getAttribute('aria-expanded')).toBe('false')
    expect(document.body.style.overflow).toBe('scroll')
    await waitFor(() => expect(document.activeElement).toBe(menu))
  })

  it('never treats the desktop sidebar as a drawer', async () => {
    installViewport(true)
    const menu = await renderAdmin()
    fireEvent.click(menu)
    expect(menu.getAttribute('aria-expanded')).toBe('false')
    expect(document.body.style.overflow).toBe('scroll')
  })
})
