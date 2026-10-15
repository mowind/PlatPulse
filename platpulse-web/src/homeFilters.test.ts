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

  it('owns the three sort keys #242 adds, and still refuses a key it never published', () => {
    for (const sort of ['disk', 'uptime', 'attention']) {
      expect(readHomeFilters(new URLSearchParams(`sort=${sort}`)).filters.sort).toBe(sort)
    }
    expect(readHomeFilters(new URLSearchParams('sort=disk_percent'))).toEqual({
      filters: DEFAULT_HOME_FILTERS,
      rejected: [{ parameter: 'sort', value: 'disk_percent' }],
    })
    // The new keys are ordinary Home parameters like the six before them: what
    // Home writes as a non-default selection is what it reads back.
    expect(writeHomeFilters(filters({ sort: 'attention' })).toString()).toBe('sort=attention')
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


  /** Every Node's Host storage percentage disagrees with its own Node data ratio,
   *  so a sort that read the Host column instead would rewrite this order. */
  it('sorts by Disk as the Node data ratio the card publishes, never as Host storage', () => {
    const full = record({
      nodeId: 'full',
      displayName: 'Full',
      nodeDataDirectorySizeBytes: 180,
      nodeDataDirectoryCapacityBytes: 200,
      hostStoragePercent: 5,
    })
    const half = record({
      nodeId: 'half',
      displayName: 'Half',
      nodeDataDirectorySizeBytes: 100,
      nodeDataDirectoryCapacityBytes: 200,
      hostStoragePercent: 95,
    })
    const zero = record({
      nodeId: 'zero',
      displayName: 'Zero',
      nodeDataDirectorySizeBytes: 0,
      nodeDataDirectoryCapacityBytes: 200,
      hostStoragePercent: 60,
    })
    const never = record({
      nodeId: 'never',
      displayName: 'Never',
      nodeDataDirectorySizeBytes: null,
      nodeDataDirectoryCapacityBytes: 200,
      hostStoragePercent: 99,
    })
    // A capacity that cannot hold anything has no ratio at all, so it is Unknown
    // rather than an infinite or zero share.
    const noCapacity = record({
      nodeId: 'no-capacity',
      displayName: 'No capacity',
      nodeDataDirectorySizeBytes: 12,
      nodeDataDirectoryCapacityBytes: 0,
      hostStoragePercent: 98,
    })
    // Byte counts no Server can attest carry no ratio either, so the order and the
    // card's own column agree on which Nodes have one at all.
    const negative = record({
      nodeId: 'negative',
      displayName: 'Negative',
      nodeDataDirectorySizeBytes: -12,
      nodeDataDirectoryCapacityBytes: 200,
      hostStoragePercent: 97,
    })
    const unmeasured = record({
      nodeId: 'unmeasured',
      displayName: 'Unmeasured',
      nodeDataDirectorySizeBytes: Number.NaN,
      nodeDataDirectoryCapacityBytes: Number.POSITIVE_INFINITY,
      hostStoragePercent: 96,
    })
    const order = (nodes: HomeNodeRecord[]) =>
      selectHomeRecords(nodes, filters({ sort: 'disk' })).matching.map((entry) => homeNodeLabel(entry.node))
    expect(order([never, noCapacity, zero, half, full])).toEqual(['Full', 'Half', 'Zero', 'Never', 'No capacity'])
    expect(order([negative, unmeasured, zero, full])).toEqual(['Full', 'Zero', 'Negative', 'Unmeasured'])
  })

  it('sorts by Uptime as the process uptime, keeping a real zero and every invalid value last', () => {
    const uptime = (processUptimeMs: number | null, nodeId: string, displayName: string) =>
      record({ nodeId, displayName, processUptimeMs })
    const order = (nodes: HomeNodeRecord[]) =>
      selectHomeRecords(nodes, filters({ sort: 'uptime' })).matching.map((entry) => homeNodeLabel(entry.node))
    expect(
      order([
        uptime(null, 'never', 'Never'),
        uptime(0, 'restarted', 'Restarted'),
        uptime(1_000, 'short', 'Short'),
        uptime(356_400_000, 'long', 'Long'),
      ]),
    ).toEqual(['Long', 'Short', 'Restarted', 'Never'])
    // An uptime no browser can rank is not a very large uptime: it is Unknown
    // like a missing one, and the Node identity keeps the two apart.
    expect(
      order([
        uptime(Number.POSITIVE_INFINITY, 'infinite', 'Infinite'),
        uptime(Number.NaN, 'not-a-number', 'Not a number'),
        uptime(1_000, 'short', 'Short'),
      ]),
    ).toEqual(['Short', 'Infinite', 'Not a number'])
  })

  it('orders Attention First as Unhealthy, Unknown, then Healthy, on the Server health alone', () => {
    // The healthy Node carries every signal that is not a health verdict — a
    // high process CPU, a resyncing process, a lagging height and an Observing
    // Activity — and none of them may promote it over an Unknown or an Unhealthy
    // Node (#242 reads the existing Server health classification and nothing
    // else).
    const busyHealthy = record({
      nodeId: 'busy-healthy',
      displayName: 'Busy Healthy',
      health: 'healthy',
      processCpuPercent: 99,
      resyncState: 'resyncing',
      syncState: 'syncing',
      currentHead: 1,
      validator: insight({ activity: 'observing' }),
    })
    const observing = record({
      nodeId: 'observing',
      displayName: 'Observing Unknown',
      health: 'unknown',
      validator: insight({ activity: 'producing' }),
    })
    const failed = record({ nodeId: 'failed', displayName: 'Failed', health: 'unhealthy' })
    const records = [busyHealthy, observing, failed]
    const order = (sort: HomeSort) =>
      selectHomeRecords(records, filters({ sort })).matching.map((entry) => homeNodeLabel(entry.node))
    expect(order('attention')).toEqual(['Failed', 'Observing Unknown', 'Busy Healthy'])
    // The delivered Health key keeps its own order: an attested Healthy Node
    // leads an Unknown one, and neither key changes the other.
    expect(order('health')).toEqual(['Failed', 'Busy Healthy', 'Observing Unknown'])
  })

  it('ranks every Activity × health combination by the Server health alone', () => {
    // Activity is a chain dimension rather than a health verdict (#242): a
    // Producing or an Observing Node is not thereby healthy, and no combination of
    // Activity and health may move a Node inside either health order. Every
    // combination Home can receive is ranked here.
    const activities = ['producing', 'verifying', 'observing', 'unknown']
    const healths = ['unhealthy', 'unknown', 'healthy']
    const nodes = activities.flatMap((activity) =>
      healths.map((health) =>
        record({
          nodeId: `${activity}-${health}`,
          displayName: `${activity} ${health}`,
          health,
          validator: insight({ activity, activityState: 'current' }),
        }),
      ),
    )
    const order = (sort: HomeSort) => selectHomeRecords(nodes, filters({ sort })).matching
    const words = (sort: HomeSort) => order(sort).map((entry) => healthCategory(entry.node.health))
    const ids = (sort: HomeSort) => order(sort).map((entry) => entry.node.nodeId)
    const times = (word: string) => Array.from({ length: activities.length }, () => word)
    expect(words('attention')).toEqual([...times('unhealthy'), ...times('unknown'), ...times('healthy')])
    expect(words('health')).toEqual([...times('unhealthy'), ...times('healthy'), ...times('unknown')])
    // Inside a group the Activity decides nothing either: both keys keep the
    // Node-identity order of each health group they read.
    expect(ids('attention').slice(0, activities.length)).toEqual(ids('health').slice(0, activities.length))
    expect(ids('attention').slice(activities.length * 2)).toEqual(ids('health').slice(activities.length, activities.length * 2))
  })

  it('breaks a tie on the Node identity under every key, so a refetch cannot reorder the list', () => {
    // One shared set with equal values under every key — the same health, the
    // same Node data ratio, the same uptime — so each key can only fall back to
    // the Node identity, and each input order must produce the same output.
    const first = record({
      nodeId: 'first',
      displayName: 'Same Name',
      nodeDataDirectorySizeBytes: 50,
      nodeDataDirectoryCapacityBytes: 100,
      processUptimeMs: 1_000,
    })
    const second = record({
      nodeId: 'second',
      displayName: 'Same Name',
      nodeDataDirectorySizeBytes: 50,
      nodeDataDirectoryCapacityBytes: 100,
      processUptimeMs: 1_000,
    })
    const third = record({
      nodeId: 'third',
      displayName: 'Same Name',
      nodeDataDirectorySizeBytes: 50,
      nodeDataDirectoryCapacityBytes: 100,
      processUptimeMs: 1_000,
    })
    const order = (nodes: HomeNodeRecord[], sort: HomeSort) =>
      selectHomeRecords(nodes, filters({ sort })).matching.map((entry) => entry.node.nodeId)
    const permutations = [
      [first, second, third],
      [third, first, second],
      [second, third, first],
      [third, second, first],
    ]
    for (const sort of HOME_SORTS) {
      expect(order(permutations[0], sort), sort).toEqual(['first', 'second', 'third'])
      for (const permutation of permutations.slice(1)) {
        expect(order(permutation, sort), sort).toEqual(['first', 'second', 'third'])
      }
    }
  })


  it('breaks a tie on the displayed name before the Node identity, so equal values read as a name list', () => {
    // The primary keys are equal here (one metric value, one health grade, one
    // Node data ratio), so only the tie-break decides. It reads the displayed name
    // first — the name a reader hunts — and only then the Node identity, so a
    // group of Nodes that share a value reads the way a name list does instead of
    // in deployment identity order.
    const zebra = record({ nodeId: 'node-3', displayName: 'Zebra', processCpuPercent: 10 })
    const alpha = record({ nodeId: 'node-9', displayName: 'Alpha', processCpuPercent: 10 })
    const beta = record({ nodeId: 'node-1', displayName: 'beta', processCpuPercent: 10 })
    const order = (nodes: HomeNodeRecord[], sort: HomeSort) =>
      selectHomeRecords(nodes, filters({ sort })).matching.map((entry) => entry.node.nodeId)
    for (const sort of HOME_SORTS) {
      expect(order([zebra, alpha, beta], sort), sort).toEqual(['node-9', 'node-1', 'node-3'])
      expect(order([beta, zebra, alpha], sort), sort).toEqual(['node-9', 'node-1', 'node-3'])
    }
  })

  it('keeps the Node identity as the last word, so two Nodes of one name never swap', () => {
    // A name tie is still total: nothing but `nodeId` remains, so the order
    // cannot depend on the order the projection arrived in.
    const second = record({ nodeId: 'second', displayName: 'Same Name', processCpuPercent: 10 })
    const first = record({ nodeId: 'first', displayName: 'Same Name', processCpuPercent: 10 })
    const order = (nodes: HomeNodeRecord[], sort: HomeSort) =>
      selectHomeRecords(nodes, filters({ sort })).matching.map((entry) => entry.node.nodeId)
    for (const sort of HOME_SORTS) {
      expect(order([second, first], sort), sort).toEqual(['first', 'second'])
    }
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
  it('carries the nine documented sort keys in their documented order', () => {
    // The three #242 adds are appended after the six #223 delivered, so every
    // existing key, direction and accessible name keeps its place.
    expect(HOME_SORTS).toEqual([
      'health',
      'name',
      'head',
      'peers',
      'process_cpu',
      'process_memory',
      'disk',
      'uptime',
      'attention',
    ])
    expect(DEFAULT_HOME_FILTERS.sort).toBe('health')
  })

  it('carries the two documented views, with the card view the default', () => {
    expect(HOME_VIEWS).toEqual(['card', 'list'])
    expect(DEFAULT_HOME_FILTERS.view).toBe('card')
  })
})
