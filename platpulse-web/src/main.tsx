import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import { schedulePreloadLazyRoutes } from './lazyRoute'
import './styles/emerald.css'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)

// Fetch the lazy route chunks (#275) after the first screen, so opening Login
// or an Admin page does not flash the Suspense "Loading…" fallback.
schedulePreloadLazyRoutes()
