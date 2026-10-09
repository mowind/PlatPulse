import { describe, expect, it } from 'vitest'
import type { PublicNode, PublicValidatorInsight } from './api/generated'
import {
  DEFAULT_HOME_FILTERS,
  HOME_SORTS,
  HOME_VIEWS,
  healthCategory,
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
  type HomeSort,
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
    peerCountries: { scope: 'unavailable', countries: [] },
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
      filters: { network: 'all', query: '', health: 'all', validator: 'all', sort: 'health', view: 'card' },
      rejected: [],
    })
  })

  it('restores every supported filter and sort value', () => {
    const search = new URLSearchParams(
      'network=home-convergence&q=Node+H&health=unknown&validator=not_validator&sort=head&view=list',
    )
    expect(readHomeFilters(search)).toEqual({
      filters: {
        network: 'home-convergence',
        query: 'Node H',
        health: 'unknown',
        validator: 'not_validator',
        sort: 'head',
        view: 'list',
      },
      rejected: [],
    })
  })

  it('falls back on an unsupported value, keeps the supported ones, and names what it could not honour', () => {
    const search = new URLSearchParams(
      'network=home-convergence&q=Node+H&health=degraded&validator=owning&sort=size&view=grid',
    )
    const state = readHomeFilters(search)
    expect(state.filters).toEqual({
      network: 'home-convergence',
      query: 'Node H',
      health: 'all',
      validator: 'all',
      sort: 'health',
      view: 'card',
    })
    expect(state.rejected).toEqual([
      { parameter: 'health', value: 'degraded' },
      { parameter: 'validator', value: 'owning' },
      { parameter: 'sort', value: 'size' },
      { parameter: 'view', value: 'grid' },
    ])
  })

  it('treats an empty parameter as absent rather than invalid', () => {
    const empty = new URLSearchParams('network=&health=&validator=&sort=&view=&q=')
    expect(readHomeFilters(empty).rejected).toEqual([])
    expect(readHomeFilters(empty).filters).toEqual(DEFAULT_HOME_FILTERS)
  })

  it('leaves a parameter Home does not own alone, so another surface can own it', () => {
    // `to` belongs to the same-tab return ticket (#224); Home neither reads nor
    // rejects it, and a Home write keeps it where its owner put it.
    const state = readHomeFilters(new URLSearchParams('to=7'))
    expect(state).toEqual({ filters: DEFAULT_HOME_FILTERS, rejected: [] })
  })

  it('owns the view parameter: card and list are the only views Home reads', () => {
    expect(readHomeFilters(new URLSearchParams('view=list')).filters.view).toBe('list')
    expect(readHomeFilters(new URLSearchParams('view=card')).filters.view).toBe('card')
    expect(readHomeFilters(new URLSearchParams('view=grid'))).toEqual({
      filters: DEFAULT_HOME_FILTERS,
      rejected: [{ parameter: 'view', value: 'grid' }],
    })
  })

  it('writes an ordinary Home URL: defaults omitted, unrelated parameters kept, canonical order', () => {
    const written = writeHomeFilters(
      filters({ network: 'home-convergence', query: 'Node H', health: 'healthy', sort: 'name', view: 'list' }),
      new URLSearchParams('view=card&to=7'),
    )
    expect(written.toString()).toBe(
      'to=7&network=home-convergence&q=Node+H&health=healthy&sort=name&view=list',
    )
    expect(writeHomeFilters(filters()).toString()).toBe('')
  })

  it('round-trips: what Home writes is what Home reads back', () => {
    const chosen = filters({ network: 'home-convergence', query: 'Node H', validator: 'unknown', sort: 'head', view: 'list' })
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

  it('keeps the existing Home health interpretation', () => {
    expect(healthTone('healthy')).toBe('good')
    expect(healthTone('unhealthy')).toBe('bad')
    expect(healthTone('unknown')).toBe('warn')
    // The Health sort is graded in the filter's own three words
    // (healthCategory), not in this tone: a health the Server reported as
    // Unknown sorts last like any other unattested value. The six orders are
    // asserted under "Home list selection".
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

  it('sorts by name, by Current Head, and by health with Unknown last', () => {
    const names = selectHomeRecords(records, filters({ sort: 'name' }))
    expect(names.matching.map((entry) => homeNodeLabel(entry.node))).toEqual(['Alpha', 'Beta', 'Gamma'])
    const heads = selectHomeRecords(records, filters({ sort: 'head' }))
    expect(heads.matching.map((entry) => entry.node.currentHead ?? null)).toEqual([900, 120, null])
    // Unhealthy leads, Healthy follows, and the health the Server reports as
    // Unknown sorts last: the same rule as the metric keys (#223, story 74).
    const health = selectHomeRecords(records, filters({ sort: 'health' }))
    expect(health.matching.map((entry) => homeNodeLabel(entry.node))).toEqual(['Gamma', 'Alpha', 'Beta'])
  })

  it('sorts by Peers, Process CPU and Process memory with Unknown last and a real zero kept as a value', () => {
    const busy = record({
      nodeId: 'busy',
      displayName: 'Busy',
      peers: { freshness: 'current', peerCount: 4, state: 'ok' },
      processCpuPercent: 62.5,
      processMemoryPercent: 38.25,
    })
    // 0 is a measurement, not a missing value: it stays a value and sorts above a
    // metric the Server has never attested.
    const zero = record({
      nodeId: 'zero',
      displayName: 'Zero',
      peers: { freshness: 'current', peerCount: 0, state: 'ok' },
      processCpuPercent: 0,
      processMemoryPercent: 0,
    })
    const never = record({
      nodeId: 'never',
      displayName: 'Never',
      peers: { freshness: 'unknown', peerCount: null, state: 'unknown' },
      processCpuPercent: null,
      processMemoryPercent: null,
    })
    const order = (sort: HomeSort) =>
      selectHomeRecords([never, busy, zero], filters({ sort })).matching.map((entry) => homeNodeLabel(entry.node))
    expect(order('peers')).toEqual(['Busy', 'Zero', 'Never'])
    expect(order('process_cpu')).toEqual(['Busy', 'Zero', 'Never'])
    expect(order('process_memory')).toEqual(['Busy', 'Zero', 'Never'])
  })

  it('reads each metric sort from its own field, so two metrics never share one rank', () => {
    // Every single-metric order above stays correct even if one metric sort read
    // another column, so these fixtures disagree with each other: Crossed leads on
    // Process CPU, Zero leads on Process memory, and Peers gives a third order
    // again (#223, stories 73 and 74).
    const busy = record({
      nodeId: 'busy',
      displayName: 'Busy',
      peers: { freshness: 'current', peerCount: 4, state: 'ok' },
      processCpuPercent: 62.5,
      processMemoryPercent: 38.25,
    })
    const zero = record({
      nodeId: 'zero',
      displayName: 'Zero',
      peers: { freshness: 'current', peerCount: 0, state: 'ok' },
      processCpuPercent: 0,
      processMemoryPercent: 74.5,
    })
    const crossed = record({
      nodeId: 'crossed',
      displayName: 'Crossed',
      peers: { freshness: 'current', peerCount: 2, state: 'ok' },
      processCpuPercent: 91,
      processMemoryPercent: 5.5,
    })
    const order = (sort: HomeSort) =>
      selectHomeRecords([crossed, zero, busy], filters({ sort })).matching.map((entry) => homeNodeLabel(entry.node))
    expect(order('peers')).toEqual(['Busy', 'Crossed', 'Zero'])
    expect(order('process_cpu')).toEqual(['Crossed', 'Busy', 'Zero'])
    expect(order('process_memory')).toEqual(['Zero', 'Busy', 'Crossed'])
  })

  it('breaks a tie on the Node identity, so any incoming order sorts the same', () => {
    const first = record({ nodeId: 'first', displayName: 'Same Name' })
    const second = record({ nodeId: 'second', displayName: 'Same Name' })
    const order = (records: HomeNodeRecord[]) =>
      selectHomeRecords(records, filters({ sort: 'name' })).matching.map((entry) => entry.node.nodeId)
    expect(order([first, second])).toEqual(['first', 'second'])
    expect(order([second, first])).toEqual(['first', 'second'])
  })
})

describe('Home list vocabulary', () => {
  it('carries the six documented sort keys in their documented order', () => {
    expect(HOME_SORTS).toEqual(['health', 'name', 'head', 'peers', 'process_cpu', 'process_memory'])
  })

  it('carries the two documented views, with the card view the default', () => {
    expect(HOME_VIEWS).toEqual(['card', 'list'])
    expect(DEFAULT_HOME_FILTERS.view).toBe('card')
  })
})
