import { execFileSync } from 'node:child_process'
import { chmodSync, copyFileSync, existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { findRepoRoot } from './repo-root'
import { attachedPort } from './worker-server'

/**
 * Build once for the whole run, before any worker boots a Server (issue #289):
 * the production WebUI bundle every Server hosts, and the platpulse-server
 * binary. The binary is copied into this run's own directory and published
 * through `PLATPULSE_E2E_SERVER_BIN` (Playwright hands the global setup's
 * environment to every worker), so another checkout's `cargo build` into a
 * shared `CARGO_TARGET_DIR` cannot replace it mid-run. Worker state
 * directories live under the same run directory, which the returned teardown
 * deletes even when a worker died before cleaning up after itself.
 */
export default function globalSetup(): () => void {
  const repoRoot = findRepoRoot()
  const webDir = join(repoRoot, 'platpulse-web')

  // In attach mode the Server on E2E_PORT serves its own bundle; the
  // disposable-Server harness still needs one, so build only if it is missing.
  if (attachedPort() === undefined || !existsSync(join(webDir, 'dist', 'index.html'))) {
    execFileSync('npm', ['run', 'build'], { cwd: webDir, stdio: ['ignore', 'ignore', 'inherit'] })
  }

  const runDir = mkdtempSync(join(tmpdir(), 'platpulse-e2e-run-'))
  const binary = join(runDir, 'platpulse-server')
  copyFileSync(buildServer(repoRoot), binary)
  chmodSync(binary, 0o755)
  process.env.PLATPULSE_E2E_SERVER_BIN = binary
  process.env.PLATPULSE_E2E_RUN_DIR = runDir

  return () => rmSync(runDir, { recursive: true, force: true })
}

/** `cargo build` the Server and return the executable cargo reports. */
function buildServer(repoRoot: string): string {
  const output = execFileSync(
    'cargo',
    ['build', '-p', 'platpulse-server', '--bin', 'platpulse-server', '--message-format=json-render-diagnostics'],
    { cwd: repoRoot, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 256 * 1024 * 1024 },
  )
  for (const line of output.split('\n')) {
    if (!line.startsWith('{')) continue
    const message = JSON.parse(line) as {
      reason?: string
      target?: { name?: string }
      executable?: string | null
    }
    if (
      message.reason === 'compiler-artifact' &&
      message.target?.name === 'platpulse-server' &&
      typeof message.executable === 'string'
    ) {
      return message.executable
    }
  }
  throw new Error('cargo build did not report a platpulse-server executable')
}
