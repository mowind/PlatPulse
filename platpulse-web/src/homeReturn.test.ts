import { describe, expect, it } from 'vitest'
import type { PublicNode } from './api/generated'
import type { HomeNodeRecord } from './homeFilters'
import {
  homeReturnFallbackIndex,
  homeReturnHref,
  homeReturnNotice,
  homeReturnState,
  homeReturnTab,
  landHomeReturn,
  readHomeReturn,
  syncHomeReturnReader,
  type HomeReturn,
} from './homeReturn'

function node(overrides: Partial<PublicNode> = {}): PublicNode {
  return {
    consensus: { freshness: 'current', state: 'ok', validator: true },
    consensusState: 'ready',
    currentHead: 120,
    freshness: 'current',
    health: 'healthy',
    healthReason: 'RPC reachable',
    latestBlockTransactionCount: 12,
    networkKey: 'home-convergence',
    networkReferenceConfidence: 'high',
    nodeId: 'node-a',
    peers: { freshness: 'current', peerCount: 8, state: 'ok' },
    processState: 'running',
    resyncState: 'idle',
    rpcState: 'connected',
    syncState: 'synced',
    ...overrides,
  }
}

function record(nodeId: string, displayName: string): HomeNodeRecord {
  return {
    network: { networkKey: 'home-convergence', displayName: 'Home Convergence' },
    node: node({ nodeId, displayName }),
  }
}

const records: HomeNodeRecord[] = [
  record('node-h', 'Node H'),
  record('node-k', 'Node K'),
  record('node-l', 'Node L'),
  record('node-m', 'Node M'),
]

// The token of the document these departures belong to: a departure is read
// back by the document that wrote it, and by no other.
const TAB = 'reading-1'

function departure(overrides: Partial<HomeReturn> = {}): HomeReturn {
  return { nodeId: 'node-l', search: '', position: null, tab: TAB, ...overrides }
}

describe('homeReturnState', () => {
  it('records the Node, the reading it was read in, and its 1-based place', () => {
    expect(
      homeReturnState('node-l', new URLSearchParams('network=home-convergence&view=list'), 2, 6, TAB),
    ).toEqual({
      homeReturn: {
        nodeId: 'node-l',
        search: 'network=home-convergence&view=list',
        position: { index: 3, count: 6 },
        tab: TAB,
      },
    })
  })

  it('records no place for a list Home cannot express one in', () => {
    expect(homeReturnState('node-l', new URLSearchParams(), 0, 0, TAB).homeReturn.position).toBeNull()
    expect(homeReturnState('node-l', new URLSearchParams(), 6, 6, TAB).homeReturn.position).toBeNull()
    expect(homeReturnState('node-l', new URLSearchParams(), -1, 6, TAB).homeReturn.position).toBeNull()
  })
})

describe('readHomeReturn', () => {
  it('reads back the departure it published', () => {
    expect(
      readHomeReturn(homeReturnState('node-l', new URLSearchParams('sort=peers'), 1, 6, TAB), TAB),
    ).toEqual({
      nodeId: 'node-l',
      search: 'sort=peers',
      position: { index: 2, count: 6 },
      tab: TAB,
    })
  })

  it('refuses a history entry that carries no return this surface can locate', () => {
    expect(readHomeReturn(undefined, TAB)).toBeNull()
    expect(readHomeReturn(null, TAB)).toBeNull()
    expect(readHomeReturn('/nodes/node-l', TAB)).toBeNull()
    expect(readHomeReturn({}, TAB)).toBeNull()
    // The login flow's own navigation state is not a return.
    expect(readHomeReturn({ from: '/', sessionExpired: false }, TAB)).toBeNull()
    expect(readHomeReturn({ homeReturn: null }, TAB)).toBeNull()
    expect(readHomeReturn({ homeReturn: { search: '', position: null, tab: TAB } }, TAB)).toBeNull()
    expect(readHomeReturn({ homeReturn: { nodeId: '', search: '', position: null, tab: TAB } }, TAB)).toBeNull()
    expect(readHomeReturn({ homeReturn: { nodeId: 'node l', search: '', position: null, tab: TAB } }, TAB)).toBeNull()
    expect(
      readHomeReturn({ homeReturn: { nodeId: 'n'.repeat(65), search: '', position: null, tab: TAB } }, TAB),
    ).toBeNull()
    expect(readHomeReturn({ homeReturn: { nodeId: 'node-l', search: 7, position: null, tab: TAB } }, TAB)).toBeNull()
    expect(
      readHomeReturn({ homeReturn: { nodeId: 'node-l', search: 'x'.repeat(4097), position: null, tab: TAB } }, TAB),
    ).toBeNull()
  })

  it('refuses a departure another document wrote', () => {
    // A history entry outlives the document that wrote it: a copied link, a
    // second tab, a reload, and a session the browser restored from disk all
    // read this state again, and none of them is the reading it describes.
    const earlier = homeReturnState('node-l', new URLSearchParams('view=list'), 1, 6, 'earlier-reading')
    expect(readHomeReturn(earlier, 'this-reading')).toBeNull()
    expect(readHomeReturn(earlier, 'earlier-reading')).not.toBeNull()
    // A departure written before this document had a token names no document.
    expect(readHomeReturn({ homeReturn: { nodeId: 'node-l', search: '', position: null, tab: undefined } }, TAB)).toBeNull()
  })

  it('keeps one token per reader, and starts a new reading when another one arrives', () => {
    expect(homeReturnTab()).not.toBe('')
    expect(homeReturnTab()).toBe(homeReturnTab())

    // The reader this document loaded for adopts the token it already holds: a
    // reload hands the same reader their own departure back.
    const loaded = homeReturnTab()
    syncHomeReturnReader('owner:1')
    expect(homeReturnTab()).toBe(loaded)

    // Another reader arrives where no Home shell is mounted, so the change is
    // announced here: what the previous reader left behind is not read back.
    syncHomeReturnReader('viewer:2')
    const rotated = homeReturnTab()
    expect(rotated).not.toBe(loaded)
    expect(
      readHomeReturn({ homeReturn: { nodeId: 'node-l', search: '', position: null, tab: loaded } }, rotated),
    ).toBeNull()

    // The same reader seen again is not a new reading: an access re-check
    // republishes their own data and owes them their own return.
    syncHomeReturnReader('viewer:2')
    expect(homeReturnTab()).toBe(rotated)
  })

  it('keeps the Node and drops only a place that never named one', () => {
    const expected = { nodeId: 'node-l', search: '', position: null, tab: TAB }
    expect(readHomeReturn({ homeReturn: { nodeId: 'node-l', search: '', position: { index: 0, count: 6 }, tab: TAB } }, TAB)).toEqual(expected)
    expect(readHomeReturn({ homeReturn: { nodeId: 'node-l', search: '', position: { index: 7, count: 6 }, tab: TAB } }, TAB)).toEqual(expected)
    expect(readHomeReturn({ homeReturn: { nodeId: 'node-l', search: '', position: { index: 1.5, count: 6 }, tab: TAB } }, TAB)).toEqual(expected)
    expect(readHomeReturn({ homeReturn: { nodeId: 'node-l', search: '', position: '3/6', tab: TAB } }, TAB)).toEqual(expected)
  })
})

describe('homeReturnHref', () => {
  it('opens the plain Home when there is no departure and nothing to restore', () => {
    expect(homeReturnHref(null)).toBe('/')
    expect(homeReturnHref(departure())).toBe('/')
  })

  it("rebuilds the reading Home was showing out of Home's own vocabulary", () => {
    expect(
      homeReturnHref(
        departure({
          search: 'network=home-convergence&q=Node+H&health=healthy&sort=name&view=list',
          position: { index: 3, count: 6 },
        }),
      ),
    ).toBe('/?network=home-convergence&q=Node+H&health=healthy&sort=name&view=list')
  })

  it('drops what Home does not own, and refuses again what Home would refuse', () => {
    // Home refuses an unknown sort on the way out and says so; the arrival then
    // shows the same refusal instead of silently reading as the default.
    expect(
      homeReturnHref(departure({ search: 'sort=sideways&health=healthy&to=7&utm=x' })),
    ).toBe('/?health=healthy&sort=sideways')
  })

  it('writes no place to the URL in either direction', () => {
    const href = homeReturnHref(departure({ search: 'view=list', position: { index: 3, count: 6 } }))
    expect(href).toBe('/?view=list')
    expect(href).not.toMatch(/to=|pos=|node-l/)
  })
})

describe('homeReturnFallbackIndex', () => {
  it('keeps the place the Node held, clamped to the list Home has now', () => {
    expect(homeReturnFallbackIndex(6, { index: 3, count: 6 })).toBe(2)
    expect(homeReturnFallbackIndex(2, { index: 3, count: 6 })).toBe(1)
    expect(homeReturnFallbackIndex(6, { index: 1, count: 6 })).toBe(0)
  })

  it('chooses no row when there is no place or no list', () => {
    expect(homeReturnFallbackIndex(6, null)).toBeNull()
    expect(homeReturnFallbackIndex(0, { index: 3, count: 6 })).toBeNull()
  })
})

describe('landHomeReturn', () => {
  it('lands on the Node itself and says nothing', () => {
    expect(landHomeReturn(records, records, departure({ position: { index: 3, count: 4 } }))).toEqual({
      index: 2,
      notice: null,
    })
  })

  it('prefers the Node ID over a place the list has moved past', () => {
    // The list was refetched and reordered while the reader was away, so the
    // place the departure recorded now names a different Node: the Node wins.
    const reordered = [records[2], records[0], records[1], records[3]]
    expect(
      landHomeReturn(reordered, reordered, departure({ position: { index: 4, count: 4 } })),
    ).toEqual({ index: 0, notice: null })
  })

  it('shows the nearest match, and says so, when the restored filters hide the Node', () => {
    const hidden = records.filter((entry) => entry.node.nodeId !== 'node-l')
    const landing = landHomeReturn(records, hidden, departure({ position: { index: 3, count: 4 } }))
    expect(landing.index).toBe(2)
    expect(landing.notice).toBe(
      'Node L is no longer shown by these filters. Showing the nearest matching Active Node instead.',
    )
  })

  it('names the Node ID when this Home cannot show the Node at all', () => {
    const elsewhere = [records[0], records[1]]
    const landing = landHomeReturn(elsewhere, elsewhere, departure({ position: { index: 2, count: 4 } }))
    expect(landing.index).toBe(1)
    expect(landing.notice).toBe(
      'node-l is no longer in this view. Showing the nearest Active Node instead.',
    )
  })

  it('leaves an empty result to the empty state instead of inventing a row', () => {
    expect(landHomeReturn([], [], departure({ position: { index: 3, count: 4 } }))).toEqual({
      index: null,
      notice: 'node-l is no longer in this view.',
    })
  })
})

describe('homeReturnNotice', () => {
  it('tells the reader which Node is gone and what Home showed instead', () => {
    expect(homeReturnNotice('missing', 'node-l', true)).toBe(
      'node-l is no longer in this view. Showing the nearest Active Node instead.',
    )
    expect(homeReturnNotice('filtered', 'Node L', true)).toBe(
      'Node L is no longer shown by these filters. Showing the nearest matching Active Node instead.',
    )
    expect(homeReturnNotice('missing', 'node-l', false)).toBe('node-l is no longer in this view.')
    expect(homeReturnNotice('filtered', 'Node L', false)).toBe('Node L is no longer shown by these filters.')
  })
})
