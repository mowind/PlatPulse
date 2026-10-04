import { describe, expect, it } from 'vitest'
import {
  formatStateSyncing,
  formatStateValueAge,
  stateEvidence,
  stateEvidenceLabel,
  stateEvidenceTone,
  stateHistoryAnchorNotice,
  stateHistoryCadenceNotice,
  stateHistoryChartGeometry,
  stateHistoryCoverageNotice,
  stateHistoryEntrySummary,
  stateHistoryGapSummary,
  stateHistoryNewestNotice,
  stateHistoryOrder,
  stateHistoryPagingNotice,
  stateHistoryPauseNotice,
  stateHistoryTruncationNotice,
  stateHistoryUnaskedNotice,
  stateHistoryView,
  stateValueAgeSeconds,
} from './stateHistory'
import type {
  AdminStateEntry,
  AdminStateGap,
  AdminStateHistoryResponse,
  AdminStateSeries,
} from './api/generated'

function entry(overrides: Partial<AdminStateEntry> = {}): AdminStateEntry {
  return {
    observedAt: '2026-08-12T09:00:00Z',
    receivedAt: '2026-08-12T09:00:01Z',
    entryKind: 'change',
    collectionState: 'ok',
    valueSource: 'current',
    valueObservedAt: '2026-08-12T09:00:00Z',
    errorCode: null,
    syncing: false,
    delaySeconds: 1,
    clockSuspect: false,
    clockNote: null,
    ...overrides,
  }
}

function gap(overrides: Partial<AdminStateGap> = {}): AdminStateGap {
  return {
    from: '2026-08-12T09:00:30Z',
    to: '2026-08-12T11:00:00Z',
    seconds: 7170,
    kind: 'collection_gap',
    reason: 'no_report',
    skippedCount: null,
    ...overrides,
  }
}

function series(overrides: Partial<AdminStateSeries> = {}): AdminStateSeries {
  return {
    observed: true,
    entryCount: 3,
    changeCount: 2,
    anchorCount: 1,
    replayedCount: 0,
    correctedCount: 0,
    entriesReturned: 3,
    firstObservedAt: '2026-08-12T09:00:00Z',
    lastObservedAt: '2026-08-12T11:00:30Z',
    lastReceivedAt: '2026-08-12T11:00:31Z',
    releasedBefore: null,
    latestCollectionState: 'ok',
    latestValueSource: 'current',
    latestValueObservedAt: '2026-08-12T11:00:30Z',
    latestErrorCode: null,
    latestSyncing: true,
    coverageSeconds: 30,
    windowSeconds: 86400,
    ...overrides,
  }
}

function answer(overrides: Partial<AdminStateHistoryResponse> = {}): AdminStateHistoryResponse {
  return {
    nodeId: '0195f2a1-0014-4014-8014-000000000014',
    component: 'sync',
    from: '2026-08-11T12:00:00Z',
    to: '2026-08-12T12:00:00Z',
    requestedFrom: '2026-08-11T12:00:00Z',
    availability: null,
    retentionDays: 30,
    anchorSeconds: 3600,
    cadenceSeconds: 300,
    coverageSeconds: 30,
    windowSeconds: 86400,
    entries: [entry()],
    gaps: [],
    series: series(),
    truncated: false,
    continuation: null,
    ...overrides,
  }
}

/** The answer of a Node that never had this component recorded at all. */
function unasked(): AdminStateHistoryResponse {
  return answer({
    entries: [],
    gaps: [],
    cadenceSeconds: 0,
    coverageSeconds: 0,
    series: series({
      observed: false,
      entryCount: 0,
      changeCount: 0,
      anchorCount: 0,
      entriesReturned: 0,
      firstObservedAt: null,
      lastObservedAt: null,
      lastReceivedAt: null,
      latestCollectionState: null,
      latestValueSource: null,
      latestValueObservedAt: null,
      latestSyncing: null,
      coverageSeconds: 0,
    }),
  })
}

describe('recorded state evidence', () => {
  it('reads a successful collection with a current value as current', () => {
    expect(stateEvidence('ok', 'current')).toBe('current')
    expect(stateEvidenceLabel('current')).toBe('Current')
    expect(stateEvidenceTone('current')).toBe('ok')
  })

  it('reads a failed collection that kept a value as last good, never as current', () => {
    expect(stateEvidence('error', 'last_good')).toBe('retained')
    expect(stateEvidenceLabel('retained')).toBe('Last good')
    expect(stateEvidenceTone('retained')).toBe('warning')
  })

  it('reads a disabled collection as a state of its own', () => {
    expect(stateEvidence('disabled', 'none')).toBe('disabled')
    expect(stateEvidenceLabel('disabled')).toBe('Not collected')
    expect(stateEvidenceTone('disabled')).toBe('neutral')
  })

  it('reads an unsupported collection as unsupported rather than as a value', () => {
    expect(stateEvidence('unsupported', 'none')).toBe('unsupported')
    expect(stateEvidenceLabel('unsupported')).toBe('Unsupported')
    expect(stateEvidenceTone('unsupported')).toBe('neutral')
  })

  it('never invents a current state for a pair it does not know', () => {
    expect(stateEvidence('ok', 'none')).toBe('unknown')
    expect(stateEvidence('collecting', 'current')).toBe('unknown')
    expect(stateEvidenceLabel('unknown')).toBe('Unknown')
    expect(stateEvidenceTone('unknown')).toBe('neutral')
  })
})

describe('recorded state syncing', () => {
  it('reads a flag the Server actually carried', () => {
    expect(formatStateSyncing(entry({ syncing: true }))).toBe('Syncing')
    expect(formatStateSyncing(entry({ syncing: false }))).toBe('Not syncing')
  })

  it('reads an absent flag as unknown rather than as false', () => {
    const summary = stateHistoryEntrySummary(entry({ syncing: null }))
    expect(summary.syncing).toBe('Unknown')
    expect(summary.syncingClaim).toContain('is unknown')
    expect(summary.syncingClaim).not.toContain('was not syncing')
    expect(stateHistoryEntrySummary(entry({ syncing: undefined })).syncing).toBe('Unknown')
  })

  it('never claims a failed probe read a flag it could not read', () => {
    const summary = stateHistoryEntrySummary(
      entry({ collectionState: 'error', valueSource: 'last_good', syncing: null }),
    )
    expect(summary.evidence).toBe('retained')
    expect(summary.syncing).toBe('Unknown')
    expect(summary.syncingClaim).toContain('no flag was read')
  })
})

describe('recorded state value age', () => {
  it('shows the age a retained value really has', () => {
    const retained = entry({
      collectionState: 'error',
      valueSource: 'last_good',
      observedAt: '2026-08-12T09:20:00Z',
      valueObservedAt: '2026-08-12T09:00:00Z',
    })
    expect(stateValueAgeSeconds(retained)).toBe(1200)
    expect(formatStateValueAge(retained)).toBe(
      'The value this state refers to was 20 minutes old when the state was recorded.',
    )
  })

  it('shows a value observed with its state as no age at all', () => {
    expect(
      formatStateValueAge(
        entry({ observedAt: '2026-08-12T09:00:00Z', valueObservedAt: '2026-08-12T09:00:00Z' }),
      ),
    ).toBe('The value was observed at the same instant as this state.')
  })

  it('never dresses a state with no value as a fresh reading', () => {
    const withoutValue = entry({ valueSource: 'none', valueObservedAt: null })
    const text = formatStateValueAge(withoutValue)
    expect(text).toBe('This state carries no value: the Server holds no observation to show for it.')
    expect(text).not.toContain('old when the state was recorded')
    expect(stateValueAgeSeconds(withoutValue)).toBeNull()
  })
})

describe('recorded state entry summary', () => {
  it('names an unchanged delivery as an anchor', () => {
    const summary = stateHistoryEntrySummary(entry({ entryKind: 'anchor' }))
    expect(summary.entryKindLabel).toBe('Anchor')
    expect(summary.anchor).toBe(true)
  })

  it('carries the delivery facts an Operator reads', () => {
    const summary = stateHistoryEntrySummary(entry({ delaySeconds: 1 }))
    expect(summary.collection).toBe('Collection succeeded')
    expect(summary.valueSourceLabel).toBe('Value observed now')
    expect(summary.delay).toBe('1 second')
    expect(summary.clockSuspect).toBe(false)
    expect(summary.errorCode).toBeNull()
    expect(summary.entryKindLabel).toBe('Change')
    expect(summary.anchor).toBe(false)
  })

  it('keeps the failure code and the clock note of a suspect delivery', () => {
    const summary = stateHistoryEntrySummary(
      entry({
        collectionState: 'error',
        errorCode: 'rpc_unreachable',
        clockSuspect: true,
        clockNote: 'stamped after its receipt',
        delaySeconds: null,
      }),
    )
    expect(summary.errorCode).toBe('rpc_unreachable')
    expect(summary.clockSuspect).toBe(true)
    expect(summary.clockNote).toBe('stamped after its receipt')
    expect(summary.delay).toBe('Unknown')
    expect(summary.evidenceLabel).toBe('Failed')
  })
})

describe('recorded state gaps', () => {
  it('reads a stretch nobody observed as a collection gap', () => {
    const row = stateHistoryGapSummary(gap({ seconds: 120 }))
    expect(row.kindLabel).toBe('Collection gap')
    expect(row.pause).toBe(false)
    expect(row.duration).toBe('2 minutes')
    expect(row.claim).toContain('Nobody observed this Node across this stretch')
    expect(row.claim).not.toContain('skipped behind it')
  })

  it('reads a paused stretch as a loss with its counted skips', () => {
    const row = stateHistoryGapSummary(
      gap({ kind: 'protection_pause', reason: 'capacity_floor', skippedCount: 7 }),
    )
    expect(row.kindLabel).toBe('Protection pause')
    expect(row.pause).toBe(true)
    expect(row.skippedCount).toBe(7)
    expect(row.claim).toContain('The operator paused history collection across this stretch')
    expect(row.claim).toContain('a paused stretch is neither a state nor a change')
    expect(row.claim).toContain('7 observations were skipped behind it.')
  })

  it('counts one skipped observation in the singular', () => {
    const row = stateHistoryGapSummary(gap({ kind: 'protection_pause', skippedCount: 1 }))
    expect(row.claim).toContain('1 observation was skipped behind it.')
  })
})

describe('state history pauses', () => {
  it('reports no pause when the window holds none', () => {
    expect(stateHistoryPauseNotice([])).toBeNull()
    expect(stateHistoryView(answer({ gaps: [gap()] })).pause).toBeNull()
  })

  it('adds the skipped observations up and reads the pause as a loss', () => {
    const notice = stateHistoryPauseNotice([
      stateHistoryGapSummary(gap({ kind: 'protection_pause', skippedCount: 3 })),
      stateHistoryGapSummary(
        gap({ kind: 'protection_pause', from: '2026-08-12T11:00:00Z', to: '2026-08-12T11:30:00Z', skippedCount: 4 }),
      ),
    ])
    expect(notice).toContain('2 stretches of paused history collection')
    expect(notice).toContain('7 skipped observation(s) behind them')
    expect(notice).toContain('a paused stretch is not a state of the Node and not a change of one')
  })
})

describe('recorded state notices', () => {
  it('says why an unchanged state is still provable', () => {
    const anchor = stateHistoryAnchorNotice(3600)
    expect(anchor).toContain('An unchanged state is recorded again at most once every 1 hour')
    expect(anchor).toContain('keeps a constant state provable')
    expect(stateHistoryAnchorNotice(0)).toContain('does not record anchors')
  })

  it('measures silences against the cadence the Node really reported at', () => {
    expect(stateHistoryCadenceNotice(300)).toContain('This Node reported every 5 minutes')
    expect(stateHistoryCadenceNotice(0)).toContain('reporting cadence of this Node is not known')
  })

  it('never claims the stretch after the newest state as covered', () => {
    const none = stateHistoryCoverageNotice(0, 86400)
    expect(none).toContain('none of it is covered')
    expect(none).toContain('nothing after the newest state is claimed')
    expect(none).not.toContain('The record proves')
    expect(stateHistoryCoverageNotice(3600, 86400)).toContain('The record proves 1 hour of 24 hours')
  })

  it('reads a component with no record as absent rather than as a state', () => {
    const absent = stateHistoryUnaskedNotice('sync')
    expect(absent).toContain('no recorded Sync state in any retained window')
    expect(absent).toContain('not current, not healthy and not zero')
  })

  it('ages the newest state instead of calling the rest of the window silent', () => {
    const newest = stateHistoryNewestNotice(
      series({ lastObservedAt: '2026-08-12T11:00:00Z' }),
      '2026-08-12T12:00:00Z',
    )
    expect(newest).toContain('1 hour older than the end of this window')
    expect(newest).toContain('2026-08-12T11:00:00Z')
    expect(newest).toContain('silence after the newest state is not reported as a gap')
    expect(stateHistoryNewestNotice(series({ lastObservedAt: null }), '2026-08-12T12:00:00Z')).toBeNull()
  })

  it('reports a truncated window instead of dropping the older entries silently', () => {
    const truncation = stateHistoryTruncationNotice('sync', 2)
    expect(truncation).toContain('more recorded sync state than this answer carries')
    expect(truncation).toContain('the newest 2 recorded states are shown')
    expect(truncation).toContain('reported here rather than dropped silently')
  })

  it('reads an older page as an answer that starts later than it was asked', () => {
    const paging = stateHistoryPagingNotice('2026-08-11T18:00:00Z', '2026-08-11T12:00:00Z')
    expect(paging).toContain('starts at 2026-08-11T18:00:00Z rather than at the 2026-08-11T12:00:00Z')
    expect(stateHistoryPagingNotice('2026-08-11T12:00:00Z', '2026-08-11T12:00:00Z')).toBeNull()
  })

  it('keeps the order the Server answered the record in', () => {
    expect(stateHistoryOrder()).toBe('Oldest recorded state first, in the order the Server answered them.')
  })
})

describe('recorded state timeline geometry', () => {
  it('places one marker per recorded state across the window', () => {
    const chart = stateHistoryChartGeometry(
      [entry({ observedAt: '2026-08-11T12:00:00Z' }), entry({ observedAt: '2026-08-12T00:00:00Z', entryKind: 'anchor' })],
      [],
      '2026-08-11T12:00:00Z',
      '2026-08-12T12:00:00Z',
    )
    expect(chart.width).toBe(600)
    expect(chart.markers).toHaveLength(2)
    expect(chart.markers[0].x).toBe(0)
    expect(chart.markers[1].x).toBe(300)
    expect(chart.markers[1].anchor).toBe(true)
    expect(chart.markers[1].label).toBe('Anchor')
    expect(chart.gapBands).toHaveLength(0)
  })

  it('draws a band for the stretch two states prove and none across a reported gap', () => {
    const entries = [entry({ observedAt: '2026-08-11T12:00:00Z' }), entry({ observedAt: '2026-08-12T00:00:00Z' })]
    const proven = stateHistoryChartGeometry(
      entries,
      [],
      '2026-08-11T12:00:00Z',
      '2026-08-12T12:00:00Z',
    )
    expect(proven.provenBands).toHaveLength(1)
    expect(proven.provenBands[0].x).toBe(0)
    expect(proven.provenBands[0].width).toBe(300)
    const silenced = stateHistoryChartGeometry(
      entries,
      [gap({ from: '2026-08-11T18:00:00Z', to: '2026-08-12T00:00:00Z' })],
      '2026-08-11T12:00:00Z',
      '2026-08-12T12:00:00Z',
    )
    expect(silenced.gapBands).toHaveLength(1)
    expect(silenced.gapBands[0].x).toBe(150)
    expect(silenced.gapBands[0].width).toBe(150)
    expect(silenced.provenBands).toHaveLength(0)
  })

  it('clamps a silence that reaches past the answer instead of drawing outside it', () => {
    const chart = stateHistoryChartGeometry(
      [entry()],
      [gap({ from: '2026-08-10T00:00:00Z', to: '2026-08-13T00:00:00Z' })],
      '2026-08-11T12:00:00Z',
      '2026-08-12T12:00:00Z',
    )
    expect(chart.gapBands[0].x).toBe(0)
    expect(chart.gapBands[0].width).toBe(600)
  })
})

describe('recorded states answer', () => {
  it('reads the ledger of an answered window', () => {
    const view = stateHistoryView(answer())
    expect(view.componentLabel).toBe('Sync')
    expect(view.observed).toBe(true)
    expect(view.entryCount).toBe(3)
    expect(view.changeCount).toBe(2)
    expect(view.anchorCount).toBe(1)
    expect(view.firstObservedAt).toBe('2026-08-12T09:00:00Z')
    expect(view.anchorSeconds).toBe(3600)
    expect(view.anchor).toContain('keeps a constant state provable')
    expect(view.cadence).toContain('This Node reported every 5 minutes')
    expect(view.entries).toHaveLength(1)
    expect(view.entries[0].evidenceLabel).toBe('Current')
    expect(view.order).toContain('Oldest recorded state first')
    expect(view.pause).toBeNull()
    expect(view.truncation).toBeNull()
    expect(view.availabilityNotice).toBeNull()
    expect(view.unasked).toBeNull()
  })

  it('reads an unasked component as absent rather than as a healthy state', () => {
    const view = stateHistoryView(unasked())
    expect(view.observed).toBe(false)
    expect(view.entries).toHaveLength(0)
    expect(view.entryCount).toBe(0)
    expect(view.latest).toBeNull()
    expect(view.chart.markers).toHaveLength(0)
    expect(view.unasked).toContain('not current, not healthy and not zero')
  })

  it('discloses a truncated window and keeps the cursor for the older page', () => {
    const view = stateHistoryView(
      answer({
        entries: [entry(), entry({ observedAt: '2026-08-12T10:00:00Z' })],
        truncated: true,
        continuation: '2026-08-12T06:00:00Z',
        from: '2026-08-12T00:00:00Z',
        requestedFrom: '2026-08-11T12:00:00Z',
      }),
    )
    expect(view.truncated).toBe(true)
    expect(view.truncation).toContain('the newest 2 recorded states are shown')
    expect(view.continuation).toBe('2026-08-12T06:00:00Z')
    expect(view.paging).toContain('it is the older page the cursor points at')
  })

  it('discloses a range the Server could only answer in part', () => {
    const view = stateHistoryView(answer({ availability: 'partial' }))
    expect(view.availability).toBe('partial')
    expect(view.availabilityNotice).toContain('starts before the investigation horizon')
  })

  it('discloses a range no tier keeps evidence for', () => {
    const view = stateHistoryView(
      answer({ availability: 'unavailable', entries: [], coverageSeconds: 0 }),
    )
    expect(view.availabilityNotice).toContain('entirely older than the investigation horizon')
    expect(view.coverage).toContain('none of it is covered')
  })
})
