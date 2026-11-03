import { createServer } from 'node:net'

/**
 * Ask the OS for a free loopback port. The probe closes before the caller's
 * Server binds it, so a caller that can retry (e2e/worker-server.ts) treats a
 * Server that exits before readiness as a lost race and asks again.
 */
export async function freePort(): Promise<number> {
  return new Promise<number>((resolvePort, rejectPort) => {
    const probe = createServer()
    probe.once('error', rejectPort)
    probe.listen(0, '127.0.0.1', () => {
      const address = probe.address()
      if (address === null || typeof address === 'string') {
        probe.close()
        rejectPort(new Error('could not allocate an ephemeral port'))
        return
      }
      const port = address.port
      probe.close(() => resolvePort(port))
    })
  })
}
