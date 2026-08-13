import { useEffect, useState } from 'react'
import { live } from '../api/generated'

export type ServerStatus = 'checking' | 'live' | 'unavailable'

export function useServerStatus(): ServerStatus {
  const [status, setStatus] = useState<ServerStatus>('checking')
  useEffect(() => {
    let active = true
    const check = async () => {
      try {
        const { data, error } = await live()
        if (active) setStatus(error || !data ? 'unavailable' : 'live')
      } catch {
        if (active) setStatus('unavailable')
      }
    }
    void check()
    const timer = window.setInterval(check, 15000)
    return () => { active = false; window.clearInterval(timer) }
  }, [])
  return status
}

export function ServerStatusNotice() {
  const status = useServerStatus()
  if (status === 'live') return null
  return <p className="server-status" aria-live="polite">{status === 'checking' ? 'Checking server status…' : 'Server is temporarily unavailable. Retrying automatically.'}</p>
}
