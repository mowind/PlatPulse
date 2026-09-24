import { useEffect, useState } from 'react'
import type { PublicNode } from '../api/generated'

/** This clock changes display age only, never Server-owned freshness. */
export function LastReportAge({ timestamp }: { timestamp?: string | null }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | undefined
    const synchronize = () => {
      clearInterval(timer)
      if (document.visibilityState !== 'hidden') {
        setNow(Date.now())
        timer = setInterval(() => setNow(Date.now()), 1000)
      }
    }
    synchronize()
    document.addEventListener('visibilitychange', synchronize)
    return () => {
      clearInterval(timer)
      document.removeEventListener('visibilitychange', synchronize)
    }
  }, [timestamp])
  const time = timestamp ? Date.parse(timestamp) : NaN
  if (!Number.isFinite(time)) return <span>Unknown</span>
  const seconds = Math.floor((now - time) / 1000)
  const age = seconds < 60 ? seconds + 's ago'
    : seconds < 3600 ? Math.floor(seconds / 60) + 'm ' + seconds % 60 + 's ago'
      : seconds < 86400 ? Math.floor(seconds / 3600) + 'h ' + Math.floor(seconds % 3600 / 60) + 'm ago'
        : Math.floor(seconds / 86400) + 'd ' + Math.floor(seconds % 86400 / 3600) + 'h ago'
  return <span data-slot="last-report-age" className="flex flex-col gap-0.5">
    <span className={time > now ? 'text-warning-foreground dark:text-warning' : undefined}>{time > now ? 'Future timestamp · check clock' : age}</span>
    <time dateTime={new Date(time).toISOString()} className="text-[11px] text-muted-foreground">{new Date(time).toISOString().replace('T', ' ').replace('Z', ' UTC')}</time>
  </span>
}

type HeadObservation = Pick<PublicNode, 'currentHead' | 'freshness' | 'rpcState' | 'health'>
type NetworkObservation = HeadObservation & Pick<PublicNode, 'networkReferenceHead' | 'networkReferenceConfidence'>

function validHeight(value: number | null | undefined): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
}

function headUnavailable(node: HeadObservation): string | undefined {
  if (!validHeight(node.currentHead)) return 'Node Head unknown or invalid'
  if (node.rpcState !== 'ok') return 'Node RPC collection ' + (node.rpcState || 'unknown')
  // PublicNode.freshness is the oldest receipt timestamp, NOT a state token.
  // Healthy is the existing Server verdict that RPC/sync/consensus are current.
  // Without it this DTO cannot establish Head currency independently: omit only
  // the comparison, retain the absolute value, and never invent a client TTL.
  if (!node.freshness || !Number.isFinite(Date.parse(node.freshness))) return 'Node Head observation time unknown'
  if (node.health !== 'healthy') return 'Current Node Head not confirmed by Server'
}

/**
 * The two references a height on this page is compared against, deliberately
 * worded apart: 'node head' is this Node's own head, 'observed network head' is
 * the Server's cross-Node reference (CONTEXT.md: Observed Network Head). One
 * screen shows both, so an "at head" claim without its reference is ambiguous.
 */
export const NODE_HEAD = 'Node Head'
export const OBSERVED_NETWORK_HEAD = 'Observed Network Head'

/**
 * A height's offset from a reference, as one phrase. Zero is a state, not a
 * measurement: a bare zero invites reading the delta as an observation, so an
 * exact match says where it is instead. The reference name is always printed
 * so the page's two distinct references cannot be confused.
 */
export function offsetLabel(value: number, reference: number, referenceName: string): string {
  const offset = value - reference
  if (offset === 0) return 'At ' + referenceName
  return (offset > 0 ? '+' : '−') + Math.abs(offset).toLocaleString() + ' from ' + referenceName
}

export function networkHeadComparison(node: NetworkObservation): { offset: string; reason?: string } {
  const reason = headUnavailable(node)
    ?? (!validHeight(node.networkReferenceHead) ? 'Observed Network Head unknown or invalid' : undefined)
    ?? (node.networkReferenceConfidence !== 'high' ? 'Observed Network Head confidence ' + (node.networkReferenceConfidence || 'unknown') : undefined)
  if (reason || !validHeight(node.currentHead) || !validHeight(node.networkReferenceHead)) return { offset: '—', reason }
  return { offset: offsetLabel(node.currentHead, node.networkReferenceHead, OBSERVED_NETWORK_HEAD) }
}

/**
 * Sync progress against the Observed Network Head. This deliberately keeps the
 * narrower reference-only gate rather than inheriting the Head delta's stricter
 * currency evidence: the approved target states that sync progress requires the
 * high-confidence reference and is not redefined by the display delta. A
 * low-confidence reference is context, not a progress claim. The direction is
 * worded rather than signed:
 * a Node *ahead* of the reference is a real state (the reference lags, or the
 * Node is leading) and must never be reported as being at it.
 */
export function syncOffsetLabel(node: NetworkObservation): string {
  if (node.currentHead == null || node.networkReferenceHead == null) return 'Reference unavailable; sync progress is Unknown'
  if (node.networkReferenceConfidence !== 'high') {
    return 'Observed Network Head confidence ' + (node.networkReferenceConfidence || 'unknown') + '; progress is not asserted'
  }
  const offset = node.currentHead - node.networkReferenceHead
  if (offset === 0) return 'At ' + OBSERVED_NETWORK_HEAD
  return Math.abs(offset).toLocaleString() + ' blocks ' + (offset > 0 ? 'ahead of' : 'behind') + ' ' + OBSERVED_NETWORK_HEAD
}

export function HeadDelta({ node }: { node: NetworkObservation }) {
  const { offset, reason } = networkHeadComparison(node)
  return <span data-slot="head-delta">{offset}{reason && <span className="block">{reason}</span>}</span>
}

export function ConsensusHeights({ node }: { node: HeadObservation & Pick<PublicNode, 'consensus'> }) {
  const consensus = node.consensus
  const status = [
    consensus?.freshness !== 'current' ? 'Freshness ' + (consensus?.freshness || 'unknown') : null,
    consensus?.state !== 'ok' ? 'Collection ' + (consensus?.state || 'unknown') : null,
  ].filter(Boolean).join(' · ')
  return <div data-slot="consensus-heights" className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,6rem),1fr))] gap-3">
    {([['QC', consensus?.highestQcBlock], ['Locked', consensus?.highestLockBlock], ['Committed', consensus?.highestCommitBlock]] as const).map(([label, height]) => {
      const known = validHeight(height)
      const available = known && consensus.freshness !== 'unknown' && !['starting', 'disabled', 'unsupported'].includes(consensus.state)
      const reason = !available ? 'Height unknown or invalid' : status || headUnavailable(node)
      const comparable = !reason && known && validHeight(node.currentHead)
      return <div key={label} className="min-w-0" role="group" aria-label={label + ' height'}>
        <div className="text-xs text-muted-foreground">{label}</div>
        <strong className="block break-words text-sm font-medium tabular-nums">{available && known ? height.toLocaleString() : 'Unknown'}</strong>
        {status && available && <small className="block text-[11px] text-warning-foreground dark:text-warning">Last-good · {status}</small>}
        <small className="block text-[11px] text-muted-foreground">
          <span data-slot="height-offset">{comparable && known ? offsetLabel(height, node.currentHead as number, NODE_HEAD) : '—'}</span>
          {reason && !(status && available) && <span data-slot="height-offset-reason" className="block">{reason}</span>}
        </small>
      </div>
    })}
  </div>
}
