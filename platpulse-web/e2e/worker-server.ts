import { spawn, type ChildProcess } from 'node:child_process'
import { createWriteStream, mkdirSync, mkdtempSync, rmSync, type WriteStream } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { freePort } from './free-port'
import { findRepoRoot } from './repo-root'

/**
 * One seeded Server per Playwright worker (issue #289).
 *
 * Every worker boots its own dev-mode platpulse-server through
 * e2e/start-server.sh — the same seed data the suite has always used — on a
 * port the OS picked, with a state directory and a Backup directory no other
 * worker can see. Tests that run in different workers therefore never share a
 * database, and a retry runs in a fresh worker against a freshly seeded Server.
 * Tests that run one after the other in the same worker still share it, as
 * they did before, so a test still prepares its own data.
 *
 * Attach mode: with `E2E_PORT` set, the suite runs in one worker against the
 * Server listening on that port (started by hand, or by the config's
 * `webServer`), the way the evidence recipes under docs/visual-migration/ run.
 */
export interface WorkerServer {
  /** Origin the worker's Server listens on; the `baseURL` of every test. */
  readonly baseUrl: string
  /** Stop the Server and its fixture refreshers and delete its directories. */
  stop(): Promise<void>
}

/** The port of the Server the suite attaches to, if `E2E_PORT` names one. */
export function attachedPort(): string | undefined {
  const port = process.env.E2E_PORT
  return port === undefined || port === '' ? undefined : port
}

const READY_TIMEOUT_MS = 150_000
const STOP_TIMEOUT_MS = 15_000
/** Attempts before giving up; a retry only follows a Server that exited early. */
const START_ATTEMPTS = 3

const delay = (milliseconds: number) =>
  new Promise<void>((resolveDelay) => setTimeout(resolveDelay, milliseconds))

/** Signal the script's whole process group: the Server and both refreshers. */
function signalGroup(child: ChildProcess, signal: NodeJS.Signals): void {
  if (child.pid === undefined) return
  try {
    process.kill(-child.pid, signal)
  } catch {
    // The group is already gone.
  }
}

async function stopGroup(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) {
    // The script's leader exited; a refresher may still be running.
    signalGroup(child, 'SIGKILL')
    return
  }
  const exited = new Promise<void>((resolveExit) => child.once('exit', () => resolveExit()))
  signalGroup(child, 'SIGTERM')
  const stopped = await Promise.race([exited.then(() => true), delay(STOP_TIMEOUT_MS).then(() => false)])
  // SIGKILL the group either way: the refreshers ignore nothing, but a slow
  // one must not outlive the directory it writes to.
  signalGroup(child, 'SIGKILL')
  if (!stopped) await exited
}

interface Attempt {
  readonly child: ChildProcess
  readonly baseUrl: string
  readonly root: string
}

async function startOnce(workerIndex: number, log: WriteStream): Promise<Attempt> {
  const repoRoot = findRepoRoot()
  const runDir = process.env.PLATPULSE_E2E_RUN_DIR
  const root = mkdtempSync(
    join(runDir === undefined || runDir === '' ? tmpdir() : runDir, `platpulse-e2e-worker-${workerIndex}-`),
  )
  const port = await freePort()
  const baseUrl = `http://127.0.0.1:${port}`
  log.write(`--- worker ${workerIndex}: starting a Server on ${baseUrl} in ${root}\n`)
  const child = spawn('bash', [join(repoRoot, 'platpulse-web', 'e2e', 'start-server.sh')], {
    cwd: repoRoot,
    // Its own process group, so stop() reaches the refreshers the script
    // leaves in the background, not only the Server it execs into.
    detached: true,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: {
      ...process.env,
      E2E_PORT: String(port),
      E2E_STATE_DIR: join(root, 'state'),
      E2E_BACKUP_DIR: join(root, 'backups'),
      E2E_SKIP_BUILD: '1',
      E2E_PARENT_PID: String(process.pid),
    },
  })
  child.stdout?.pipe(log, { end: false })
  child.stderr?.pipe(log, { end: false })

  const deadline = Date.now() + READY_TIMEOUT_MS
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      signalGroup(child, 'SIGKILL')
      rmSync(root, { recursive: true, force: true })
      throw new ServerExitedEarly(
        `worker ${workerIndex}: the Server on ${baseUrl} exited before readiness (${child.exitCode ?? child.signalCode})`,
      )
    }
    try {
      const response = await fetch(`${baseUrl}/health/ready`)
      if (response.ok) return { child, baseUrl, root }
    } catch {
      // Still seeding, or not listening yet.
    }
    await delay(250)
  }
  await stopGroup(child)
  rmSync(root, { recursive: true, force: true })
  throw new Error(`worker ${workerIndex}: the Server on ${baseUrl} was not ready within ${READY_TIMEOUT_MS} ms`)
}

class ServerExitedEarly extends Error {}

/**
 * Boot this worker's Server. Its output goes to `logPath` (under the
 * project's test-results directory, which CI uploads when a job fails).
 */
export async function startWorkerServer(workerIndex: number, logPath: string): Promise<WorkerServer> {
  const port = attachedPort()
  if (port !== undefined) {
    return { baseUrl: `http://127.0.0.1:${port}`, stop: async () => {} }
  }

  mkdirSync(join(logPath, '..'), { recursive: true })
  const log = createWriteStream(logPath, { flags: 'a' })
  let attempt: Attempt | undefined
  for (let tries = 1; attempt === undefined; tries += 1) {
    try {
      attempt = await startOnce(workerIndex, log)
    } catch (error) {
      // A Server that exits before readiness most likely lost its port to
      // another process between the probe and the bind; ask for a new one.
      if (!(error instanceof ServerExitedEarly) || tries >= START_ATTEMPTS) {
        log.end()
        throw new Error(`${(error as Error).message}; Server output: ${logPath}`, { cause: error })
      }
    }
  }
  const { child, baseUrl, root } = attempt
  return {
    baseUrl,
    async stop() {
      await stopGroup(child)
      rmSync(root, { recursive: true, force: true })
      log.end()
    },
  }
}
