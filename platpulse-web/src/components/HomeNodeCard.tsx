import { memo, useEffect, useState } from 'react'
import { Link } from 'react-router'
import { Info } from 'lucide-react'
import type { PublicNetwork, PublicNode } from '../api/generated'
import {
  HEAD_COMPARISON_TONE_CLASS,
  OBSERVED_NETWORK_HEAD,
  networkHeadComparison,
  networkHeadOffset,
  nodeHeadUnconfirmed,
  validHeight,
  type HeadComparisonTone,
} from './NodeDetailObservations'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from './PeerInsight'
import { NodeHealthMarker, formatRelativeTime, formatUtcDateTime } from './StatusBadge'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { MetricRow } from './MetricRow'
import { CardX } from './ui/card-x'
import { Button } from './ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from './ui/dialog'
import { healthTone, homeNodeLabel } from '../homeFilters'
import type { HomeReturnState } from '../homeReturn'
import { LinkedValidatorSection, validatorDataStatus, validatorIdentityStatement, type ValidatorDataStatus } from './LinkedValidator'
import { ValidatorActivityBadge } from './ValidatorActivityBadge'
import { HomeResourceMetrics } from './HomeResourceMetrics'
import {
  NODE_CARD_DISCLOSURES,
  RESYNC_DISCLOSURES,
  formatNodeIdentityDescription,
  formatResourceMetricsExplanation,
} from '../lib/nodeDisclosures'
import { SURFACE_CARD_INTERACTIVE } from '../lib/surface'
import { cn } from '../lib/utils'

export type HomeNodeCardProps = {
  network: PublicNetwork
  node: PublicNode
  linkState: HomeReturnState
}

export function formatNumber(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toLocaleString()
}

export function healthLabel(value: string): string {
  if (value === 'healthy') return 'Healthy'
  if (value === 'unhealthy') return 'Unhealthy'
  return 'Unknown'
}

/**
 * Compact Home card, styled as Emerald's NodeCard: a stretched semantic title
 * link to Node Detail, an emerald status dot with a ping ring, a secondary
 * identity line carrying Network, process uptime and Peers, a three-column
 * resource row with thin progress bars, one shared Host network line, the
 * current Block Height with the evidence-backed Observed Network Head comparison
 * beside it (#237, #239), and the three-metric Validator summary or the short
 * identity line a confirmed status allows (#238). Copy, disclosure and
 * identity controls sit outside the anchor so they never trigger navigation.
 * Healthy Nodes carry no routine prose. Exceptional health keeps a short
 * diagnostic line (issue #97); resync progress is a separate, lightweight status
 * area, and every explanation the compaction removed from the face of the card
 * stays in the identity entry.
 */
function areHomeNodeCardPropsEqual(prev: HomeNodeCardProps, next: HomeNodeCardProps): boolean {
  return (
    prev.node === next.node &&
    prev.linkState === next.linkState &&
    prev.network.displayName === next.network.displayName &&
    prev.network.networkKey === next.network.networkKey
  )
}

export const HomeNodeCard = memo(function HomeNodeCard({ network, node, linkState }: HomeNodeCardProps) {
  const tone = healthTone(node.health)
  const diagnostic = exceptionalDiagnostic(node)
  const resyncing = (node.resyncState ?? '').toLowerCase() === 'resyncing'
  // The compact Peer cue the identity line can carry, and the explanations the
  // identity entry keeps in full: nothing the card compresses is lost.
  const peerCue = peerCardCue(node)
  const peerObservation = formatPeerObservation(node)
  const nodeDataBytes = formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  // One compact, abnormal-only Provider data cue for the identity row. A
  // routine Current value stays silent; the region below owns the authoritative
  // no-live-Validator empty state.
  const dataStatus = node.validator ? validatorDataStatus(node.validator) : null
  // The identity line the card shows when the Server does not confirm a
  // Validator, and the same statement the identity entry keeps in full — so the
  // short face and the explanation can never disagree (#238 round 2).
  const identityStatement = node.validator ? validatorIdentityStatement(node.validator) : null

  return (
    <article
      data-slot="node-card"
      data-tone={tone}
      className={cn(
        // Borderless by design (the surface and its hover ring carry the
        // card): Tailwind's preflight leaves border-style: solid behind, so
        // this has to be stated explicitly.
        'group/node-card relative h-full w-full rounded-md border-none transition-all duration-200',
        SURFACE_CARD_INTERACTIVE,
        'hover:z-1 hover:-translate-y-0.5 hover:shadow-[0_0_20px,0_0_0_1px] hover:shadow-emerald-600/10',
        tone === 'bad' && 'shadow-[0_0_0_1px] shadow-red-600/20',
        tone === 'warn' && 'shadow-[0_0_0_1px] shadow-amber-500/20',
      )}
    >
      <CardX
        bordered={false}
        className="bg-transparent"
        contentClassName="gap-2.5"
        headerClassName="!block !p-0"
        header={
          <div
            data-node-region="identity"
            className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-2 gap-y-1.5 px-4 pt-3 pb-2.5"
          >
            <div className="flex min-w-0 items-center gap-2">
              <NodeHealthMarker health={node.health} />
              {/* The 44px target is kept on the anchor; the negative block margin
                  keeps it from inflating the identity row, so the name row and
                  the Network · Uptime row stay content-driven and can sit 6px
                  apart. */}
              <h2 className="min-w-0 text-base font-semibold">
                <Link
                  to={`/nodes/${node.nodeId}`}
                  state={linkState}
                  data-slot="node-link"
                  aria-label={homeNodeLabel(node)}
                  title={homeNodeLabel(node)}
                  className="flex -my-2.5 min-h-11 min-w-0 items-center after:absolute after:inset-0 after:rounded-md focus-visible:outline-none focus-visible:after:ring-[3px] focus-visible:after:ring-ring/50"
                >
                  <span className="truncate">{homeNodeLabel(node)}</span>
                </Link>
              </h2>
            </div>
            <ValidatorActivityBadge validator={node.validator} identityReason={node.validatorIdentityReason} />
            <div
              data-slot="node-identity-meta"
              className="col-span-2 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground"
            >
              {/* Second line (#237): the Node's environment at secondary weight —
                  Network, process uptime and the Peer count. A long Network name
                  wraps here instead of squeezing the count or the status cues, and
                  the count itself never breaks: the identity explanation carries
                  the whole name. */}
              <p className="min-w-0 flex-1">
                <span className="[overflow-wrap:anywhere]">{network.displayName}</span> ·{' '}
                <span className="whitespace-nowrap">Uptime {formatDuration(node.processUptimeMs)}</span> ·{' '}
                <span data-slot="node-peers-meta" className="whitespace-nowrap tabular-nums">
                  Peers {formatPeerCount(node)}
                </span>
                {/* One short last-good/freshness cue: a retained count must never
                    read as a current one, and the full observation sentence stays
                    in the identity explanation this row already opens. */}
                {peerCue && <span data-slot="node-peers-cue" className="ml-1.5">{peerCue}</span>}
              </p>
              {/* One unified top status position: an abnormal Node-health summary,
                  a live resync percentage and an abnormal Provider data state sit
                  here as short chips. The normal Data: Current line is gone; the
                  full reason for each stays in the explanation and Node detail. */}
              <span
                data-slot="node-status-slot"
                className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-x-1.5 gap-y-1"
              >
                {resyncing && <ResyncCue node={node} />}
                {dataStatus && <ValidatorDataCue status={dataStatus} />}
                {/* State audit of the reference capture's conspicuous frame: the control is
                    transparent by default (transparent background, transparent 1px border, no
                    outline or shadow), paints the ghost accent on hover, and paints the shared
                    3px focus-visible ring while keyboard-focused. The open dialog also gives the
                    trigger aria-expanded's accent background. The captured frame is that
                    focus-visible ring — required keyboard feedback, so it is kept; the icon
                    matches the copy/Details controls at 14px. */}
                <Dialog>
                  <DialogTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="relative z-10 -my-3.5 size-11 shrink-0"
                      aria-label="Node identity details"
                    >
                      <Info className="size-3.5" />
                    </Button>
                  </DialogTrigger>
                  <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm">
                    <DialogTitle className="pr-10 [overflow-wrap:anywhere]">{homeNodeLabel(node)}</DialogTitle>
                    <DialogDescription className="[overflow-wrap:anywhere]">
                      {formatNodeIdentityDescription(network.displayName, formatDuration(node.processUptimeMs))}
                    </DialogDescription>
                    <p className="text-sm text-muted-foreground">{NODE_CARD_DISCLOSURES.activeAndHealth}</p>
                    <p className="text-sm text-muted-foreground">{NODE_CARD_DISCLOSURES.attention}</p>
                    <p data-slot="node-head-explanation" className="text-sm text-muted-foreground">
                      Block Height: {headComparisonExplanation(node)}
                    </p>
                    <p className="text-sm text-muted-foreground">
                      {formatResourceMetricsExplanation(nodeDataBytes)}
                    </p>
                    {peerObservation && (
                      <p className="text-sm text-muted-foreground">Peer observation: {peerObservation}.</p>
                    )}
                    {diagnostic && (
                      <p data-slot="node-diagnostic-explanation" className="text-sm text-muted-foreground">
                        Current health diagnostic: {diagnostic.detail}
                      </p>
                    )}
                    {identityStatement && (
                      <p data-slot="node-validator-identity-explanation" className="text-sm text-muted-foreground">
                        Validator identity: {identityStatement.label} — {identityStatement.detail}
                      </p>
                    )}
                    {dataStatus && (
                      <p className="text-sm text-muted-foreground">
                        Validator data: {dataStatus.label}. {dataStatus.description}
                      </p>
                    )}
                  </DialogContent>
                </Dialog>
              </span>
            </div>
            {/* Abnormal reasons remain readable in one short line, and the Server's
                own sentence stays reachable as that line's title and in the identity
                entry. Healthy cards create no empty row. */}
            {diagnostic && (
              <p
                data-slot="node-diagnostic"
                data-tone={diagnostic.tone}
                title={diagnostic.detail}
                className={cn(
                  'col-span-2 m-0 text-[11px] [overflow-wrap:anywhere]',
                  diagnostic.tone === 'destructive' ? 'text-destructive' : 'text-warning-foreground dark:text-warning',
                )}
              >
                {diagnostic.text}
              </p>
            )}
          </div>
        }
      >
        <div data-node-region="resource">
          <HomeResourceMetrics node={node} />
        </div>
        {/* Chain (#237, #239): Home keeps the current Block Height alone as one
            compact key-value line, and names where that height sits against the
            deployment-local Observed Network Head only while the Server's own
            evidence supports the comparison. QC, Locked, Committed and Txs stay
            on Node Detail, which reads the same public fields — no field,
            observation or history is deleted. */}
        <div data-node-region="chain">
          <div data-slot="node-business-metrics" className="border-t border-border pt-3">
            {/* One head line: the height row is the flexible track and the short
                comparison the fixed one, so the comparison shares the line while
                the card can hold both and drops to its own right-aligned line when
                the tightest track cannot — never overflowing, clipping or
                shrinking the reading itself. */}
            <div data-slot="node-head-line" className="flex flex-wrap items-baseline justify-end gap-x-2 gap-y-0.5">
              {/* An unusable height — absent, negative, fractional or non-finite —
                  is a height the Server never validly observed, so it stays
                  explicitly Unknown rather than being restated as a reading the
                  reader could trust, and it supports no comparison either. */}
              <MetricRow
                className="min-w-0 flex-[1_1_auto]"
                layout="compact"
                label="Head"
                value={validHeight(node.currentHead) ? formatNumber(node.currentHead) : 'Unknown'}
              />
              <HeadComparison node={node} />
              <HeadRetainedMark node={node} />
            </div>
          </div>
        </div>
        <div data-node-region="validator">
          <LinkedValidatorSection node={node} />
        </div>
      </CardX>
    </article>
  )
}, areHomeNodeCardPropsEqual)
HomeNodeCard.displayName = 'HomeNodeCard'

/** Home's short form of the evidence-backed comparison (#239): the gated offset
 *  and its display tone are the Node Detail page's own (`networkHeadOffset`),
 *  and only this wording is Home's. An exact match is a state rather than a
 *  measurement, and a Node ahead of the reference is a leading state — never a
 *  clamped zero and never written as synchronized. */
export function homeHeadComparison(node: PublicNode): { label: string; tone: HeadComparisonTone } | null {
  const offset = networkHeadOffset(node)
  if (offset === null) return null
  if (offset.blocks === 0) return { label: 'At observed head', tone: offset.tone }
  const blocks = Math.abs(offset.blocks)
  return {
    label: (offset.blocks > 0 ? 'Behind ' : 'Ahead ') + blocks.toLocaleString() + ' block' + (blocks === 1 ? '' : 's'),
    tone: offset.tone,
  }
}

export const HEAD_RETAINED_LABEL = 'Last-good'

export const OBSERVED_NETWORK_HEAD_NOTE =
  'The ' +
  OBSERVED_NETWORK_HEAD +
  " is this deployment's Server-side cross-Node reference, not the public chain head, and the comparison does not claim the Node is synchronized."

export function HeadComparison({ node }: { node: PublicNode }) {
  const comparison = homeHeadComparison(node)
  if (comparison === null) return null
  return (
    <small
      data-slot="node-head-comparison"
      data-tone={comparison.tone}
      title={'Compared with the ' + OBSERVED_NETWORK_HEAD + ': ' + comparison.label + '. ' + OBSERVED_NETWORK_HEAD_NOTE}
      className={cn(
        'min-w-0 text-right text-[11px] leading-[1.375rem] [overflow-wrap:anywhere]',
        HEAD_COMPARISON_TONE_CLASS[comparison.tone],
      )}
    >
      {comparison.label}
    </small>
  )
}

export function HeadRetainedMark({ node }: { node: PublicNode }) {
  const reason = nodeHeadUnconfirmed(node)
  if (!reason) return null
  return (
    <small
      data-slot="node-head-retained"
      title={
        HEAD_RETAINED_LABEL +
        ' value: ' +
        reason +
        '. The Server is not confirming this as the Node’s current Block Height, so no comparison is asserted.'
      }
      className="min-w-0 text-right text-[11px] leading-[1.375rem] text-warning-foreground dark:text-warning [overflow-wrap:anywhere]"
    >
      {HEAD_RETAINED_LABEL}
    </small>
  )
}

export function headComparisonExplanation(node: PublicNode): string {
  const retained = nodeHeadUnconfirmed(node)
  const absolute = retained
    ? 'the last Block Height the Server retained for this Node, marked ' + HEAD_RETAINED_LABEL + ' because ' + retained + '.'
    : 'the current head the Server published for this Node; a height the Server never observed stays Unknown rather than a fabricated zero.'
  const tone =
    'Its colour is a display level only — an exact match uses the success colour, a lag of 1 to 10 blocks the warning colour, a longer lag the error colour, and a Node ahead of the reference stays neutral — never a health, alert or Attention rule.'
  const comparison = homeHeadComparison(node)
  const boundary =
    comparison !== null
      ? ' Shown here: ' + comparison.label + '.'
      : retained
        ? ' No comparison is asserted from a retained height.'
        : ' No comparison is asserted here: ' +
          (networkHeadComparison(node).reason ?? 'the observation evidence does not support one') +
          '.'
  return absolute + boundary + ' ' + OBSERVED_NETWORK_HEAD_NOTE + ' ' + tone
}

type ResyncProgress = {
  rounded: string
  approximate: boolean
  share: string | null
}

export const APPROXIMATE_TARGET_LABEL = '≈100%'

export function resyncProgress(current: number | null, target: number | null): ResyncProgress | null {
  if (current === null || target === null || target <= 0) return null
  const share = (current / target) * 100
  const rounded = share.toFixed(2) + '%'
  return { rounded, approximate: share < 100 && rounded === '100.00%', share: readableShare(share) }
}

export function readableShare(share: number): string | null {
  for (const digits of [4, 6, 8, 10]) {
    const text = share.toFixed(digits).replace(/0+$/, '').replace(/\.$/, '')
    if (Number(text) < 100) return text + '%'
  }
  return null
}

export function ResyncCue({ node }: { node: PublicNode }) {
  const [, refreshAge] = useState(0)
  useEffect(() => {
    const timer = window.setInterval(() => refreshAge((tick) => tick + 1), 15_000)
    return () => window.clearInterval(timer)
  }, [])

  const current = validHeight(node.currentHead) ? node.currentHead : null
  const target = validHeight(node.historicalHighWatermark) ? node.historicalHighWatermark : null
  const progress = resyncProgress(current, target)
  const percent = progress === null ? null : progress.approximate ? APPROXIMATE_TARGET_LABEL : progress.rounded
  const timestamp = node.resyncLastProgressAt
  const date = validProgressDate(timestamp)

  return (
    <Dialog>
      <DialogTrigger asChild>
        <button
          type="button"
          data-slot="node-resync-cue"
          className="relative z-10 -my-3.5 inline-flex min-h-11 items-center rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={
            progress === null
              ? `Resync state: ${node.resyncState || 'unknown'}; no progress percentage is asserted`
              : progress.approximate
                ? 'Resync progress: approximately 100% toward the Historical High-Water Mark, short of the target; the share is stated in the dialog'
                : `Resync progress: ${percent} toward the Historical High-Water Mark`
          }
        >
          <span className="inline-flex items-center gap-1 rounded border border-amber-500/40 px-1.5 py-0.5 text-[11px] font-medium leading-4 text-amber-500 dark:text-amber-400">
            <span>Resyncing</span>
            {percent !== null && <span className="tabular-nums">{percent}</span>}
          </span>
        </button>
      </DialogTrigger>
      <DialogContent className="max-h-[85dvh] overflow-y-auto rounded-md shadow-sm">
        <DialogTitle>Resync progress</DialogTitle>
        <DialogDescription>{RESYNC_DISCLOSURES.target}</DialogDescription>
        <p className="m-0 text-sm tabular-nums">
          {formatNumber(current)} / {formatNumber(target)}
        </p>
        {percent === null && (
          <p data-slot="resync-progress-unavailable" className="m-0 text-sm">
            {RESYNC_DISCLOSURES.percentageUnavailable}
          </p>
        )}
        {progress !== null && progress.approximate && (
          <p data-slot="resync-progress-rounding" className="m-0 text-sm tabular-nums">
            {progress.share === null
              ? `Below the Historical High-Water Mark by less than any decimal this card prints, where two decimal places print ${progress.rounded}`
              : `Below the Historical High-Water Mark: the closest reading this card prints is ${progress.share}, where two decimal places print ${progress.rounded}`}
            {`, which is why the cue reads ${APPROXIMATE_TARGET_LABEL} rather than a finished percentage. The Resyncing state stays the Server's own: a rounded number neither reaches the target nor ends the Resync.`}
          </p>
        )}
        {date ? (
          <p className="m-0 text-sm">
            Last progress <time dateTime={date.toISOString()}>{formatRelativeTime(date)}</time> ·{' '}
            {formatUtcDateTime(date)}
          </p>
        ) : (
          <p className="m-0 text-sm">Last progress: Unknown</p>
        )}
        {date && timestamp && (
          <p
            className="m-0 min-w-0 overflow-x-auto whitespace-nowrap font-mono text-xs text-muted-foreground"
            tabIndex={0}
            aria-label="Full source timestamp with UTC offset"
          >
            {timestamp}
          </p>
        )}
      </DialogContent>
    </Dialog>
  )
}

export function ValidatorDataCue({ status }: { status: ValidatorDataStatus }) {
  return (
    <span
      data-slot="validator-data-cue"
      data-tone={status.tone}
      role="status"
      title={status.description}
      aria-label={`Validator data: ${status.label}. ${status.description}`}
      className={cn(
        'inline-flex min-w-0 items-center gap-1 rounded border px-1.5 py-0.5 text-[11px] leading-4',
        status.tone === 'destructive'
          ? 'border-destructive/40 text-destructive'
          : 'border-amber-500/40 text-amber-600 dark:text-amber-400',
      )}
    >
      <span
        className={cn(
          'inline-block size-1.5 shrink-0 rounded-full',
          status.tone === 'destructive' ? 'bg-destructive' : 'bg-amber-500',
        )}
        aria-hidden="true"
      />
      <span className="whitespace-nowrap">Data: {status.label}</span>
    </span>
  )
}

export function validProgressDate(value: string | null | undefined): Date | null {
  if (
    typeof value !== 'string' ||
    !/^\d{4}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12]\d|3[01])T(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/i.test(
      value,
    )
  )
    return null
  const date = new Date(value)
  const calendarDate = value.slice(0, 10)
  const calendar = new Date(calendarDate + 'T00:00:00Z')
  if (!Number.isFinite(date.getTime()) || calendar.toISOString().slice(0, 10) !== calendarDate) return null
  return date
}

type NodeDiagnostic = {
  text: string
  detail: string
  tone: 'destructive' | 'warning'
}

export const DIAGNOSTIC_MAX_CHARS = 40

export const DIAGNOSTIC_SHORT_FORMS: ReadonlyArray<readonly [RegExp, string]> = [
  [/stale or unknown/i, 'Observations stale / unknown'],
  [/unknown or unsupported/i, 'Observations unknown or unsupported'],
]

export function shortDiagnostic(detail: string): string {
  const short = DIAGNOSTIC_SHORT_FORMS.find(([pattern]) => pattern.test(detail))?.[1]
  if (short) return short
  if (detail.length <= DIAGNOSTIC_MAX_CHARS) return detail
  const cut = detail.slice(0, DIAGNOSTIC_MAX_CHARS)
  const boundary = cut.lastIndexOf(' ')
  return (boundary > 0 ? cut.slice(0, boundary) : cut).trimEnd() + '…'
}

export function exceptionalDiagnostic(node: PublicNode): NodeDiagnostic | null {
  if (node.health !== 'healthy') {
    const detail = node.healthReason?.trim() || `Health ${healthLabel(node.health).toLowerCase()}`
    return {
      text: shortDiagnostic(detail),
      detail,
      tone: node.health === 'unhealthy' ? 'destructive' : 'warning',
    }
  }
  return null
}

export function formatPeerCount(node: PublicNode) {
  return formatNumber(node.peers?.peerCount)
}

export function formatPeerObservation(node: PublicNode): string | undefined {
  const peer = node.peers
  if (!peer) return 'Unknown observation'

  const collection = peerInsightCollectionStatus(peer)
  const freshness = peerInsightFreshnessStatus(peer)
  const value = peerInsightValueStatus(peer)
  const hasValue = value !== 'Unknown'
  const qualifiers: string[] = []

  if (collection === 'Error') qualifiers.push('Collection failed')
  else if (collection !== 'Current') qualifiers.push('Collection ' + collection.toLowerCase())
  if (freshness === 'Stale') qualifiers.push('Stale')
  else if (freshness !== 'Current') qualifiers.push('Freshness unknown')

  if (!hasValue) {
    if (qualifiers.length === 0) qualifiers.push('Value unknown')
    qualifiers.push('No successful Peer snapshot is available')
    return qualifiers.join('; ')
  }

  if (qualifiers.length > 0) {
    qualifiers.push('Showing last successful snapshot')
    if (value === 'Empty') qualifiers.push('authoritative zero')
    return qualifiers.join('; ')
  }

  if (value === 'Empty') return 'Empty; authoritative zero'
  return undefined
}

export function peerRetentionCue(node: PublicNode): string | undefined {
  const peer = node.peers
  if (!peer || peerInsightValueStatus(peer) === 'Unknown') return undefined
  const reasons: string[] = []
  const collection = peerInsightCollectionStatus(peer)
  const freshness = peerInsightFreshnessStatus(peer)
  if (collection !== 'Current') {
    reasons.push(collection === 'Unknown' ? 'collection unknown' : `collection ${collection.toLowerCase()}`)
  }
  if (freshness !== 'Current') {
    reasons.push(freshness === 'Unknown' ? 'freshness unknown' : `freshness ${freshness.toLowerCase()}`)
  }
  if (reasons.length === 0) return undefined
  return `last good (${reasons.join(', ')})`
}

export function peerCardCue(node: PublicNode): string | undefined {
  const value = peerInsightValueStatus(node.peers)
  if (value === 'Unknown') return 'no successful snapshot'
  const retention = peerRetentionCue(node)
  if (retention) return retention
  return value === 'Empty' ? 'Empty; authoritative zero' : undefined
}
