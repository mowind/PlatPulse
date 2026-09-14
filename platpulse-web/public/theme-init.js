/*
 * Production theme pre-application (design webui.md §11.1 "Theme behavior").
 *
 * A same-origin classic script in <head> so the resolved theme, canvas, and
 * color-scheme are painted before the application module and the first frame.
 * The Server enforces `script-src 'self'` with no inline allowance, so this
 * cannot be an inline script; a module would be deferred, so it is served as a
 * plain synchronous script. Keep the key, the attribute, and the resolution
 * rules in sync with src/theme.ts.
 */
;(function () {
  var mode = 'auto'
  try {
    var stored = window.localStorage.getItem('platpulse.themeMode')
    if (stored === 'light' || stored === 'dark' || stored === 'auto') mode = stored
  } catch {
    // Unavailable storage defaults to Auto; switching still works.
  }
  var prefersDark =
    window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches
  var dark = mode === 'dark' || (mode === 'auto' && Boolean(prefersDark))
  var root = document.documentElement
  root.classList.toggle('dark', dark)
  root.style.colorScheme = dark ? 'dark' : 'light'
  root.setAttribute('data-theme-mode', mode)
})()
