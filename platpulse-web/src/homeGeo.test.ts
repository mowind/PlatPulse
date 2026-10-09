import { describe, expect, it } from 'vitest'
import type { PublicNetwork, PublicNode, PublicNodeDetail } from './api/generated'
import { geoMapStatus, homeGeoOverview, homeNodeSelectionGeoOverview, nodeGeoOverview } from './homeGeo'

type NetworkOverrides = {
  networkKey: string
  displayName?: string
  geo?: Record<string, unknown>
  peers?: Record<string, unknown>
}

function network({ networkKey, displayName = networkKey, geo = {}, peers = {} }: NetworkOverrides): PublicNetwork {
  return {
    networkKey,
    displayName,
    geo: { state: 'current', scope: 'complete', ...geo },
    peers: { state: 'ok', freshness: 'current', ...peers },
    nodes: [],
    validators: [],
  } as unknown as PublicNetwork
}

const se = { countryCode: 'SE', count: 3, staleCount: 0, centroidLat: 60.1282, centroidLon: 18.6435 }
const de = { countryCode: 'DE', count: 2, staleCount: 1, centroidLat: 51.1657, centroidLon: 10.4515 }
const unknownCountry = { countryCode: 'ZZ', count: 4, staleCount: 0, centroidLat: null, centroidLon: null }

describe('homeGeoOverview', () => {
  it('projects one selected Network with its own Geo Insight', () => {
    const overview = homeGeoOverview([
      network({
        networkKey: 'mainnet',
        displayName: 'PlatON Mainnet',
        geo: {
          state: 'stale',
          scope: 'partial',
          countries: [se],
          knownCountryCount: 3,
          unknownCountryCount: 2,
          unknownWithoutRemoteIpCount: 2,
          unknownWithPublicIpCount: 0,
          availablePeerCount: 5,
          attribution: 'GeoLite Data created by MaxMind',
          lastGoodAt: '2026-08-01T00:00:00Z',
        },
      }),
      network({ networkKey: 'testnet', displayName: 'Testnet', geo: { countries: [de] } }),
    ], 'mainnet')

    expect(overview.scopeLabel).toBe('PlatON Mainnet')
    expect(overview.unitsInScope).toBe(1)
    expect(overview.unitsWithBasis).toBe(1)
    expect(overview.state).toBe('stale')
    expect(overview.scope).toBe('partial')
    expect(overview.countries).toEqual([{ code: 'SE', count: 3, staleCount: 0, point: { lat: 60.1282, lon: 18.6435 } }])
    expect(overview.knownCountryCount).toBe(3)
    expect(overview.unknownCountryCount).toBe(2)
    expect(overview.availablePeerCount).toBe(5)
    expect(overview.attribution).toBe('GeoLite Data created by MaxMind')
    expect(overview.lastGoodAt).toBe('2026-08-01T00:00:00Z')
  })

  it('aggregates every Network for All Networks and keeps the per-Node Peer record basis', () => {
    const overview = homeGeoOverview([
      network({ networkKey: 'mainnet', displayName: 'Mainnet', geo: { countries: [se, de], knownCountryCount: 5, unknownCountryCount: 0, availablePeerCount: 5 } }),
      network({ networkKey: 'testnet', displayName: 'Testnet', geo: { countries: [se, unknownCountry], knownCountryCount: 3, unknownCountryCount: 4, availablePeerCount: 7 } }),
    ], 'all')

    expect(overview.scopeLabel).toBe('All Networks')
    expect(overview.unitsInScope).toBe(2)
    expect(overview.unitsWithBasis).toBe(2)
    // The same country observed on two Networks keeps both Networks' records.
    expect(overview.countries).toEqual([
      { code: 'SE', count: 6, staleCount: 0, point: { lat: 60.1282, lon: 18.6435 } },
      { code: 'ZZ', count: 4, staleCount: 0, point: null },
      { code: 'DE', count: 2, staleCount: 1, point: { lat: 51.1657, lon: 10.4515 } },
    ])
    expect(overview.knownCountryCount).toBe(8)
    expect(overview.unknownCountryCount).toBe(4)
    expect(overview.availablePeerCount).toBe(12)
  })

  it('never fabricates a country basis for a Network that has none', () => {
    const overview = homeGeoOverview([
      network({ networkKey: 'mainnet', displayName: 'Mainnet', geo: { scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } }),
      network({ networkKey: 'testnet', displayName: 'Testnet', geo: { countries: [de], knownCountryCount: 2, unknownCountryCount: 1, availablePeerCount: 3 } }),
    ], 'all')

    expect(overview.unitsInScope).toBe(2)
    expect(overview.unitsWithBasis).toBe(1)
    expect(overview.scope).toBe('partial')
    // Sums cover only the Networks that published a denominator.
    expect(overview.knownCountryCount).toBe(2)
    expect(overview.unknownCountryCount).toBe(1)
    expect(overview.countries).toEqual([{ code: 'DE', count: 2, staleCount: 1, point: { lat: 51.1657, lon: 10.4515 } }])
  })

  it('keeps an authoritative empty country set as a real zero, not Unknown', () => {
    const overview = homeGeoOverview([
      network({ networkKey: 'mainnet', geo: { countries: [], knownCountryCount: 0, unknownCountryCount: 0, availablePeerCount: 0, scope: 'complete' } }),
    ], 'all')

    expect(overview.countries).toEqual([])
    expect(overview.knownCountryCount).toBe(0)
    expect(overview.unknownCountryCount).toBe(0)
    expect(overview.scope).toBe('complete')
  })

  it('combines Geo states and scopes without letting one Network mask another', () => {
    const current = network({ networkKey: 'a', geo: { state: 'current', scope: 'complete' } })
    const stale = network({ networkKey: 'b', geo: { state: 'stale', scope: 'partial' } })
    const error = network({ networkKey: 'c', geo: { state: 'error', scope: 'unavailable' } })
    const disabled = network({ networkKey: 'd', geo: { state: 'disabled', scope: 'unavailable' } })

    expect(homeGeoOverview([current, stale], 'all').state).toBe('stale')
    expect(homeGeoOverview([stale, error], 'all').state).toBe('error')
    expect(homeGeoOverview([current, current], 'all').state).toBe('current')
    expect(homeGeoOverview([disabled, disabled], 'all').state).toBe('disabled')
    expect(homeGeoOverview([current, disabled], 'all').state).toBe('unknown')
    expect(homeGeoOverview([current, stale], 'all').scope).toBe('partial')
    expect(homeGeoOverview([error, current], 'all').scope).toBe('unavailable')
    expect(homeGeoOverview([current, current], 'all').scope).toBe('complete')
  })

  it('reports Peer observation freshness as its own dimension', () => {
    const current = network({ networkKey: 'a', peers: { state: 'ok', freshness: 'current' } })
    const stale = network({ networkKey: 'b', peers: { state: 'ok', freshness: 'stale' } })
    const never = network({ networkKey: 'c', peers: { state: 'unknown', freshness: 'unknown' } })

    expect(homeGeoOverview([current, current], 'all').peerObservation).toBe('current')
    expect(homeGeoOverview([stale, stale], 'all').peerObservation).toBe('stale')
    expect(homeGeoOverview([never], 'all').peerObservation).toBe('unknown')
    expect(homeGeoOverview([current, stale], 'all').peerObservation).toBe('mixed')
    // A freshness the Server did not establish is never read as Current.
    expect(homeGeoOverview([network({ networkKey: 'x', peers: { state: 'ok', freshness: 'weird' } })], 'all').peerObservation).toBe('unknown')
  })

  it('treats a country without both representative coordinates as unplottable', () => {
    const partialPoint = { countryCode: 'NO', count: 1, staleCount: 0, centroidLat: 60.472, centroidLon: null }
    const overview = homeGeoOverview([network({ networkKey: 'mainnet', geo: { countries: [partialPoint] } })], 'all')

    expect(overview.countries).toEqual([{ code: 'NO', count: 1, staleCount: 0, point: null }])
  })

  it('stays Unknown for an empty or unmatched scope instead of inventing zero', () => {
    const empty = homeGeoOverview([], 'all')
    expect(empty.state).toBe('unknown')
    expect(empty.scope).toBe('unavailable')
    expect(empty.countries).toEqual([])
    expect(empty.knownCountryCount).toBeNull()
    expect(empty.unknownCountryCount).toBeNull()
    expect(empty.unitsInScope).toBe(0)

    const unmatched = homeGeoOverview([network({ networkKey: 'mainnet' })], 'missing')
    expect(unmatched.scopeLabel).toBe('missing')
    expect(unmatched.unitsInScope).toBe(0)
    expect(unmatched.state).toBe('unknown')
  })

  it('keeps Server-provided reasons and ages for the worst retained state', () => {
    const overview = homeGeoOverview([
      network({ networkKey: 'a', geo: { state: 'current', countries: [se], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } }),
      network({
        networkKey: 'b',
        geo: {
          state: 'error',
          scope: 'unavailable',
          countries: null,
          knownCountryCount: null,
          errorReason: 'Geo lookup is unavailable',
          lastGoodAt: '2026-07-01T00:00:00Z',
        },
      }),
    ], 'all')

    expect(overview.state).toBe('error')
    expect(overview.errorReason).toBe('Geo lookup is unavailable')
    expect(overview.lastGoodAt).toBe('2026-07-01T00:00:00Z')
  })
})

type NodeOverrides = { geo?: Record<string, unknown>; displayName?: string; peers?: Record<string, unknown> }

function node(overrides: NodeOverrides = {}): PublicNodeDetail {
  return {
    nodeId: 'node-1',
    displayName: overrides.displayName ?? 'Validator A',
    peers: { state: 'ok', freshness: 'current', ...overrides.peers },
    geo: { state: 'current', scope: 'complete', ...overrides.geo },
  } as unknown as PublicNodeDetail
}

describe('nodeGeoOverview', () => {
  it('projects exactly one Node scope with its own Geo Insight', () => {
    const overview = nodeGeoOverview(node({
      geo: {
        state: 'stale',
        scope: 'complete',
        countries: [de, se],
        knownCountryCount: 5,
        unknownCountryCount: 2,
        unknownWithoutRemoteIpCount: 2,
        unknownWithPublicIpCount: 0,
        availablePeerCount: 7,
        attribution: 'GeoLite Data created by MaxMind',
        lastGoodAt: '2026-08-01T00:00:00Z',
      },
    }))

    expect(overview.scopeLabel).toBe('Validator A')
    // A Node scope is not a Home Network selection.
    expect(overview.scopeKey).toBeNull()
    expect(overview.unitsInScope).toBe(1)
    expect(overview.unitsWithBasis).toBe(1)
    expect(overview.state).toBe('stale')
    expect(overview.scope).toBe('complete')
    // The map always reads most-count-first, whatever order the Server sent.
    expect(overview.countries).toEqual([
      { code: 'SE', count: 3, staleCount: 0, point: { lat: 60.1282, lon: 18.6435 } },
      { code: 'DE', count: 2, staleCount: 1, point: { lat: 51.1657, lon: 10.4515 } },
    ])
    expect(overview.knownCountryCount).toBe(5)
    expect(overview.unknownCountryCount).toBe(2)
    expect(overview.unknownWithoutRemoteIpCount).toBe(2)
    expect(overview.availablePeerCount).toBe(7)
    expect(overview.attribution).toBe('GeoLite Data created by MaxMind')
    expect(overview.lastGoodAt).toBe('2026-08-01T00:00:00Z')
    expect(overview.peerObservation).toBe('current')
  })

  it('stays unobserved without a fabricated zero when the Node has no basis', () => {
    const overview = nodeGeoOverview(node({
      geo: { scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null },
    }))

    expect(overview.scope).toBe('unobserved')
    expect(overview.unitsWithBasis).toBe(0)
    expect(overview.knownCountryCount).toBeNull()
    expect(overview.unknownCountryCount).toBeNull()
    expect(overview.availablePeerCount).toBeNull()
    expect(overview.countries).toEqual([])
  })

  it('keeps a Disabled Geo state unavailable and a missing projection Unknown', () => {
    const disabled = nodeGeoOverview(node({ geo: { state: 'disabled', scope: 'unavailable', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } }))
    expect(disabled.state).toBe('disabled')
    expect(disabled.scope).toBe('unavailable')
    expect(disabled.availablePeerCount).toBeNull()

    const missing = nodeGeoOverview({ nodeId: 'node-2' } as unknown as PublicNodeDetail)
    expect(missing.state).toBe('unknown')
    expect(missing.scope).toBe('unavailable')
    expect(missing.scopeLabel).toBe('node-2')
    expect(missing.countries).toEqual([])
  })

  it('never presents a single Node as Partial when the scope is not a Node scope', () => {
    const overview = nodeGeoOverview(node({
      geo: {
        state: 'current', scope: 'partial', countries: [se],
        knownCountryCount: 3, unknownCountryCount: 1, availablePeerCount: 4,
      },
    }))
    // One Node is complete or never-observed; Partial is Home's Network
    // vocabulary, so an untrusted scope reads Unknown rather than Partial.
    expect(overview.scope).toBe('unavailable')
    expect(overview.state).toBe('unknown')
  })

  it('reads Peer freshness as its own dimension and rejects a made-up value', () => {
    expect(nodeGeoOverview(node({ peers: { state: 'ok', freshness: 'stale' } })).peerObservation).toBe('stale')
    expect(nodeGeoOverview(node({ peers: { state: 'unknown', freshness: 'unknown' } })).peerObservation).toBe('unknown')
    expect(nodeGeoOverview(node({ peers: { state: 'ok', freshness: 'weird' } })).peerObservation).toBe('unknown')
  })
})

type ListedNodeOverrides = {
  nodeId?: string
  displayName?: string
  peers?: Record<string, unknown>
  peerCountries?: Record<string, unknown> | null
}

/** A listed Node of the public Networks response (#233): the compact buckets
 * the Home selection aggregate sums, never a full Geo Insight. */
function listedNode(overrides: ListedNodeOverrides = {}): PublicNode {
  return {
    nodeId: overrides.nodeId ?? 'node-1',
    displayName: overrides.displayName ?? 'Validator A',
    peers: { state: 'ok', freshness: 'current', ...overrides.peers },
    peerCountries: overrides.peerCountries === null
      ? undefined
      : {
        scope: 'complete',
        countries: [se],
        knownCountryCount: 3,
        unknownCountryCount: 1,
        unknownWithoutRemoteIpCount: 1,
        unknownWithPublicIpCount: 0,
        availablePeerCount: 4,
        ...overrides.peerCountries,
      },
  } as unknown as PublicNode
}

describe('homeNodeSelectionGeoOverview', () => {
  it('sums the matched Nodes\u2019 buckets into the Home Geo shape', () => {
    const overview = homeNodeSelectionGeoOverview([
      listedNode({
        nodeId: 'a',
        peerCountries: {
          scope: 'complete', countries: [se], knownCountryCount: 3, unknownCountryCount: 1,
          unknownWithoutRemoteIpCount: 1, unknownWithPublicIpCount: 0, availablePeerCount: 4,
        },
      }),
      listedNode({
        nodeId: 'b',
        peerCountries: {
          scope: 'complete', countries: [de, se], knownCountryCount: 5, unknownCountryCount: 2,
          unknownWithoutRemoteIpCount: 2, unknownWithPublicIpCount: 0, availablePeerCount: 7,
        },
      }),
    ], [
      network({
        networkKey: 'mainnet',
        displayName: 'Mainnet',
        geo: { countries: [se, de], knownCountryCount: 8, unknownCountryCount: 3, availablePeerCount: 11, attribution: 'GeoLite Data created by MaxMind' },
      }),
    ], 'the current list selection in Mainnet', 'mainnet')

    expect(overview.scopeKey).toBe('mainnet')
    expect(overview.scopeLabel).toBe('the current list selection in Mainnet')
    // The matched Nodes are the units in scope, not the Networks they sit in.
    expect(overview.unitsInScope).toBe(2)
    expect(overview.unitsWithBasis).toBe(2)
    expect(overview.countries).toEqual([
      { code: 'SE', count: 6, staleCount: 0, point: { lat: 60.1282, lon: 18.6435 } },
      { code: 'DE', count: 2, staleCount: 1, point: { lat: 51.1657, lon: 10.4515 } },
    ])
    expect(overview.knownCountryCount).toBe(8)
    expect(overview.unknownCountryCount).toBe(3)
    expect(overview.unknownWithoutRemoteIpCount).toBe(3)
    expect(overview.unknownWithPublicIpCount).toBe(0)
    expect(overview.availablePeerCount).toBe(11)
    // Provider state, attribution and the age fields stay the Network's (#233).
    expect(overview.state).toBe('current')
    expect(overview.scope).toBe('complete')
    expect(overview.attribution).toBe('GeoLite Data created by MaxMind')
  })

  it('reads exactly as one Node\u2019s own Detail page when only that Node is selected', () => {
    const buckets = {
      scope: 'complete', countries: [de, se], knownCountryCount: 5, unknownCountryCount: 2,
      unknownWithoutRemoteIpCount: 2, unknownWithPublicIpCount: 0, availablePeerCount: 7,
      attribution: 'GeoLite Data created by MaxMind',
    }
    const selection = homeNodeSelectionGeoOverview(
      [listedNode({ peerCountries: buckets })],
      [network({ networkKey: 'mainnet', displayName: 'Mainnet', geo: { ...buckets, state: 'stale' } })],
      'the current list selection in Mainnet',
      'mainnet',
    )
    const detail = nodeGeoOverview(node({ geo: { ...buckets, state: 'stale' } }))

    expect(selection.unitsWithBasis).toBe(detail.unitsWithBasis)
    expect(selection.state).toBe(detail.state)
    expect(selection.scope).toBe(detail.scope)
    expect(selection.countries).toEqual(detail.countries)
    expect(selection.knownCountryCount).toBe(detail.knownCountryCount)
    expect(selection.unknownCountryCount).toBe(detail.unknownCountryCount)
    expect(selection.unknownWithoutRemoteIpCount).toBe(detail.unknownWithoutRemoteIpCount)
    expect(selection.unknownWithPublicIpCount).toBe(detail.unknownWithPublicIpCount)
    expect(selection.availablePeerCount).toBe(detail.availablePeerCount)
    expect(selection.peerObservation).toBe(detail.peerObservation)
    expect(selection.attribution).toBe(detail.attribution)
    // Only the naming differs: a Home selection keeps the Network scope it is in.
    expect(selection.scopeKey).toBe('mainnet')
    expect(detail.scopeKey).toBeNull()
  })

  it('keeps a never-observed Node out of the counts without dropping it from the scope', () => {
    const unobservedBuckets = { scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null }
    const overview = homeNodeSelectionGeoOverview([
      listedNode({ nodeId: 'a', peerCountries: unobservedBuckets }),
      listedNode({ nodeId: 'b', peerCountries: { scope: 'complete', countries: [se], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } }),
    ], [network({ networkKey: 'mainnet', geo: { countries: [se], knownCountryCount: 3, unknownCountryCount: 0, availablePeerCount: 3 } })], 'the current list selection', 'all')

    // Partial, exactly as the Server reads the same selection: one Node has no
    // basis, so the reading is not Complete over the Nodes that do.
    expect(overview.scope).toBe('partial')
    expect(overview.unitsInScope).toBe(2)
    expect(overview.unitsWithBasis).toBe(1)
    expect(overview.knownCountryCount).toBe(3)
    expect(overview.unknownCountryCount).toBe(0)
    expect(overview.countries).toEqual([{ code: 'SE', count: 3, staleCount: 0, point: { lat: 60.1282, lon: 18.6435 } }])

    const never = homeNodeSelectionGeoOverview([listedNode({ peerCountries: unobservedBuckets })],
      [network({ networkKey: 'mainnet', geo: { scope: 'unobserved', countries: null, knownCountryCount: null, unknownCountryCount: null, availablePeerCount: null } })],
      'the current list selection', 'all')
    expect(never.scope).toBe('unobserved')
    expect(never.unitsWithBasis).toBe(0)
    expect(never.knownCountryCount).toBeNull()
    expect(never.availablePeerCount).toBeNull()
    expect(never.countries).toEqual([])
  })

  it('matches nothing into an Empty map slot instead of a fabricated zero', () => {
    const overview = homeNodeSelectionGeoOverview([], [network({ networkKey: 'mainnet' })], 'the current list selection', 'mainnet')

    expect(overview.unitsInScope).toBe(0)
    expect(overview.unitsWithBasis).toBe(0)
    expect(overview.scope).toBe('unavailable')
    // Provider state stays Network-level: a filter cannot make a provider read
    // fresh, so the empty slot comes from having no unit in scope and not from
    // a Geo state the client invented.
    expect(overview.state).toBe('current')
    expect(overview.countries).toEqual([])
    expect(overview.knownCountryCount).toBeNull()
    expect(overview.availablePeerCount).toBeNull()
    expect(geoMapStatus(overview, { loading: false, hasProjection: true })).toBe('empty')
    expect(geoMapStatus(overview, { loading: true, hasProjection: true })).toBe('starting')
  })

  it('reads a listed Node without buckets as Unavailable, never as a zero', () => {
    const overview = homeNodeSelectionGeoOverview([listedNode({ peerCountries: null })], [network({ networkKey: 'mainnet' })], 'the current list selection', 'all')

    expect(overview.scope).toBe('unavailable')
    expect(overview.unitsWithBasis).toBe(0)
    expect(overview.knownCountryCount).toBeNull()
    expect(overview.countries).toEqual([])
  })

  it('reports the selection\u2019s Peer freshness as Stale or Unknown, never Mixed', () => {
    const label = 'the current list selection'
    expect(homeNodeSelectionGeoOverview([listedNode(), listedNode({ nodeId: 'b' })], [], label, 'all').peerObservation).toBe('current')
    expect(homeNodeSelectionGeoOverview([listedNode(), listedNode({ nodeId: 'b', peers: { state: 'ok', freshness: 'stale' } })], [], label, 'all').peerObservation).toBe('stale')
    expect(homeNodeSelectionGeoOverview([listedNode({ peers: { state: 'unknown', freshness: 'unknown' } })], [], label, 'all').peerObservation).toBe('unknown')
    expect(homeNodeSelectionGeoOverview([listedNode(), listedNode({ nodeId: 'b', peers: { state: 'ok', freshness: null } })], [], label, 'all').peerObservation).toBe('unknown')
    expect(homeNodeSelectionGeoOverview([], [], label, 'all').peerObservation).toBe('unknown')
  })
})

describe('geoMapStatus', () => {
  it('passes the Server state through and adds only the client slots', () => {
    const empty = homeGeoOverview([], 'all')
    expect(geoMapStatus(empty, { loading: true, hasProjection: true })).toBe('starting')
    expect(geoMapStatus(empty, { loading: false, hasProjection: false })).toBe('unavailable')
    expect(geoMapStatus(empty, { loading: false, hasProjection: true })).toBe('empty')

    const current = homeGeoOverview([network({ networkKey: 'mainnet' })], 'all')
    expect(geoMapStatus(current, { loading: false, hasProjection: true })).toBe('current')
    const disabled = homeGeoOverview([network({ networkKey: 'mainnet', geo: { state: 'disabled' } })], 'all')
    expect(geoMapStatus(disabled, { loading: false, hasProjection: true })).toBe('disabled')
  })
})
