/**
 * A local stand-in for a PlatScan deployment (issue #219).
 *
 * The Validator daily trend is written by the Server's *own* refresh loop: with
 * `[validator_provider]` configured, the Server discovers a Node's Validator
 * identity from its Agent report, then POSTs to the deployment's
 * `browser-server/staking/...` endpoints and stores one snapshot row per local
 * day. Acceptance evidence therefore has to travel through that loop instead of
 * mocking a fetch — this module replays the captured mainnet responses so the
 * loop has something real to talk to.
 *
 * The bodies are replayed verbatim from
 * `crates/platpulse-server/tests/fixtures/platscan-mainnet/`, the same captures
 * the Server's own adapter tests use: `totalCount` 240 over five ranking pages,
 * with the captured node id ranking first on page 1.
 */

import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { Socket } from 'node:net'
import { dirname, join } from 'node:path'

/** Resolved the same way `server-harness.ts` does: the repo root is the
 *  directory that holds both the Cargo workspace and the web package. */
function findRepoRoot(): string {
  let directory = process.cwd()
  for (;;) {
    if (existsSync(join(directory, 'Cargo.toml')) && existsSync(join(directory, 'platpulse-web'))) return directory
    const parent = dirname(directory)
    if (parent === directory) {
      throw new Error('could not locate the PlatPulse repository root from ' + process.cwd())
    }
    directory = parent
  }
}

export const PLATSCAN_FIXTURE_DIR = join(
  findRepoRoot(),
  'crates',
  'platpulse-server',
  'tests',
  'fixtures',
  'platscan-mainnet',
)

/** `POST {base_url}/browser-server/staking/stakingDetails` — one Validator's detail. */
const DETAIL_PATH = '/browser-server/staking/stakingDetails'
/** `POST {base_url}/browser-server/staking/aliveStakingList` — the ranking cohort, paged. */
const RANKING_PATH = '/browser-server/staking/aliveStakingList'

/** The ranking page the Server has to stop after: the capture holds 240 entries
 *  in pages of 50, so pages 1..5 are the complete cohort. */
const RANKING_PAGE_COUNT = 5

function fixtureText(name: string): string {
  return readFileSync(join(PLATSCAN_FIXTURE_DIR, name), 'utf8')
}

/** The Validator node id the capture was taken for, in the canonical
 *  `0x` + 128 lowercase hex form the Server stores. An Agent report whose
 *  Node announces this id as its enode pubkey is what makes the automatic link
 *  discoverable, so a spec can seed a `validators` row for it. */
export const PLATSCAN_CAPTURE_NODE_ID: string = (() => {
  const nodeId = (JSON.parse(fixtureText('staking-details.json')) as { data?: { nodeId?: unknown } }).data?.nodeId
  if (typeof nodeId !== 'string' || !/^0x[0-9a-f]{128}$/.test(nodeId)) {
    throw new Error('staking-details.json does not carry a canonical 0x-prefixed node id')
  }
  return nodeId
})()

export interface PlatscanReplay {
  /** Base URL to bind a Network to in `[validator_provider] networks`. */
  baseUrl: string
  /** Stop listening; safe to call in a `finally` block. */
  stop(): Promise<void>
  /** How many stakingDetails requests the Server made (evidence of the real loop). */
  readonly detailRequests: number
  /** Every ranking `pageNo` the Server asked for, in request order. */
  readonly rankingPages: readonly number[]
  /** Every `nodeId` the Server asked about, in request order. */
  readonly requestedNodeIds: readonly string[]
}

function readBody(request: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = []
    request.on('data', (chunk: Buffer) => chunks.push(chunk))
    request.on('end', () => {
      resolve(Buffer.concat(chunks).toString('utf8'))
    })
    request.on('error', reject)
  })
}

function respond(response: ServerResponse, status: number, body: string): void {
  response.writeHead(status, {
    'content-type': 'application/json',
    'content-length': String(Buffer.byteLength(body)),
  })
  response.end(body)
}

/**
 * Start the replay on an ephemeral 127.0.0.1 port. Only the two
 * `browser-server/staking` routes exist: anything else is a 404, and a non-POST
 * is a 405, so a spec cannot accidentally pass because some other Client
 * answered.
 */
export async function startPlatscanReplay(): Promise<PlatscanReplay> {
  const detailBody = fixtureText('staking-details.json')
  const rankingBodies = new Map<number, string>()
  const pageFile = /^alive-staking-list-page-(\d+)\.json$/
  for (const name of readdirSync(PLATSCAN_FIXTURE_DIR)) {
    const match = pageFile.exec(name)
    if (match) rankingBodies.set(Number(match[1]), fixtureText(name))
  }
  if (rankingBodies.size !== RANKING_PAGE_COUNT) {
    throw new Error(
      'expected ' + RANKING_PAGE_COUNT + ' captured ranking pages, found ' + String(rankingBodies.size),
    )
  }

  const detailRequests = { count: 0 }
  const rankingPages: number[] = []
  const requestedNodeIds: string[] = []

  const server = createServer((request, response) => {
    void handle(request, response)
  })
  // reqwest pools keep-alive connections; without closing them `stop()` would
  // wait for the pool to expire.
  const sockets = new Set<Socket>()
  server.on('connection', (socket) => {
    sockets.add(socket)
    socket.on('close', () => sockets.delete(socket))
  })

  async function handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const path = (request.url ?? '').split('?')[0]
    if (path !== DETAIL_PATH && path !== RANKING_PATH) {
      respond(response, 404, JSON.stringify({ error: 'unknown route' }))
      return
    }
    if (request.method !== 'POST') {
      respond(response, 405, JSON.stringify({ error: 'method not allowed' }))
      return
    }
    const body = await readBody(request)
    if (path === DETAIL_PATH) {
      detailRequests.count += 1
      let nodeId: unknown
      try {
        nodeId = (JSON.parse(body) as { nodeId?: unknown }).nodeId
      } catch {
        nodeId = undefined
      }
      if (typeof nodeId === 'string') requestedNodeIds.push(nodeId)
      // The capture is one Validator's detail. Any other node id is a node this
      // deployment has no data for, which real PlatScan answers as not found.
      if (nodeId !== PLATSCAN_CAPTURE_NODE_ID) {
        respond(response, 404, JSON.stringify({ error: 'unknown node' }))
        return
      }
      respond(response, 200, detailBody)
      return
    }
    let pageNo: unknown
    try {
      pageNo = (JSON.parse(body) as { pageNo?: unknown }).pageNo
    } catch {
      pageNo = undefined
    }
    const page = typeof pageNo === 'number' ? rankingBodies.get(pageNo) : undefined
    if (page === undefined) {
      respond(response, 404, JSON.stringify({ error: 'unknown ranking page' }))
      return
    }
    rankingPages.push(pageNo as number)
    respond(response, 200, page)
  }

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      server.removeListener('error', reject)
      resolve()
    })
  })
  const address = server.address()
  if (address === null || typeof address === 'string') {
    throw new Error('the PlatScan replay did not report a TCP address')
  }

  return {
    baseUrl: 'http://127.0.0.1:' + String(address.port),
    stop: async () => {
      const closed = new Promise<void>((resolve, reject) => {
        server.close((error) => {
          if (error) reject(error)
          else resolve()
        })
      })
      for (const socket of sockets) socket.destroy()
      await closed
    },
    get detailRequests(): number {
      return detailRequests.count
    },
    rankingPages,
    requestedNodeIds,
  }
}
