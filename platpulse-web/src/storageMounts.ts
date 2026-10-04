// Storage mount investigation (issue #216, design §11.6).
//
// A Host storage series is named by the mount path the Agent reported, and the
// Server compares that path literally. Without the list of paths the stored
// evidence actually holds, an Operator has to guess the exact string and a path
// the Agent stopped reporting becomes unreachable exactly when it matters most.
// This module is presentation only: it turns the Server's answer into the state
// of one mount, the coverage of the two series behind it, and the statements the
// card owes the Operator. Two rules it never breaks: unknown is never rendered
// as zero, and a path is never claimed to be a device.

import type {
  AdminAgentStorageMountsResponse,
  AdminStorageMount,
  AdminStorageSeries,
} from './api/generated'
import { formatBytes, formatPercent } from './formatBytes'
import { formatHistoryDuration } from './metricHistory'

/** The states the Server answers with, in the card's own vocabulary. */
export type StorageMountState = 'reported' | 'silent' | 'unknown'
export type StorageMountTone = 'ok' | 'warning' | 'neutral'

/**
 * The Server's observation state, narrowed to the three words this card knows.
 * A state the WebUI does not recognize is read as unknown: it is never
 * presented as a path the Agent is still reporting.
 */
export function storageMountState(observationState: string): StorageMountState {
  switch (observationState) {
    case 'reported':
      return 'reported'
    case 'silent':
      return 'silent'
    default:
      return 'unknown'
  }
}

/** The one place a state's word and tone are written down, so a state added to
 * the union cannot reach the badge with only one of the two. */
const MOUNT_STATE_PRESENTATION: Record<
  StorageMountState,
  { label: string; tone: StorageMountTone }
> = {
  reported: { label: 'Current', tone: 'ok' },
  silent: { label: 'Stale', tone: 'warning' },
  unknown: { label: 'Unknown', tone: 'neutral' },
}

/** The state in the fixed WebUI vocabulary, whose glyphs StatusBadge owns. */
export function storageMountStateLabel(state: StorageMountState): string {
  return MOUNT_STATE_PRESENTATION[state].label
}

export function storageMountStateTone(state: StorageMountState): StorageMountTone {
  return MOUNT_STATE_PRESENTATION[state].tone
}

/**
 * What the state means for this path, with the instant evidence behind it:
 * a path is silent only against a cadence the Server actually measured, and a
 * Server that could not measure one answers unknown rather than stale. The two
 * ways an unknown is reached are stated separately, because they mean different
 * things: no cadence to judge against at all, or a pause the Server itself is
 * holding the readings under.
 */
export function storageMountObservation(
  mount: Pick<AdminStorageMount, 'observationState' | 'silentSeconds'>,
  cadenceSeconds: number,
  silenceThresholdSeconds: number,
  collectionPaused: boolean,
): string {
  const state = storageMountState(mount.observationState)
  const age =
    mount.silentSeconds == null
      ? 'Unknown age'
      : mount.silentSeconds < 60
        ? 'Within the last minute'
        : formatHistoryDuration(mount.silentSeconds) + ' ago'
  if (state === 'silent') {
    return (
      'Silent for ' +
      (mount.silentSeconds == null ? 'an unknown time' : formatHistoryDuration(mount.silentSeconds)) +
      ', longer than the ' +
      formatHistoryDuration(silenceThresholdSeconds) +
      ' a silence is judged against on this Host. The readings already stored stay on the series.'
    )
  }
  if (state === 'reported') {
    return 'Still observed at the cadence of ' + formatHistoryDuration(cadenceSeconds) + ': ' + age + '.'
  }
  if (collectionPaused && cadenceSeconds > 0) {
    return (
      'The Server is holding optional history back, so it is not judging silence for any path: ' +
      'this path is not called stale for a pause it did not cause, and the readings already stored stay on the series.'
    )
  }
  return (
    'No cadence could be measured from the stored evidence, so this path is neither reported nor silent: ' +
    (mount.silentSeconds == null
      ? 'the Server states no age for it, because an age with no cadence behind it is not a verdict.'
      : 'its newest observation is ' + age + ', which is an age and not a verdict.')
  )
}

/**
 * The cadence the Server measured over the Agent's Host observations. Zero is
 * not a cadence of zero: it says the evidence holds too few observations to
 * measure one, and the card states that instead of a rhythm nobody observed.
 */
export function formatStorageCadence(cadenceSeconds: number): string {
  if (cadenceSeconds > 0) {
    return 'The Agent observes this Host every ' + formatHistoryDuration(cadenceSeconds) + '.'
  }
  return 'The Server could not measure a cadence from the stored Host observations, so no silence can be declared.'
}

/**
 * The silence that means a path stopped being reported: three cadences of the
 * cadence this Host is judged to observe at, with that cadence capped at five
 * minutes, and nothing at all while the cadence is unknown.
 */
export function formatStorageSilenceThreshold(
  cadenceSeconds: number,
  silenceThresholdSeconds: number,
): string {
  if (cadenceSeconds > 0 && silenceThresholdSeconds > 0) {
    return (
      'A path is called silent once it has said nothing for ' +
      formatHistoryDuration(silenceThresholdSeconds) +
      ': three cadences of at most five minutes, and never less than two minutes.'
    )
  }
  return 'Silence needs a measured cadence to compare against; without one a path is answered as unknown rather than as stopped.'
}

/**
 * The newest used reading as a share of the newest capacity reading, or null
 * when either side is unknown. The two are separate series: the card shows each
 * side's own instant beside the value so a share is never read as one reading.
 */
export function formatStorageShare(
  usedBytes: number | null | undefined,
  capacityBytes: number | null | undefined,
): string | null {
  if (usedBytes == null || capacityBytes == null || !(capacityBytes > 0)) return null
  return formatBytes(usedBytes) + ' of ' + formatBytes(capacityBytes) + ' · ' + formatPercent(
    (usedBytes / capacityBytes) * 100,
  )
}

/** One series behind one mount path, in the words the card shows. */
export type StorageSeriesSummary = {
  metric: string
  /** True when this path was observed for this series at all. */
  observed: boolean
  /** How much evidence the ledger holds, independent of the read window. */
  coverage: string
  /** The newest stored reading, or why there is none. Unknown is never zero. */
  latest: string
  /** Where the series begins; null when nothing was ever observed. */
  firstObservedAt: string | null
  /** The newest instant the ledger holds, which outlives released readings. */
  lastObservedAt: string | null
  /** The boundary retention left behind, stated rather than hidden. */
  releasedBefore: string | null
  /** Replays and corrections, named only when the ledger recorded some. */
  evidence: string | null
}

function observationCount(count: number): string {
  const readings = Math.max(0, Math.round(count))
  if (readings === 1) return '1 observation recorded'
  return readings + ' observations recorded'
}

/**
 * What one series holds: how much evidence, and the newest reading. A series
 * whose readings were released by retention says so with the boundary it kept,
 * which is a different statement from a series nothing was ever observed for.
 */
export function storageSeriesSummary(series: AdminStorageSeries): StorageSeriesSummary {
  const counts = observationCount(series.observationCount)
  let coverage = counts
  if (!series.observed) {
    coverage = 'Never observed'
  }
  const parts: string[] = [counts]
  if (series.replayedCount > 0) {
    parts.push(series.replayedCount === 1 ? '1 replayed' : series.replayedCount + ' replayed')
  }
  if (series.correctedCount > 0) {
    parts.push(
      series.correctedCount === 1 ? '1 corrected' : series.correctedCount + ' corrected',
    )
  }
  let latest: string
  if (series.latestValue != null) {
    latest =
      formatBytes(series.latestValue) +
      (series.latestObservedAt ? ' at ' + series.latestObservedAt : '')
  } else if (!series.observed) {
    latest = 'No reading was ever observed for this series.'
  } else if (series.releasedBefore != null) {
    latest = 'The newest reading is no longer stored: retention released this series before ' + series.releasedBefore + '.'
  } else {
    latest = 'No reading is stored in the retained window.'
  }
  if (series.latestClockSuspect) {
    latest += ' The newest reading is stamped ahead of its receipt: the Agent clock is suspect.'
  }
  return {
    metric: series.metric,
    observed: series.observed,
    coverage,
    latest,
    firstObservedAt: series.firstObservedAt ?? null,
    lastObservedAt: series.lastObservedAt ?? null,
    releasedBefore: series.releasedBefore ?? null,
    // Phrased so it reads correctly for any mixture of replays and corrections,
    // which are counted separately by the ledger and are not one quantity.
    evidence:
      parts.length > 1
        ? 'Of the observations it holds, the ledger records ' + parts.slice(1).join(' and ') + '.'
        : null,
  }
}

/** One mount path: what the Server said about it, and the two series behind it. */
export type StorageMountSummary = {
  mountPath: string
  state: StorageMountState
  stateLabel: string
  tone: StorageMountTone
  /** What the state means for this path, with the cadence behind it. */
  observation: string
  used: StorageSeriesSummary
  capacity: StorageSeriesSummary
  /** The newest used reading against the newest capacity reading, or null. */
  share: string | null
}

export function storageMountSummary(
  mount: AdminStorageMount,
  cadenceSeconds: number,
  silenceThresholdSeconds: number,
  collectionPaused: boolean,
): StorageMountSummary {
  const state = storageMountState(mount.observationState)
  return {
    mountPath: mount.mountPath,
    state,
    stateLabel: storageMountStateLabel(state),
    tone: storageMountStateTone(state),
    observation: storageMountObservation(mount, cadenceSeconds, silenceThresholdSeconds, collectionPaused),
    used: storageSeriesSummary(mount.used),
    capacity: storageSeriesSummary(mount.capacity),
    share: formatStorageShare(mount.used.latestValue, mount.capacity.latestValue),
  }
}

/** Everything the card renders, assembled from one Server answer. */
export type StorageMountsView = {
  answeredAt: string
  cadenceSeconds: number
  silenceThresholdSeconds: number
  cadence: string
  silenceThreshold: string
  usedMetric: string
  capacityMetric: string
  mountLimit: number
  /** How the list relates to the evidence the Agent holds. */
  coverageLimit: string
  truncated: boolean
  /** The paths the answer left out, stated rather than dropped. */
  truncation: string | null
  /** True while the Server itself is holding optional history back. */
  collectionPaused: boolean
  /** Why no path is judged silent right now, or null when none is. */
  pause: string | null
  /** The order the Server answered in, which the card keeps as it is. */
  order: string
  mounts: StorageMountSummary[]
}

/**
 * The Server answers the newest path first and settles ties by the path, so the
 * card keeps that order instead of ranking the paths itself.
 */
export function storageMountsOrder(): string {
  return 'Newest observed path first; two paths observed at the same instant are ordered by their path.'
}

export function storageMountsView(answer: AdminAgentStorageMountsResponse): StorageMountsView {
  const cadenceSeconds = answer.cadenceSeconds ?? 0
  const collectionPaused = answer.collectionPaused ?? false
  return {
    answeredAt: answer.answeredAt,
    cadenceSeconds,
    silenceThresholdSeconds: answer.silenceThresholdSeconds ?? 0,
    cadence: formatStorageCadence(cadenceSeconds),
    silenceThreshold: formatStorageSilenceThreshold(
      cadenceSeconds,
      answer.silenceThresholdSeconds ?? 0,
    ),
    usedMetric: answer.usedMetric,
    capacityMetric: answer.capacityMetric,
    mountLimit: answer.mountLimit,
    coverageLimit:
      'This list carries at most ' +
      answer.mountLimit +
      ' mount paths: the newest paths are answered and older ones are left out.',
    truncated: answer.truncated,
    truncation: answer.truncated
      ? 'This Agent holds more mount paths than the list carries. The oldest of them are not in this answer; the paths shown are the newest ones.'
      : null,
    collectionPaused,
    pause: collectionPaused
      ? 'Low-space protection is holding optional history back, so no path is judged silent: the Agent keeps reporting, the readings already stored stay on their series, and the age of a stored reading is not the Agent going quiet.'
      : null,
    order: storageMountsOrder(),
    mounts: answer.mounts.map((mount) =>
      storageMountSummary(
        mount,
        cadenceSeconds,
        answer.silenceThresholdSeconds ?? 0,
        collectionPaused,
      ),
    ),
  }
}
