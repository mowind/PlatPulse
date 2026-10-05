import { describe, expect, it } from 'vitest'
import type { PublicNode, PublicValidatorInsight } from './api/generated'
import {
  DEFAULT_HOME_FILTERS,
  healthCategory,
  healthRank,
  healthTone,
  homeNetworkScope,
  homeNodeLabel,
  homeNodeMatchesQuery,
  homeNodeSearchValues,
  readHomeFilters,
  selectHomeRecords,
  validatorCategory,
  writeHomeFilters,
  type HomeFilters,
  type HomeNodeRecord,
} from './homeFilters'

function node(overrides: Partial<PublicNode> = {}): PublicNode {
  return {
    consensus: { freshness: 'current', state: 'ok', validator: true },
    consensusState: 'ready',
    currentHead: 120,
    freshness: 'current',
    health: 'healthy',
    healthReason: 'RPC reachable',
    latestBlockTransactionCount: 12,
    networkKey: 'mainnet',
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

function insight(overrides: Partial<PublicValidatorInsight> = {}): PublicValidatorInsight {
  return {
    activity: 'producing',
    activityState: 'current',
    blockRateState: 'ok',
    counterState: 'ok',
    currentValidatorStatus: 'validator',
    currentValidatorStatusState: 'current',
    freshness: 'current',
    rankFreshness: 'current',
    rankState: 'ok',
    state: 'current',
    validatorId: 'validator-1',
    validatorNodeId: 'validator-node-1',
    ...overrides,
  }
}

const mainnet = { networkKey: 'mainnet', displayName: 'Mainnet' }
const convergence = {
  networkKey: 'home-convergence',
  displayName: 'Home Convergence Network With An Extremely Long Display Name',
}

function record(overrides: Partial<PublicNode> = {}, network = mainnet): HomeNodeRecord {
  return { network, node: node(overrides) }
}

const filters = (overrides: Partial<HomeFilters> = {}) => ({
  ...DEFAULT_HOME_FILTERS,
  ...overrides,
})

describe('Home filter vocabulary in a navigation URL', () => {
  it('reads the documented defaults from an empty URL', () => {
    expect(readHomeFilters(new URLSearchParams())).toEqual({
      filters: { network: 'all', query: '', health: 'all', validator: 'all', sort: 'health' },
      rejected: [],
    })
  })

  it('restores every supported filter and sort value', () => {
    const search = new URLSearchParams(
      'network=home-convergence&q=Node+H&health=unknown&validator=not_validator&sort=head',
    )
    expect(readHomeFilters(search)).toEqual({
      filters: {
        network: 'home-convergence',
        query: 'Node H',
        health: 'unknown',
        validator: 'not_validator',
        sort: 'head',
      },
      rejected: [],
    })
  })

  it('falls back on an unsupported value, keeps the supported ones, and names what it could not honour', () => {
    const search = new URLSearchParams(
      'network=home-convergence&q=Node+H&health=degraded&validator=owning&sort=peers',
    )
    const state = readHomeFilters(search)
    expect(state.filters).toEqual({
      network: 'home-convergence',
      query: 'Node H',
      health: 'all',
      validator: 'all',
      sort: 'health',
    })
    expect(state.rejected).toEqual([
      { parameter: 'health', value: 'degraded' },
      { parameter: 'validator', value: 'owning' },
      { parameter: 'sort', value: 'peers' },
    ])
  })

  it('treats an empty parameter as absent rather than invalid', () => {
    expect(readHomeFilters(new URLSearchParams('network=&health=&validator=&sort=&q=')).rejected).toEqual([])
    expect(readHomeFilters(new URLSearchParams('network=&health=&validator=&sort=&q=')).filters).toEqual(
      DEFAULT_HOME_FILTERS,
    )
  })

  it('ignores parameters Home does not own, so another surface can own them', () => {
    // `view` belongs to the card/list ticket (#223); Home neither reads nor rejects it.
    const state = readHomeFilters(new URLSearchParams('view=card&to=7'))
    expect(state).toEqual({ filters: DEFAULT_HOME_FILTERS, rejected: [] })
  })

  it('writes an ordinary Home URL: defaults omitted, unrelated parameters kept, canonical order', () => {
    const written = writeHomeFilters(
      filters({ network: 'home-convergence', query: 'Node H', health: 'healthy', sort: 'name' }),
      new URLSearchParams('view=card'),
    )
    expect(written.toString()).toBe('view=card&network=home-convergence&q=Node+H&health=healthy&sort=name')
    expect(writeHomeFilters(filters()).toString()).toBe('')
  })

  it('round-trips: what Home writes is what Home reads back', () => {
    const chosen = filters({ network: 'home-convergence', query: 'Node H', validator: 'unknown', sort: 'head' })
    expect(readHomeFilters(writeHomeFilters(chosen)).filters).toEqual(chosen)
  })
})

describe('Home Network scope', () => {
  it('accepts the selected Network while the projection is not available yet', () => {
    expect(homeNetworkScope('home-convergence', null)).toEqual({ network: 'home-convergence', rejected: null })
  })

  it('keeps a selected Network the projection still contains', () => {
    expect(homeNetworkScope('home-convergence', ['mainnet', 'home-convergence'])).toEqual({
      network: 'home-convergence',
      rejected: null,
    })
  })

  it('falls back to all Networks when the selected Network is no longer in the projection', () => {
    expect(homeNetworkScope('retired-network', ['mainnet', 'home-convergence'])).toEqual({
      network: 'all',
      rejected: { parameter: 'network', value: 'retired-network' },
    })
  })

  it('leaves the all-Networks selection alone', () => {
    expect(homeNetworkScope('all', ['mainnet'])).toEqual({ network: 'all', rejected: null })
    expect(homeNetworkScope('all', null)).toEqual({ network: 'all', rejected: null })
  })
})

describe('Home public search fields', () => {
  it('searches exactly the public Node name, the public Node ID, and the Network name', () => {
    expect(homeNodeSearchValues(record({ displayName: 'Node H', nodeId: 'node-h' }, convergence))).toEqual([
      'Node H',
      'node-h',
      convergence.displayName,
    ])
  })

  it('does not make a linked Validator, an Agent name, or an endpoint searchable', () => {
    const linked = record({ displayName: 'Node A', nodeId: 'node-a', validator: insight() })
    expect(homeNodeSearchValues(linked)).toEqual(['Node A', 'node-a', 'Mainnet'])
    expect(homeNodeMatchesQuery(linked, 'validator-1')).toBe(false)
    expect(homeNodeMatchesQuery(linked, 'validator-node-1')).toBe(false)
  })

  it('matches ordinary case-insensitive substrings', () => {
    const record1 = record({ displayName: 'Node H — Producing', nodeId: 'node-h' }, convergence)
    expect(homeNodeMatchesQuery(record1, 'node h')).toBe(true)
    expect(homeNodeMatchesQuery(record1, 'NODE-H')).toBe(true)
    expect(homeNodeMatchesQuery(record1, 'oducing')).toBe(true)
    expect(homeNodeMatchesQuery(record1, 'convergence')).toBe(true)
    expect(homeNodeMatchesQuery(record1, '   ')).toBe(true)
    expect(homeNodeMatchesQuery(record1, 'node p')).toBe(false)
  })

  it('falls back to the Node ID for a Node without a display name', () => {
    const unnamed = record({ displayName: null, nodeId: '0195f2a1-0065-4065-8065-000000000065' })
    expect(homeNodeLabel(unnamed.node)).toBe('0195f2a1-0065-4065-8065-000000000065')
    expect(homeNodeMatchesQuery(unnamed, '4065')).toBe(true)
  })
})

describe('Home health and Validator vocabulary', () => {
  it('reads the Server health value as healthy, unhealthy, or unknown and never as absence', () => {
    expect(healthCategory('healthy')).toBe('healthy')
    expect(healthCategory('Unhealthy')).toBe('unhealthy')
    expect(healthCategory('unknown')).toBe('unknown')
    expect(healthCategory('')).toBe('unknown')
    expect(healthCategory('degraded')).toBe('unknown')
  })

  it('reads Current Validator Status verbatim, with no Link as unknown', () => {
    expect(validatorCategory(node({ validator: insight() }))).toBe('validator')
    expect(validatorCategory(node({ validator: insight({ currentValidatorStatus: 'not_validator' }) }))).toBe(
      'not_validator',
    )
    expect(validatorCategory(node({ validator: insight({ currentValidatorStatus: 'unknown' }) }))).toBe('unknown')
    expect(validatorCategory(node({ validator: null }))).toBe('unknown')
    expect(validatorCategory(node())).toBe('unknown')
  })

  it('keeps the existing Home health interpretation and ordering', () => {
    expect(healthTone('healthy')).toBe('good')
    expect(healthTone('unhealthy')).toBe('bad')
    expect(healthTone('unknown')).toBe('warn')
    // The three-value economy the overview already reads: attention first,
    // unknown between it and healthy. #223 owns Unknown-last across six sorts.
    expect(healthRank('unhealthy')).toBeLessThan(healthRank('unknown'))
    expect(healthRank('unknown')).toBeLessThan(healthRank('healthy'))
  })
})

describe('Home list selection', () => {
  const alpha = record({ nodeId: 'alpha', displayName: 'Alpha', health: 'healthy', currentHead: 120 })
  const beta = record({ nodeId: 'beta', displayName: 'Beta', health: 'unknown', currentHead: null })
  const gamma = record(
    { nodeId: 'gamma', displayName: 'Gamma', health: 'unhealthy', currentHead: 900, validator: insight({ currentValidatorStatus: 'not_validator' }) },
    convergence,
  )
  const records = [alpha, beta, gamma]

  it('keeps the Network scope and the matching list apart', () => {
    const selected = selectHomeRecords(records, filters({ network: 'home-convergence' }))
    expect(selected.scoped.map((entry) => homeNodeLabel(entry.node))).toEqual(['Gamma'])
    expect(selected.matching.map((entry) => homeNodeLabel(entry.node))).toEqual(['Gamma'])
  })

  it('combines search and the health and Validator filters with AND semantics', () => {
    const selected = selectHomeRecords(records, filters({ query: 'a', health: 'healthy', validator: 'unknown' }))
    expect(selected.scoped).toHaveLength(3)
    expect(selected.matching.map((entry) => homeNodeLabel(entry.node))).toEqual(['Alpha'])
  })

  it('narrows the list without changing the Network scope', () => {
    const selected = selectHomeRecords(records, filters({ query: 'nothing matches this' }))
    expect(selected.scoped).toHaveLength(3)
    expect(selected.matching).toHaveLength(0)
  })

  it('sorts by name, by Current Head with Unknown last, and by health', () => {
    const names = selectHomeRecords(records, filters({ sort: 'name' }))
    expect(names.matching.map((entry) => homeNodeLabel(entry.node))).toEqual(['Alpha', 'Beta', 'Gamma'])
    const heads = selectHomeRecords(records, filters({ sort: 'head' }))
    expect(heads.matching.map((entry) => entry.node.currentHead ?? null)).toEqual([900, 120, null])
    const health = selectHomeRecords(records, filters({ sort: 'health' }))
    expect(health.matching.map((entry) => homeNodeLabel(entry.node))).toEqual(['Gamma', 'Beta', 'Alpha'])
  })

  it('keeps equal sort values in their incoming order', () => {
    const first = record({ nodeId: 'first', displayName: 'First', health: 'healthy' })
    const second = record({ nodeId: 'second', displayName: 'Second', health: 'healthy' })
    const sorted = selectHomeRecords([first, second], filters()).matching
    expect(sorted.map((entry) => homeNodeLabel(entry.node))).toEqual(['First', 'Second'])
  })
})
