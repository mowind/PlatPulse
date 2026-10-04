import { describe, expect, it } from 'vitest'
import {
  formatStorageCadence,
  formatStorageShare,
  formatStorageSilenceThreshold,
  storageMountObservation,
  storageMountState,
  storageMountStateLabel,
  storageMountStateTone,
  storageMountSummary,
  storageMountsOrder,
  storageMountsView,
  type StorageMountState,
} from './storageMounts'
import type {
  AdminAgentStorageMountsResponse,
  AdminStorageMount,
  AdminStorageSeries,
} from './api/generated'

function series(overrides: Partial<AdminStorageSeries> = {}): AdminStorageSeries {
  return {
    metric: 'disk_used_bytes',
    observed: true,
    firstObservedAt: '2026-08-12T00:00:00Z',
    lastObservedAt: '2026-08-12T08:00:00Z',
    lastReceivedAt: '2026-08-12T08:00:01Z',
    observationCount: 12,
    replayedCount: 0,
    correctedCount: 0,
    releasedBefore: null,
    latestValue: 512,
    latestObservedAt: '2026-08-12T08:00:00Z',
    latestReceivedAt: '2026-08-12T08:00:01Z',
    latestDelaySeconds: 1,
    latestClockSuspect: false,
    ...overrides,
  }
}

function mount(overrides: Partial<AdminStorageMount> = {}): AdminStorageMount {
  return {
    mountPath: '/data',
    observationState: 'reported',
    silentSeconds: 60,
    used: series(),
    capacity: series({ metric: 'disk_total_bytes', latestValue: 1024 }),
    ...overrides,
  }
}

function answer(
  overrides: Partial<AdminAgentStorageMountsResponse> = {},
): AdminAgentStorageMountsResponse {
  return {
    agentId: '0195f2a1-0011-4011-8011-000000000011',
    answeredAt: '2026-08-12T08:00:05Z',
    cadenceSeconds: 900,
    silenceThresholdSeconds: 900,
    usedMetric: 'disk_used_bytes',
    capacityMetric: 'disk_total_bytes',
    mountLimit: 256,
    truncated: false,
    collectionPaused: false,
    mounts: [mount()],
    ...overrides,
  }
}

describe('storage mount state', () => {
  it('answers the Server state in the fixed vocabulary and reads anything else as unknown', () => {
    expect(storageMountState('reported')).toBe('reported')
    expect(storageMountState('silent')).toBe('silent')
    // A state this card does not know is never shown as a path still reporting.
    expect(storageMountState('unknown')).toBe('unknown')
    expect(storageMountState('stale')).toBe('unknown')
    expect(storageMountState('')).toBe('unknown')
  })

  it('labels the states with the vocabulary StatusBadge owns', () => {
    const states: StorageMountState[] = ['reported', 'silent', 'unknown']
    expect(states.map((state) => storageMountStateLabel(state))).toEqual([
      'Current',
      'Stale',
      'Unknown',
    ])
    expect(states.map((state) => storageMountStateTone(state))).toEqual([
      'ok',
      'warning',
      'neutral',
    ])
  })
})

describe('storage mount cadence', () => {
  it('states a measured cadence and refuses a zero cadence as a rhythm', () => {
    expect(formatStorageCadence(900)).toBe('The Agent observes this Host every 15 minutes.')
    // Zero is not a cadence of zero: it says too little evidence was stored for
    // the Server to measure one, and no silence may be declared from it.
    const unmeasured = formatStorageCadence(0)
    expect(unmeasured).toContain('could not measure a cadence')
    expect(unmeasured).toContain('no silence can be declared')
    expect(formatStorageCadence(0)).not.toContain('every')
  })

  it('compares silence only against a measured cadence', () => {
    expect(formatStorageSilenceThreshold(900, 900)).toBe(
      'A path is called silent once it has said nothing for 15 minutes: three cadences of at most five minutes, and never less than two minutes.',
    )
    // Without a cadence there is nothing to be silent against.
    expect(formatStorageSilenceThreshold(0, 0)).toContain(
      'Silence needs a measured cadence to compare against',
    )
    expect(formatStorageSilenceThreshold(0, 0)).toContain('answered as unknown rather than as stopped')
  })
})

describe('storage mount observation', () => {
  it('says a silent path is silent for longer than a silence is judged against, and that the readings stay', () => {
    const text = storageMountObservation(
      { observationState: 'silent', silentSeconds: 1200 },
      900,
      900,
      false,
    )
    expect(text).toBe(
      'Silent for 20 minutes, longer than the 15 minutes a silence is judged against on this Host. The readings already stored stay on the series.',
    )
  })

  it('reports a current path with the cadence it is still observed at', () => {
    expect(storageMountObservation({ observationState: 'reported', silentSeconds: 30 }, 900, 900, false)).toBe(
      'Still observed at the cadence of 15 minutes: Within the last minute.',
    )
    expect(storageMountObservation({ observationState: 'reported', silentSeconds: null }, 900, 900, false)).toBe(
      'Still observed at the cadence of 15 minutes: Unknown age.',
    )
  })

  it('answers an unmeasured cadence as unknown and never as a verdict of silencing', () => {
    const text = storageMountObservation(
      { observationState: 'unknown', silentSeconds: 1200 },
      0,
      0,
      false,
    )
    expect(text).toBe(
      'No cadence could be measured from the stored evidence, so this path is neither reported nor silent: its newest observation is 20 minutes ago, which is an age and not a verdict.',
    )
    expect(text).not.toContain('Silent for')
    // A null age means the Server stated no age, not that no instant exists: the
    // ledger holds the instant, and there is simply no cadence to judge it with.
    expect(
      storageMountObservation({ observationState: 'unknown', silentSeconds: null }, 0, 0, false),
    ).toContain(
      'the Server states no age for it, because an age with no cadence behind it is not a verdict',
    )
  })

  it('blames the pause the Server is holding, and never the Agent, while readings are held back', () => {
    const paused = storageMountObservation(
      { observationState: 'unknown', silentSeconds: null },
      900,
      900,
      true,
    )
    expect(paused).toContain('holding optional history back')
    expect(paused).toContain('not called stale for a pause it did not cause')
    expect(paused).not.toContain('No cadence could be measured')
    // A cadence nobody could measure stays the reason when there is none, pause
    // or no pause: the pause only explains an unknown the pause itself caused.
    expect(
      storageMountObservation({ observationState: 'unknown', silentSeconds: null }, 0, 0, true),
    ).toContain('No cadence could be measured')

    const view = storageMountsView(
      answer({
        collectionPaused: true,
        mounts: [mount({ observationState: 'unknown', silentSeconds: null })],
      }),
    )
    expect(view.collectionPaused).toBe(true)
    expect(view.pause).toContain('Low-space protection is holding optional history back')
    expect(view.pause).toContain('no path is judged silent')
    expect(view.mounts[0].observation).toContain('holding optional history back')
    expect(storageMountsView(answer()).pause).toBeNull()
  })
})

describe('storage series coverage', () => {
  it('never renders an unobserved series as a zero reading', () => {
    const summary = storageMountSummary(
      mount({
        observed: true,
        used: series({ observed: false, latestValue: null, observationCount: 0, firstObservedAt: null, lastObservedAt: null, latestObservedAt: null }),
      } as Partial<AdminStorageMount>),
      900,
      900,
      false,
    ).used
    expect(summary.coverage).toBe('Never observed')
    expect(summary.latest).toBe('No reading was ever observed for this series.')
    expect(summary.firstObservedAt).toBeNull()
  })

  it('tells a retained-but-released series apart from a series nothing was observed for', () => {
    const released = storageMountSummary(
      mount({
        used: series({ latestValue: null, releasedBefore: '2026-08-12T06:00:00Z' }),
      }),
      900,
      900,
      false,
    ).used
    expect(released.coverage).toBe('12 observations recorded')
    expect(released.latest).toBe(
      'The newest reading is no longer stored: retention released this series before 2026-08-12T06:00:00Z.',
    )
    expect(released.latest).not.toContain('was ever observed')
    // The instant the ledger holds outlives the readings retention removed.
    expect(released.lastObservedAt).toBe('2026-08-12T08:00:00Z')
    expect(released.releasedBefore).toBe('2026-08-12T06:00:00Z')
  })

  it('names replays and corrections only when the ledger recorded them, and flags a suspect clock', () => {
    const quiet = storageMountSummary(mount(), 900, 900, false).used
    expect(quiet.evidence).toBeNull()
    const loud = storageMountSummary(
      mount({
        used: series({ replayedCount: 2, correctedCount: 1, latestClockSuspect: true }),
      }),
      900,
      900,
      false,
    ).used
    expect(loud.evidence).toBe(
      'Of the observations it holds, the ledger records 2 replayed and 1 corrected.',
    )
    expect(loud.latest).toContain('stamped ahead of its receipt')
  })

  it('reads one observation in the singular', () => {
    expect(storageMountSummary(mount({ used: series({ observationCount: 1 }) }), 900, 900, false).used.coverage).toBe(
      '1 observation recorded',
    )
  })
})

describe('storage mount share', () => {
  it('names each side of the share and refuses a share it cannot compute', () => {
    expect(formatStorageShare(512, 1024)).toBe('512 B of 1.00 KiB · 50%')
    expect(formatStorageShare(null, 1024)).toBeNull()
    expect(formatStorageShare(512, null)).toBeNull()
    expect(formatStorageShare(512, 0)).toBeNull()
  })

  it('leaves the share unknown when either series has no stored reading', () => {
    const summary = storageMountSummary(mount({ capacity: series({ metric: 'disk_total_bytes', latestValue: null, observed: false }) }), 900, 900, false)
    expect(summary.share).toBeNull()
  })
})

describe('storage mounts answer', () => {
  it('keeps the Server order and states the limit rather than dropping paths silently', () => {
    const view = storageMountsView(
      answer({ truncated: true, mounts: [mount({ mountPath: '/bulk-299' }), mount({ mountPath: '/data' })] }),
    )
    expect(view.mounts.map((entry) => entry.mountPath)).toEqual(['/bulk-299', '/data'])
    expect(view.order).toBe(storageMountsOrder())
    expect(view.coverageLimit).toBe(
      'This list carries at most 256 mount paths: the newest paths are answered and older ones are left out.',
    )
    expect(view.truncation).toContain('holds more mount paths than the list carries')
    expect(view.truncation).toContain('oldest of them are not in this answer')
  })

  it('states no truncation when every path fits', () => {
    const view = storageMountsView(answer())
    expect(view.truncated).toBe(false)
    expect(view.truncation).toBeNull()
  })

  it('carries the cadence and the mount limit into every row it renders', () => {
    const view = storageMountsView(
      answer({ cadenceSeconds: 900, silenceThresholdSeconds: 900, mounts: [mount({ silentSeconds: 1200, observationState: 'silent' })] }),
    )
    expect(view.cadence).toBe('The Agent observes this Host every 15 minutes.')
    expect(view.mountLimit).toBe(256)
    expect(view.mounts[0].tone).toBe('warning')
    expect(view.mounts[0].stateLabel).toBe('Stale')
    expect(view.mounts[0].observation).toContain('longer than the 15 minutes')
    expect(view.mounts[0].share).toBe('512 B of 1.00 KiB · 50%')
  })
})
