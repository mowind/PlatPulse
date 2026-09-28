import { describe, expect, it } from 'vitest'
import type { PublicNetwork, PublicNodeDetail } from './api/generated'
import { geoMapStatus, homeGeoOverview, nodeGeoOverview } from './homeGeo'

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
    expect(overview.networksInScope).toBe(1)
    expect(overview.networksWithBasis).toBe(1)
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
    expect(overview.networksInScope).toBe(2)
    expect(overview.networksWithBasis).toBe(2)
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

    expect(overview.networksInScope).toBe(2)
    expect(overview.networksWithBasis).toBe(1)
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
    expect(empty.networksInScope).toBe(0)

    const unmatched = homeGeoOverview([network({ networkKey: 'mainnet' })], 'missing')
    expect(unmatched.scopeLabel).toBe('missing')
    expect(unmatched.networksInScope).toBe(0)
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
    expect(overview.networksInScope).toBe(1)
    expect(overview.networksWithBasis).toBe(1)
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
    expect(overview.networksWithBasis).toBe(0)
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
