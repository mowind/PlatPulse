import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'

/**
 * Disposable Server harness for the management acceptance suite (issue #166,
 * part of #165).
 *
 * The fixed-viewport suite shares one long-lived Server and must not gain
 * Agent/Node state from a mutation test. This harness boots a second, entirely
 * disposable `platpulse-server` on an ephemeral port with its own temporary
 * state directory and SQLite database, so a test can onboard a throwaway Agent
 * through the real Admin/Agent HTTP API, submit a real report with the minted
 * credential, restart the process in place, and then remove every artifact
 * without touching the shared Server.
 *
 * It is deliberately built on the production binary and the real wire routes
 * (`/api/admin/v1/...`, `/api/agent/v1/...`), not on a test-only injection
 * seam, so the browser and REST assertions exercise the same boundary the
 * design specifies.
 */

export const HARNESS_OWNER_USERNAME = 'admin'
export const HARNESS_OWNER_PASSWORD = 'platpulse-harness-admin-2026'

// The report fixture declares the `platon-mainnet` key but a genesis hash that
// deliberately contradicts the Registry tuple, exactly like the Rust transfer
// acceptance fixture. Registering the real tuple keeps the Network resolvable
// while proving Node acceptance does not depend on Registry identity agreement;
// a transfer target's *matching* identity is proven separately.
const NETWORK_KEY = 'platon-mainnet'
const NETWORK_GENESIS =
  '0x0000000000000000000000000000000000000000000000000000000000000001'

/** Wire shapes consumed by the suite, limited to the fields it asserts. */
export interface ReportReceipt {
  disposition: string
  nodes: { current: string; rejections: { code: string }[] }[]
  samples: { disposition: string }[]
}
export interface ReportEnvelope {
  receipt: ReportReceipt
}
export interface EnrolledAgent {
  agentId: string
  credential: string
}

export interface DisposableServer {
  /** Origin the disposable Server listens on. */
  readonly baseUrl: string
  /** Mint an Enrollment Token and exchange it for a real Agent credential. */
  enrollAgent(): Promise<EnrolledAgent>
  /**
   * Submit the wire fixture with the Agent credential. An optional mutator
   * can inject evidence (for example a fatal spool state) before submission.
   */
  submitReport(
    agentId: string,
    credential: string,
    mutate?: (report: Record<string, unknown>) => void,
  ): Promise<ReportEnvelope>
  /** Assert an authenticated Admin GET status, returning the parsed body. */
  expectAdminGet(path: string, status: number): Promise<unknown>
  /** Kill and respawn the Server on the same state directory. */
  restart(): Promise<void>
  /** Stop the Server and delete every temporary artifact. */
  dispose(): Promise<void>
}

/** Walk up from the invocation directory so the harness does not depend on the
 *  Playwright cwd happening to be `platpulse-web`. */
function findRepoRoot(): string {
  let directory = process.cwd()
  for (;;) {
    if (existsSync(join(directory, 'Cargo.toml')) && existsSync(join(directory, 'platpulse-web'))) {
      return directory
    }
    const parent = dirname(directory)
    if (parent === directory) break
    directory = parent
  }
  throw new Error(`could not locate the PlatPulse repository root from ${process.cwd()}`)
}

const REPO_ROOT = findRepoRoot()
const SERVER_BINARY = join(REPO_ROOT, 'target', 'debug', 'platpulse-server')
const WEB_ROOT = join(REPO_ROOT, 'platpulse-web', 'dist')
const REPORT_FIXTURE = join(
  REPO_ROOT,
  'crates',
  'platpulse-core',
  'tests',
  'fixtures',
  'report_v1_minimal.json',
)

const delay = (milliseconds: number) =>
  new Promise<void>((resolveDelay) => setTimeout(resolveDelay, milliseconds))

async function freePort(): Promise<number> {
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

function ensureBuildInputs(): void {
  if (!existsSync(SERVER_BINARY)) {
    execFileSync('cargo', ['build', '-p', 'platpulse-server'], {
      cwd: REPO_ROOT,
      stdio: 'inherit',
    })
  }
  if (!existsSync(join(WEB_ROOT, 'index.html'))) {
    throw new Error(
      `the production WebUI build is missing at ${WEB_ROOT}; run "npm run build" in platpulse-web`,
    )
  }
  if (!existsSync(REPORT_FIXTURE)) {
    throw new Error(`report fixture is missing: ${REPORT_FIXTURE}`)
  }
}

function runCli(args: string[], stdin?: string): void {
  try {
    execFileSync(SERVER_BINARY, args, {
      input: stdin ?? '',
      stdio: ['pipe', 'pipe', 'pipe'],
    })
  } catch (error) {
    const failure = error as { stderr?: Buffer; stdout?: Buffer; message: string }
    const detail = failure.stderr?.toString().trim() || failure.stdout?.toString().trim()
    throw new Error(`platpulse-server ${args.join(' ')} failed: ${detail || failure.message}`, {
      cause: error,
    })
  }
}

function sessionCookie(response: Response): string {
  const headers = response.headers as Headers & { getSetCookie?: () => string[] }
  const values = headers.getSetCookie
    ? headers.getSetCookie()
    : [response.headers.get('set-cookie')].filter((value): value is string => value !== null)
  const value =
    values.find((candidate) => candidate.startsWith('platpulse_dev_session=')) ?? values[0]
  if (!value) throw new Error('login did not set a session cookie')
  return value.split(';', 1)[0]
}

async function readJson(response: Response): Promise<unknown> {
  const text = await response.text()
  if (text.length === 0) return null
  return JSON.parse(text) as unknown
}

/** Boot a disposable Server; delete it with {@link DisposableServer.dispose}. */
export async function startDisposableServer(): Promise<DisposableServer> {
  ensureBuildInputs()

  const port = await freePort()
  const baseUrl = `http://127.0.0.1:${port}`
  const stateDir = mkdtempSync(join(tmpdir(), 'platpulse-e2e-disposable-'))
  const logs: string[] = []
  let child: ChildProcess | undefined

  const capture = (stream: NodeJS.ReadableStream | null) => {
    stream?.on('data', (chunk: Buffer) => {
      logs.push(chunk.toString())
      if (logs.length > 200) logs.shift()
    })
  }

  const stop = async (target: ChildProcess | undefined): Promise<void> => {
    if (!target || target.exitCode !== null) return
    const exited = new Promise<void>((resolveExit) => target.once('exit', () => resolveExit()))
    target.kill('SIGTERM')
    const stopped = await Promise.race([
      exited.then(() => true),
      delay(15_000).then(() => false),
    ])
    if (!stopped) {
      target.kill('SIGKILL')
      await exited
    }
  }

  try {
    const backupDir = join(stateDir, 'backups')
    mkdirSync(backupDir, { recursive: true })
    const configPath = join(stateDir, 'server.toml')
    writeFileSync(
      configPath,
      [
        `state_dir = "${stateDir}"`,
        `db_path = "${join(stateDir, 'platpulse.db')}"`,
        `pepper_file = "${join(stateDir, 'server-pepper')}"`,
        `backup_dir = "${backupDir}"`,
        `web_root = "${WEB_ROOT}"`,
        `listen = "127.0.0.1:${port}"`,
        `public_base_url = "${baseUrl}"`,
        'development = true',
        '',
      ].join('\n'),
    )

    runCli(['init', '--config', configPath])
    runCli([
      'network',
      'create',
      '--config',
      configPath,
      '--key',
      NETWORK_KEY,
      '--display-name',
      'PlatON Mainnet',
      '--genesis-hash',
      NETWORK_GENESIS,
      '--chain-id',
      '210425',
      '--p2p-network-id',
      '210425',
      '--address-hrp',
      'lat',
    ])
    runCli(
      ['owner', 'create', '--config', configPath, '--username', HARNESS_OWNER_USERNAME],
      `${HARNESS_OWNER_PASSWORD}\n`,
    )

    const spawnServe = async (): Promise<ChildProcess> => {
      const serverProcess = spawn(SERVER_BINARY, ['serve', '--config', configPath], {
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      capture(serverProcess.stdout)
      capture(serverProcess.stderr)
      const deadline = Date.now() + 60_000
      while (Date.now() < deadline) {
        if (serverProcess.exitCode !== null) {
          throw new Error(
            `disposable Server exited before readiness (code ${serverProcess.exitCode}):\n${logs.join('')}`,
          )
        }
        try {
          const response = await fetch(`${baseUrl}/health/ready`)
          if (response.ok) return serverProcess
        } catch {
          // The listener is not accepting yet; keep polling the deadline.
        }
        await delay(200)
      }
      serverProcess.kill('SIGKILL')
      throw new Error(`disposable Server never became ready:\n${logs.join('')}`)
    }

    child = await spawnServe()

    const login = async () => {
      const response = await fetch(`${baseUrl}/api/public/v1/login`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', origin: baseUrl },
        body: JSON.stringify({
          username: HARNESS_OWNER_USERNAME,
          password: HARNESS_OWNER_PASSWORD,
        }),
      })
      if (!response.ok) throw new Error(`harness login failed: ${response.status}`)
      const body = (await readJson(response)) as { csrfToken: string }
      return { cookie: sessionCookie(response), csrf: body.csrfToken }
    }

    const adminRequest = async (
      method: 'GET' | 'POST',
      path: string,
      payload?: unknown,
    ): Promise<{ status: number; body: unknown }> => {
      const { cookie, csrf } = await login()
      const response = await fetch(`${baseUrl}${path}`, {
        method,
        headers: {
          cookie,
          origin: baseUrl,
          'x-csrf-token': csrf,
          ...(payload === undefined ? {} : { 'content-type': 'application/json' }),
        },
        body: payload === undefined ? undefined : JSON.stringify(payload),
      })
      return { status: response.status, body: await readJson(response) }
    }

    const enrollAgent = async (): Promise<EnrolledAgent> => {
      const token = await adminRequest('POST', '/api/admin/v1/agents/enroll-token', {
        expiresInHours: 24,
      })
      if (token.status !== 200) {
        throw new Error(`enroll-token failed: ${token.status} ${JSON.stringify(token.body)}`)
      }
      const enrollmentToken = (token.body as { token: string }).token
      const response = await fetch(`${baseUrl}/api/agent/v1/enroll`, {
        method: 'POST',
        headers: { authorization: `Bearer ${enrollmentToken}` },
      })
      if (!response.ok) throw new Error(`agent enrollment failed: ${response.status}`)
      const body = (await readJson(response)) as { agent_id: string; credential: string }
      return { agentId: body.agent_id, credential: body.credential }
    }

    const submitReport = async (
      agentId: string,
      credential: string,
      mutate?: (report: Record<string, unknown>) => void,
    ): Promise<ReportEnvelope> => {
      const report = JSON.parse(readFileSync(REPORT_FIXTURE, 'utf8')) as Record<string, unknown>
      report.agent_id = agentId
      mutate?.(report)
      const response = await fetch(`${baseUrl}/api/agent/v1/reports`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', authorization: `Bearer ${credential}` },
        body: JSON.stringify(report),
      })
      const body = await readJson(response)
      if (!response.ok) {
        throw new Error(`report submission failed: ${response.status} ${JSON.stringify(body)}`)
      }
      return body as ReportEnvelope
    }

    const expectAdminGet = async (path: string, status: number): Promise<unknown> => {
      const response = await adminRequest('GET', path)
      if (response.status !== status) {
        throw new Error(`expected GET ${path} -> ${status}, got ${response.status}`)
      }
      return response.body
    }

    return {
      baseUrl,
      enrollAgent,
      submitReport,
      expectAdminGet,
      async restart() {
        await stop(child)
        child = await spawnServe()
      },
      async dispose() {
        await stop(child)
        child = undefined
        rmSync(stateDir, { recursive: true, force: true })
      },
    }
  } catch (error) {
    // Setup failed before a caller could dispose: never orphan the state dir.
    await stop(child)
    rmSync(stateDir, { recursive: true, force: true })
    throw error
  }
}
