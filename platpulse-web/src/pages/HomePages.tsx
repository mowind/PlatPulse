import { ArrowLeft, Check, Copy } from 'lucide-react'
import { useId, useState } from 'react'
import type { ReactNode } from 'react'
import { Link, useParams } from 'react-router'
import {
  usePublicNode,
  usePublicNodeHistory,
  usePublicNodeMetrics,
} from '../api/public'
import type { PublicMetricPoint, PublicNode, PublicNodeMetricHistory } from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from '../components/PeerInsight'
import { formatUtcDateTime, NodeHealthMarker } from '../components/StatusBadge'
import { LinkedValidatorSection } from '../components/LinkedValidator'
import GeoMapBoundary from '../components/GeoMapBoundary'
import GeoWorldMap from '../components/GeoWorldMap'
import { RealtimeNotice } from '../components/RealtimeNotice'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from '../components/MetricRow'
import { ValidatorActivityBadge } from '../components/ValidatorActivityBadge'
import { ConsensusHeights, HeadDelta, LastReportAge, syncOffsetLabel, validHeight } from '../components/NodeDetailObservations'
import { geoMapStatus, nodeGeoOverview } from '../homeGeo'
import { NODE_PEER_COUNTRIES_HEADING } from '../components/geoPresentation'
import { CardX } from '../components/ui/card-x'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Spinner } from '../components/ui/spinner'
import { SURFACE_CARD_STATIC, SURFACE_CARD_SUMMARY } from '../lib/surface'
import { Disclosure } from '../components/ui/disclosure'
import { DataTooltip } from '../components/ui/data-tooltip'
import { Button } from '../components/ui/button'
import { cn } from '../lib/utils'

const PAGE = 'min-w-0 p-4'
// Info/chart tier; the summary tiles sit one step brighter, the disclosures one step dimmer.
const CARD = cn('min-w-0 rounded-md border-none', SURFACE_CARD_STATIC)
const SUMMARY_CARD = cn('min-w-0 rounded-md border-none', SURFACE_CARD_SUMMARY)

export function NodePage() {
  const { nodeId = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const nodeQuery = usePublicNode(nodeId, generation)
  const historyQuery = usePublicNodeHistory(nodeId, generation)
  const metricsQuery = usePublicNodeMetrics(nodeId, generation)

  if (resetting) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className={cn(PAGE, 'relative min-h-32')}><RealtimeNotice realtime={realtime} /><Spinner label="Loading Node"><span className="text-sm text-muted-foreground">Loading Node…</span></Spinner></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><Alert variant="destructive" className="mt-3 rounded-md border-none bg-red-400/10"><AlertDescription>{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</AlertDescription></Alert><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>
  if (!nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Node unavailable.</p><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to="/">Back to Home</Link></section>

  const node = nodeQuery.data
  // The Node Peer Country View is a projection of exactly this Node. The Node
  // response has already loaded, so the map slot is never Starting or
  // Unavailable here; its status is the Server's own Geo state.
  const nodeGeo = nodeGeoOverview(node)
  const nodeGeoStatus = geoMapStatus(nodeGeo, { loading: false, hasProjection: true })
  const health = nodeHealthPresentation(node.health)
  const blockInterval = latestBlockInterval(historyQuery.data)
  const metricHistory = metricsQuery.data
  const metricHistoryMessage = metricHistory
    ? undefined
    : metricsQuery.isPending
      ? 'Loading metric history…'
      : metricsQuery.error
        ? 'Metric history unavailable'
        : undefined
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const metricWindowTitle = describeMetricWindow(metricHistory)
  const validatorNode = nodeUsesValidatorComposition(node)

  return <section className={PAGE}>
    <div data-slot="node-detail-breadcrumb" className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-medium text-muted-foreground">
        <Link className="inline-flex min-h-11 min-w-11 items-center hover:text-foreground" to="/"><ArrowLeft size={16} aria-hidden="true" />All Networks</Link>
        <span aria-hidden="true">/</span>
        <span>Node detail</span>
      </div>
      <RealtimeNotice realtime={realtime} />
    </div>
    {nodeQuery.isRefetchError && <p role="status" className="mt-2 text-sm text-destructive">Node refresh failed; showing the last successful Node data.</p>}

    {validatorNode ? <ValidatorIdentityHeader node={node} /> : <CompactNodeHeader node={node} />}

    {health.tone !== 'ok' && <p className="m-0 mt-2 break-words text-sm text-warning-foreground dark:text-warning">{node.healthReason}</p>}

    {/* The first row pairs the summary tiles with the Node Peer Country View. A
        Validator keeps its established composition; an ordinary Node leads with
        six compact operational tiles and its Chain state, so the map never
        becomes the first thing read. The map is uncarded, so §11.1's
        card-container rule is unchanged. */}
    {validatorNode ? (
      <div data-slot="node-overview" className="mt-4 grid min-w-0 items-end gap-4 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
        <div className="grid min-w-0 auto-rows-fr grid-cols-2 gap-3" role="group" aria-label="Node key summary">
          <SummaryTile label="Head" value={formatNumber(node.currentHead)} detail={<HeadDelta node={node} />} />
          <SummaryTile label="Sync" value={nodeComponentStateLabel(node.syncState)} detail={syncOffsetLabel(node)} />
          <SummaryTile label="Peers" value={peerCount(node.peers)} detail={peerBreakdown(node.peers)} />
          <SummaryTile label="Process uptime" value={formatDuration(node.processUptimeMs)} />
        </div>
        <div data-slot="node-map" className="order-first min-w-0 aspect-[2/1] lg:order-none xl:aspect-auto xl:h-64">
          <GeoMapBoundary label={NODE_PEER_COUNTRIES_HEADING}>
            <GeoWorldMap overview={nodeGeo} status={nodeGeoStatus} heading={NODE_PEER_COUNTRIES_HEADING} />
          </GeoMapBoundary>
        </div>
      </div>
    ) : (
      <div data-slot="node-overview" className="mt-4 grid min-w-0 items-end gap-4 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
        <div className="grid min-w-0 auto-rows-fr grid-cols-2 gap-3 sm:grid-cols-3" role="group" aria-label="Node key summary">
          <SummaryTile dense label="Head" value={formatNumber(node.currentHead)} detail={headLagDetail(node)} />
          <SummaryTile dense label="Sync" value={nodeComponentStateLabel(node.syncState)} detail={syncLagDetail(node)} />
          <SummaryTile dense label="Peers" value={peerCount(node.peers)} detail={peerDirectionSummary(node.peers)} />
          <SummaryTile dense label="Uptime" value={formatDuration(node.processUptimeMs)} />
          <SummaryTile dense label="Block interval" value={blockInterval.value} detail={blockInterval.value === 'Unknown' ? blockInterval.detail : undefined} />
          <SummaryTile dense label="Transactions / block" value={formatNumber(node.latestBlockTransactionCount)} />
        </div>
        <div data-slot="node-map" className="min-w-0 aspect-[2/1] xl:aspect-auto xl:h-64">
          <GeoMapBoundary label={NODE_PEER_COUNTRIES_HEADING}>
            <GeoWorldMap overview={nodeGeo} status={nodeGeoStatus} heading={NODE_PEER_COUNTRIES_HEADING} />
          </GeoMapBoundary>
        </div>
      </div>
    )}

    {validatorNode ? (
      <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-3">
        <NodeInfoGroup title="Chain & consensus" label="Node chain and consensus observations">
          <ConsensusHeights node={node} />
          <MetricRow label="Validator" value={formatValidatorMembership(node)} />
          <MetricRow label="Resync" value={nodeComponentStateLabel(node.resyncState)} detail={formatResyncDetail(node)} />
          <MetricRow label="Network reference" value={formatReferenceHead(node)} detail={formatReferenceDetail(node)} />
        </NodeInfoGroup>

        <NodeInfoGroup title="PlatON process & Node Data" label="PlatON process resources">
          <MetricRow label="CPU" value={formatPercent(node.processCpuPercent)} progress={node.processCpuPercent} />
          <MetricRow label="Memory" value={formatPercent(node.processMemoryPercent)} progress={node.processMemoryPercent} />
          <MetricRow label="Started" value={formatUtcDateTime(node.processStartedAt)} />
          <MetricRow label="Process state" value={nodeComponentStateLabel(node.processState)} />
          <div className="mt-1 grid grid-cols-1 gap-2 border-t border-dashed border-border/60 pt-2" role="group" aria-label="Node data directory">
            <MetricRow
              label="Directory usage"
              value={formatPercent(nodeDataProgressValue)}
              detail={(formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes) ?? 'Unknown') + ' · directory size against the hosting filesystem capacity, not whole-Host disk usage'}
              progress={nodeDataProgressValue}
            />
          </div>
        </NodeInfoGroup>

        <NodeInfoGroup title="Host resources" label="Shared Host resources" note="Collected once per Agent; shared by every Node it monitors">
          <MetricRow label="Host CPU" value={formatPercent(node.hostCpuPercent)} progress={node.hostCpuPercent} />
          <MetricRow label="Host memory" value={formatPercent(node.hostMemoryPercent)} progress={node.hostMemoryPercent} />
          <MetricRow label="Host storage" value={formatPercent(node.hostStoragePercent)} progress={node.hostStoragePercent} />
          <MetricRow label="Host upload" value={formatRate(node.hostNetworkTxBytesPerSec)} />
          <MetricRow label="Host download" value={formatRate(node.hostNetworkRxBytesPerSec)} />
        </NodeInfoGroup>
      </div>
    ) : (
      <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-3">
        <ChainStateGroup node={node} />
        <ProcessGroup node={node} nodeDataProgressValue={nodeDataProgressValue} />
        <HostResourcesGroup node={node} />
      </div>
    )}

    {validatorNode && <LinkedValidatorSection node={node} variant="detail" />}

    <section data-slot="node-metrics-section" className="mt-4" aria-labelledby="node-metrics-title">
      <header className="mb-2">
        <h2 id="node-metrics-title" className="m-0 text-lg font-semibold">{metricWindowTitle}</h2>
      </header>
      <div className="grid auto-rows-fr grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
        <NodeMetricCard
          label="Process CPU"
          unit="%"
          value={formatPercent(node.processCpuPercent)}
          detail={processStatusDetail(node.processState)}
          tone="blue"
          fixedMax={percentChartMax(metricHistory?.processCpuPercent ?? [])}
          series={[{ label: 'Process CPU', points: metricHistory?.processCpuPercent ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatPercentAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Process memory"
          unit="%"
          value={formatPercent(node.processMemoryPercent)}
          detail={processStatusDetail(node.processState)}
          tone="cyan"
          fixedMax={percentChartMax(metricHistory?.processMemoryPercent ?? [])}
          series={[{ label: 'Process memory', points: metricHistory?.processMemoryPercent ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatPercentAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Host network"
          value={formatRate(node.hostNetworkTxBytesPerSec)}
          valueLabel="Upload"
          tone="blue"
          series={[
            { label: 'Upload', points: metricHistory?.networkTxBytesPerSec ?? [] },
            { label: 'Download', points: metricHistory?.networkRxBytesPerSec ?? [], secondary: true },
          ]}
          showLegend
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatRate}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Peer connections"
          value={peerCount(node.peers)}
          detail={peerBreakdown(node.peers)}
          tone="blue"
          series={[
            { label: 'Inbound', points: metricHistory?.peerInboundCount ?? [] },
            { label: 'Outbound', points: metricHistory?.peerOutboundCount ?? [], secondary: true },
          ]}
          showLegend
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatCountAxis}
          historyMessage={metricHistoryMessage}
        />
        <NodeMetricCard
          label="Block interval"
          value={blockInterval.value}
          detail={historyQuery.error ? 'History unavailable' : blockInterval.detail}
          tone="amber"
          series={[{ label: 'Block interval', points: metricHistory?.blockIntervalMs ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatMillisecondsAxis}
          historyMessage={metricHistoryMessage}
          chartKind="bar"
        />
        <NodeMetricCard
          label="Transactions per block"
          value={formatNumber(node.latestBlockTransactionCount)}
          tone="violet"
          series={[{ label: 'Transactions per block', points: metricHistory?.transactionCount ?? [] }]}
          from={metricHistory?.from}
          to={metricHistory?.to}
          windowSeconds={metricHistory?.windowSeconds}
          axisFormat={formatCountAxis}
          historyMessage={metricHistoryMessage}
          chartKind="bar"
        />
      </div>
    </section>

    <Disclosure title="Identifiers and technical details" description="Node ID, component states, and reference context">
      <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(14rem,1fr))] gap-x-4 gap-y-2">
        <TechnicalFact label="Node ID" value={<code className="break-all font-mono">{node.nodeId}</code>} />
        <TechnicalFact label="Network key" value={<code className="break-all font-mono">{node.networkKey}</code>} />
        <TechnicalFact label="RPC state" value={nodeComponentStateLabel(node.rpcState)} />
        <TechnicalFact label="Sync state" value={nodeComponentStateLabel(node.syncState)} />
        <TechnicalFact label="Consensus state" value={nodeComponentStateLabel(node.consensusState)} />
        <TechnicalFact label="Process state" value={nodeComponentStateLabel(node.processState)} />
        <TechnicalFact label="Historical high watermark" value={formatNumber(node.historicalHighWatermark)} />
        <TechnicalFact label="Observed Network Head" value={formatNumber(node.networkReferenceHead)} />
        <TechnicalFact label="Reference confidence" value={node.networkReferenceConfidence || 'Unknown'} />
      </dl>
    </Disclosure>
  </section>
}

function TechnicalFact({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="m-0 break-words text-sm tabular-nums">{value}</dd>
    </div>
  )
}

function SummaryTile({ label, value, detail, dense = false }: { label: string; value: ReactNode; detail?: ReactNode; dense?: boolean }) {
  return (
    <CardX bordered={false} data-slot="node-summary-tile" className={cn('group min-w-0', SUMMARY_CARD)} contentClassName="flex h-full flex-col gap-1">
      <span className={cn('break-words font-medium tracking-wider text-muted-foreground', dense ? 'text-[11px]' : 'text-xs')}>{label}</span>
      <strong className={cn('min-w-0 break-words font-bold leading-none tracking-tight tabular-nums', dense ? 'text-base md:text-lg' : 'text-lg md:text-2xl')}>{value}</strong>
      {detail != null && detail !== '' && <small className={cn('break-words text-muted-foreground', dense ? 'text-[10px]' : 'text-[11px]')}>{detail}</small>}
    </CardX>
  )
}

function NodeInfoGroup({ title, label, note, children }: { title: string; label: string; note?: string; children: ReactNode }) {
  return (
    <CardX
      bordered={false}
      role="region"
      aria-label={label}
      data-slot="node-info-group"
      className={CARD}
      contentClassName="flex h-full min-w-0 flex-col gap-2"
    >
      <h2 className="m-0 flex flex-wrap items-baseline gap-x-2 gap-y-1 text-sm font-medium">
        {title}
        {note && <span className="text-[11px] font-normal text-muted-foreground">{note}</span>}
      </h2>
      <div className="grid grid-cols-1 gap-2">{children}</div>
    </CardX>
  )
}



function nodeUsesValidatorComposition(node: PublicNode): boolean {
  const validator = node.validator
  // Only an explicit Node Validator Link proves a staking identity, and the
  // Server's Current Validator Status is the only staking verdict (ADR
  // 0005/0006); consensus membership is never used. An authoritative
  // `not_validator` verdict means the chain key has no current staking
  // identity, so the Node takes the ordinary composition and renders no Linked
  // Validator region at all — never a "Not a Validator" card. An unestablished
  // `unknown` verdict is not a negative one, so a linked identity keeps its
  // region and the existing non-verdict presentation.
  return validator != null && (validator.currentValidatorStatus ?? '').toLowerCase() !== 'not_validator'
}

function nodeRoleLabel(node: PublicNode): string {
  const activity = typeof node.validator?.activity === 'string' ? node.validator.activity.trim().toLowerCase() : ''
  if (activity === 'exiting' || activity === 'exited' || activity === 'locked' || activity === 'candidate') {
    return activity[0].toUpperCase() + activity.slice(1)
  }
  return 'Observer'
}

function nodeSyncStateLabel(node: PublicNode): string | null {
  const label = nodeComponentStateLabel(node.syncState)
  return label === 'Unknown' ? null : label
}

const TONE_OK = 'text-success'
const TONE_WARNING = 'text-warning-foreground dark:text-warning'
const TONE_ERROR = 'text-destructive'
const TONE_MUTED = 'text-muted-foreground'
const HEAD_LAG_WARNING_MAX = 10

type HeadLagPresentation = { blocks: number; label: string; detail?: string; toneClass: string }

/** Head lag = Observed Network Head − this Node's Head, computed from the two
 *  heights the Server already publishes. Without both heights there is no lag
 *  row at all. A reference the Server has not rated high-confidence is reported
 *  without a health verdict, so the number is never asserted from weak evidence. */
function headLagPresentation(node: PublicNode): HeadLagPresentation | null {
  if (!validHeight(node.currentHead) || !validHeight(node.networkReferenceHead)) return null
  const blocks = node.networkReferenceHead - node.currentHead
  const confirmed = node.networkReferenceConfidence === 'high'
  const toneClass = !confirmed
    ? TONE_MUTED
    : blocks <= 1
      ? TONE_OK
      : blocks <= HEAD_LAG_WARNING_MAX
        ? TONE_WARNING
        : TONE_ERROR
  const label = blocks === 0
    ? 'At network head'
    : blocks < 0
      ? Math.abs(blocks).toLocaleString() + ' block' + (blocks === -1 ? '' : 's') + ' ahead'
      : blocks.toLocaleString() + ' block' + (blocks === 1 ? '' : 's')
  const detail = confirmed ? undefined : 'Network Head confidence ' + (node.networkReferenceConfidence || 'unknown')
  return { blocks, label, detail, toneClass }
}

function headLagDetail(node: PublicNode): string | undefined {
  const lag = headLagPresentation(node)
  if (!lag) return undefined
  if (lag.blocks === 0) return 'At network head'
  return Math.abs(lag.blocks).toLocaleString() + (lag.blocks > 0 ? ' from network head' : ' ahead of network head')
}

function syncLagDetail(node: PublicNode): string | undefined {
  const lag = headLagPresentation(node)
  if (!lag) return undefined
  if (lag.blocks === 0) return 'At network head'
  return Math.abs(lag.blocks).toLocaleString() + ' block' + (Math.abs(lag.blocks) === 1 ? '' : 's') + (lag.blocks > 0 ? ' behind' : ' ahead')
}

function peerDirectionSummary(insight: PublicNode['peers']): string {
  if (insight.inboundCount != null && insight.outboundCount != null) {
    return insight.inboundCount.toLocaleString() + ' inbound · ' + insight.outboundCount.toLocaleString() + ' outbound'
  }
  return peerBreakdown(insight)
}

/** CPU/Memory curves are read against the bucket the window actually reached; a
 *  fixed 0–100% axis flattens a low-utilisation Node onto the baseline. The
 *  samples themselves are never rescaled, only the displayed range. */
function percentChartMax(points: PublicMetricPoint[]): number {
  const values = points.map((point) => point.value).filter(Number.isFinite)
  const max = values.length > 0 ? Math.max(...values) : 0
  for (const bucket of [5, 10, 25, 50]) if (max <= bucket) return bucket
  return 100
}

function processStateDot(state: string | null | undefined): string {
  const value = typeof state === 'string' ? state.trim().toLowerCase() : ''
  if (value === 'ok') return 'bg-success'
  if (value === 'starting') return 'bg-warning'
  if (value === 'error') return 'bg-destructive'
  return 'bg-muted-foreground'
}

function ValidatorIdentityHeader({ node }: { node: PublicNode }) {
  return <header className="mt-2 flex min-w-0 flex-col gap-3 md:flex-row md:items-start md:justify-between" aria-labelledby="node-detail-title">
    <div data-slot="node-identity-main" className="flex min-w-0 flex-1 items-start gap-2">
      <NodeHealthMarker health={node.health} />
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <h1 id="node-detail-title" className="m-0 min-w-0 max-w-full break-words text-lg font-semibold leading-tight md:text-2xl">{nodeDisplayName(node)}</h1>
          <span className="inline-flex min-w-0"><ValidatorActivityBadge validator={node.validator} identityReason={node.validatorIdentityReason} /></span>
        </div>
        <p className="m-0 mt-1 break-words text-[11px] text-muted-foreground">Node ID <code className="font-mono">{node.nodeId}</code></p>
      </div>
    </div>
    <dl className="m-0 flex flex-wrap gap-x-5 gap-y-2" aria-label="Node identity facts">
      <div className="min-w-0">
        <dt className="text-xs font-medium tracking-wider text-muted-foreground">Last report</dt>
        <dd className="m-0 text-sm tabular-nums"><LastReportAge timestamp={node.lastReportAt} /></dd>
      </div>
    </dl>
  </header>
}

function CompactNodeHeader({ node }: { node: PublicNode }) {
  const syncStateLabel = nodeSyncStateLabel(node)
  return <header className="mt-2 flex min-w-0 flex-col gap-1" aria-labelledby="node-detail-title">
    <div data-slot="node-identity-main" className="flex min-w-0 flex-1 items-start gap-2">
      <NodeHealthMarker health={node.health} />
      <div className="min-w-0">
        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <h1 id="node-detail-title" className="m-0 min-w-0 max-w-full break-words text-lg font-semibold leading-tight md:text-2xl">{nodeDisplayName(node)}</h1>
          {syncStateLabel != null && <span className="text-sm font-medium text-muted-foreground">{syncStateLabel}</span>}
          {syncStateLabel != null && <span aria-hidden="true" className="text-muted-foreground">·</span>}
          <span className="text-xs text-muted-foreground">{nodeRoleLabel(node)}</span>
          <span aria-hidden="true" className="text-muted-foreground">·</span>
          <LastReportAge timestamp={node.lastReportAt} variant="inline" />
        </div>
        <NodeIdLine nodeId={node.nodeId} />
      </div>
    </div>
  </header>
}

function NodeIdLine({ nodeId }: { nodeId: string }) {
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(nodeId)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard access can be denied; the full identifier stays selectable.
    }
  }
  return <p className="m-0 mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-muted-foreground">
    <span>Node ID</span>
    <code className="break-all font-mono">{nodeId}</code>
    <DataTooltip as="span" content="Copy full Node ID">
      <Button variant="ghost" size="icon-sm" aria-label="Copy full Node ID" className="text-muted-foreground" onClick={() => { void copy() }}>
        {copied ? <Check className="size-3.5" aria-hidden="true" /> : <Copy className="size-3.5" aria-hidden="true" />}
      </Button>
    </DataTooltip>
  </p>
}

function ChainStateGroup({ node }: { node: PublicNode }) {
  const consensus = node.consensus
  const qcValue = consensus?.highestQcBlock
  const lockedValue = consensus?.highestLockBlock
  const committedValue = consensus?.highestCommitBlock
  const observedValue = node.currentHead
  const networkValue = node.networkReferenceHead
  const qc = validHeight(qcValue) ? qcValue : null
  const locked = validHeight(lockedValue) ? lockedValue : null
  const committed = validHeight(committedValue) ? committedValue : null
  const observed = validHeight(observedValue) ? observedValue : null
  const network = validHeight(networkValue) ? networkValue : null
  const lag = headLagPresentation(node)
  const hasRows = qc != null || locked != null || committed != null || observed != null || network != null || lag != null
  const staleParts = [
    typeof consensus?.freshness === 'string' && consensus.freshness !== 'current' ? 'freshness ' + consensus.freshness : null,
    typeof consensus?.state === 'string' && consensus.state !== 'ok' ? 'collection ' + consensus.state : null,
  ].filter((part): part is string => part != null)
  return <NodeInfoGroup title="Chain state" label="Node chain state">
    {qc != null && <MetricRow label="QC Head" value={qc.toLocaleString()} />}
    {locked != null && <MetricRow label="Locked Head" value={locked.toLocaleString()} />}
    {committed != null && <MetricRow label="Committed Head" value={committed.toLocaleString()} />}
    {observed != null && <MetricRow label="Observed Head" value={observed.toLocaleString()} />}
    {network != null && <MetricRow label="Network Head" value={network.toLocaleString()} />}
    {lag != null && <MetricRow label="Head lag" value={<span className={lag.toneClass}>{lag.label}</span>} detail={lag.detail} />}
    {!hasRows && <p className="m-0 text-[11px] text-muted-foreground">Chain state has not been observed yet.</p>}
    {staleParts.length > 0 && <p data-slot="chain-state-stale" className="m-0 text-[11px] text-muted-foreground">Last-good consensus observation · {staleParts.join(' · ')}</p>}
  </NodeInfoGroup>
}

function ProcessGroup({ node, nodeDataProgressValue }: { node: PublicNode; nodeDataProgressValue: number | null }) {
  const bytes = formatNodeDataBytes(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const uptime = formatDuration(node.processUptimeMs)
  return <NodeInfoGroup title="Process" label="PlatON process">
    <MetricRow
      label="State"
      value={<span className="inline-flex items-center gap-1.5"><span className={cn('inline-block size-2 shrink-0 rounded-full', processStateDot(node.processState))} aria-hidden="true" />{nodeComponentStateLabel(node.processState)}</span>}
    />
    {uptime !== 'Unknown' && <MetricRow label="Started" value={uptime + ' ago'} detail={node.processStartedAt != null ? formatUtcDateTime(node.processStartedAt) : undefined} />}
    <div className="mt-1 grid grid-cols-1 gap-2 border-t border-dashed border-border/60 pt-2" role="group" aria-label="Node data directory">
      {bytes != null && <MetricRow label="Storage" value={bytes} />}
      <MetricRow
        label="Data usage"
        value={formatPercent(nodeDataProgressValue)}
        detail={(bytes ?? 'Unknown') + ' · directory size against the hosting filesystem capacity, not whole-Host disk usage'}
        progress={nodeDataProgressValue}
      />
    </div>
  </NodeInfoGroup>
}

function HostResourcesGroup({ node }: { node: PublicNode }) {
  return <NodeInfoGroup title="Host resources" label="Shared Host resources" note="Collected once per Agent; shared by every Node it monitors">
    <MetricRow label="CPU" value={formatPercent(node.hostCpuPercent)} progress={node.hostCpuPercent} />
    <MetricRow label="Memory" value={formatPercent(node.hostMemoryPercent)} progress={node.hostMemoryPercent} />
    <MetricRow label="Storage" value={formatPercent(node.hostStoragePercent)} progress={node.hostStoragePercent} />
    <MetricRow
      label="Network"
      value={<span className="inline-flex flex-wrap items-center justify-end gap-x-2 tabular-nums"><span>↑ {formatRate(node.hostNetworkTxBytesPerSec)}</span><span>↓ {formatRate(node.hostNetworkRxBytesPerSec)}</span></span>}
    />
  </NodeInfoGroup>
}

type MetricSeries = {
  label: string
  points: PublicMetricPoint[]
  secondary?: boolean
}

type MetricChartKind = 'line' | 'bar'

type MetricTone = 'blue' | 'cyan' | 'violet' | 'amber'

const TONE_TEXT: Record<MetricTone, string> = {
  blue: 'text-blue-600 dark:text-blue-400',
  cyan: 'text-cyan-600 dark:text-cyan-400',
  violet: 'text-violet-600 dark:text-violet-400',
  amber: 'text-amber-500 dark:text-amber-400',
}

type MetricChartProps = {
  label: string
  series: MetricSeries[]
  from?: string
  to?: string
  fixedMax?: number
  axisFormat: (value: number) => string
  message?: string
  kind?: MetricChartKind
  windowSeconds?: number
  toneClass: string
  legend?: ReactNode
}

function NodeMetricCard({ label, unit, value, valueLabel, detail, tone, series, showLegend = false, from, to, fixedMax, axisFormat, historyMessage, chartKind = 'line', windowSeconds, className = '' }: {
  label: string
  unit?: string
  value: string
  valueLabel?: string
  detail?: string
  tone: MetricTone
  series: MetricSeries[]
  showLegend?: boolean
  from?: string
  to?: string
  fixedMax?: number
  axisFormat: (value: number) => string
  historyMessage?: string
  chartKind?: MetricChartKind
  windowSeconds?: number
  className?: string
}) {
  // A direction series that has no retained sample must not disappear behind
  // its available partner: name it explicitly while the partner keeps plotting.
  const missingDirections = series.length > 1 && series.some((item) => item.points.length > 0)
    ? series.filter((item) => item.points.length === 0).map((item) => item.label)
    : []
  const toneClass = TONE_TEXT[tone]
  // Multi-direction cards explain their curves under the plot, never under the
  // title, so the header stays title-left / current-value-right on every card.
  const legend = showLegend ? <MetricSeriesLegend label={label} series={series} toneClass={toneClass} /> : undefined
  return (
    <CardX bordered={false} role="article" data-slot="node-metric-card" className={cn('min-w-0', CARD, className)} contentClassName="flex h-full min-w-0 flex-col gap-2">
      <div data-slot="node-metric-header" className="flex min-w-0 items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="m-0 text-sm font-medium">{label}</h3>
          {unit && <p className="m-0 mt-0.5 text-[11px] leading-4 text-muted-foreground">{unit}</p>}
        </div>
        <div className="flex shrink-0 flex-col items-end gap-0.5">
          <strong data-slot="node-metric-value" className="text-right text-xl font-bold leading-none tracking-tight tabular-nums">{value}</strong>
          {valueLabel && <span className="text-[10px] leading-none text-muted-foreground">{valueLabel}</span>}
        </div>
      </div>
      {detail && <p className="m-0 break-words text-[11px] leading-4 text-muted-foreground">{detail}</p>}
      {missingDirections.length > 0 && <p className="m-0 text-[11px] italic text-muted-foreground">{missingDirections.join(' and ')} unavailable in this window</p>}
      <MetricChart label={label} series={series} from={from} to={to} fixedMax={fixedMax} axisFormat={axisFormat} message={historyMessage} kind={chartKind} windowSeconds={windowSeconds} toneClass={toneClass} legend={legend} />
    </CardX>
  )
}

function MetricSeriesLegend({ label, series, toneClass }: { label: string; series: MetricSeries[]; toneClass: string }) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1" aria-label={label + ' chart legend'}>
      {series.map((item) => (
        <span key={item.label} className="inline-flex min-w-0 items-center gap-1.5 text-[10px] text-muted-foreground">
          <i className={cn('inline-block size-2 shrink-0 rounded-full', item.secondary ? 'bg-cyan-500' : cn('bg-current', toneClass))} aria-hidden="true" />
          {item.label}
        </span>
      ))}
    </div>
  )
}

function MetricChart({ label, series, from, to, fixedMax, axisFormat, message, kind = 'line', windowSeconds, toneClass, legend }: MetricChartProps) {
  const seconds = Number.isFinite(windowSeconds) && (windowSeconds ?? 0) > 0 ? Math.round(windowSeconds as number) : 60
  const gradientId = 'node-metric-fill-' + useId().replaceAll(':', '')
  const fromMs = from ? Date.parse(from) : Number.NaN
  const toMs = to ? Date.parse(to) : Number.NaN
  const validWindow = Number.isFinite(fromMs) && Number.isFinite(toMs) && toMs > fromMs
  const values = series.flatMap((item) => item.points.map((point) => point.value)).filter(Number.isFinite)
  const max = fixedMax ?? niceChartMax(values.length > 0 ? Math.max(...values) : 0)
  const plots = validWindow
    ? series.map((item) => ({
        ...item,
        coordinates: kind === 'bar'
          ? chartBarCoordinates(item.points, fromMs, toMs, max)
          : chartCoordinates(item.points, fromMs, toMs, max),
      }))
    : []
  const hasPoints = plots.some((item) => item.coordinates.length > 0)
  const chartMessage = message ?? (hasPoints ? undefined : 'No samples in the last minute')
  const windowLabel = 'over the last ' + seconds + ' seconds'

  return <div className="mt-auto grid min-w-0 grid-cols-[3rem_minmax(0,1fr)] grid-rows-[7.25rem_auto_auto] lg:grid-rows-[6.25rem_auto_auto] gap-x-2">
    <div className="flex flex-col justify-between pr-1 text-right text-[11px] tabular-nums text-muted-foreground" aria-hidden="true">
      <span>{axisFormat(max)}</span>
      <span>{axisFormat(max / 2)}</span>
      <span>{axisFormat(0)}</span>
    </div>
    <svg viewBox="0 0 600 150" preserveAspectRatio="none" role="img" aria-label={label + ' ' + kind + ' chart ' + windowLabel} className={cn('col-start-2 row-start-1 h-[7.25rem] lg:h-[6.25rem] w-full overflow-visible', toneClass)}>
      <title>{label} {kind} chart {windowLabel}</title>
      <desc>{chartMessage ? label + ': ' + chartMessage : series.map((item) => item.label).join(' and ') + ' values from ' + seconds + ' seconds ago to now'}</desc>
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="currentColor" stopOpacity="0.34" />
          <stop offset="100%" stopColor="currentColor" stopOpacity="0.02" />
        </linearGradient>
      </defs>
      <g aria-hidden="true">
        <line x1="0" y1="8" x2="600" y2="8" className="stroke-border [vector-effect:non-scaling-stroke]" />
        <line x1="0" y1="75" x2="600" y2="75" className="stroke-border [vector-effect:non-scaling-stroke]" />
        <line x1="0" y1="142" x2="600" y2="142" className="stroke-border [vector-effect:non-scaling-stroke]" />
      </g>
      {!chartMessage && kind === 'line' && plots.map((item, index) => {
        const line = chartLinePath(item.coordinates)
        const area = plots.length === 1 ? chartAreaPath(item.coordinates) : ''
        return <g key={item.label}>
          {area && <path d={area} fill={'url(#' + gradientId + ')'} />}
          {line && <path data-slot="node-metric-chart-line" className={cn('[vector-effect:non-scaling-stroke]', item.secondary ? 'fill-none stroke-cyan-500' : 'fill-none stroke-current')} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" d={line} />}
          {item.coordinates.length === 1 && <circle className={cn('[vector-effect:non-scaling-stroke]', item.secondary ? 'fill-cyan-500 stroke-background' : 'fill-current stroke-background')} strokeWidth={1.5} cx={item.coordinates[0].x} cy={item.coordinates[0].y} r={index === 0 ? 4 : 3.5} />}
        </g>
      })}
      {!chartMessage && kind === 'bar' && plots.flatMap((item) => {
        const width = chartBarWidth(item.coordinates.length)
        return item.coordinates.map((point, index) => {
          const height = Math.max(1, 142 - point.y)
          const x = Math.max(0, Math.min(600 - width, point.x - width / 2))
          return <rect key={item.label + '-' + index} data-slot="node-metric-chart-bar" className={cn('opacity-75', item.secondary ? 'fill-cyan-500' : 'fill-current')} x={x} y={142 - height} width={width} height={height} rx={Math.min(2.5, width / 3)} />
        })
      })}
      {chartMessage && <text data-slot="node-metric-chart-empty" className="fill-muted-foreground text-[22px]" x="300" y="78" textAnchor="middle">{chartMessage}</text>}
    </svg>
    <div className="col-start-2 row-start-2 flex justify-between pt-1 text-[11px] tabular-nums text-muted-foreground" aria-hidden="true"><span>{seconds}s</span><span>0s</span></div>
    {legend && <div data-slot="node-metric-legend" className="col-start-2 row-start-3 pt-1.5">{legend}</div>}
  </div>
}

type ChartCoordinate = { x: number; y: number }

function chartCoordinates(points: PublicMetricPoint[], from: number, to: number, max: number): ChartCoordinate[] {
  const coordinates = points
    .map((point) => ({ sampledAt: Date.parse(point.sampledAt), value: point.value }))
    .filter((point) => Number.isFinite(point.sampledAt) && Number.isFinite(point.value))
    .sort((left, right) => left.sampledAt - right.sampledAt)
    .map((point) => ({
      x: Math.max(0, Math.min(600, ((point.sampledAt - from) / (to - from)) * 600)),
      y: 142 - (Math.max(0, Math.min(max, point.value)) / max) * 134,
    }))
  const last = coordinates.at(-1)
  if (last && last.x < 600) coordinates.push({ x: 600, y: last.y })
  return coordinates
}

function chartBarCoordinates(points: PublicMetricPoint[], from: number, to: number, max: number): ChartCoordinate[] {
  return points
    .map((point) => ({ sampledAt: Date.parse(point.sampledAt), value: point.value }))
    .filter((point) => Number.isFinite(point.sampledAt) && Number.isFinite(point.value) && point.sampledAt >= from && point.sampledAt <= to)
    .sort((left, right) => left.sampledAt - right.sampledAt)
    .map((point) => ({
      x: ((point.sampledAt - from) / (to - from)) * 600,
      y: 142 - (Math.max(0, Math.min(max, point.value)) / max) * 134,
    }))
}

function chartBarWidth(count: number): number {
  if (count <= 0) return 0
  return Math.max(2, Math.min(18, (600 / count) * 0.64))
}

function chartLinePath(points: ChartCoordinate[]): string {
  return points.map((point, index) => (index === 0 ? 'M' : 'L') + ' ' + point.x.toFixed(2) + ' ' + point.y.toFixed(2)).join(' ')
}

function chartAreaPath(points: ChartCoordinate[]): string {
  if (points.length < 2) return ''
  const first = points[0]
  const last = points.at(-1)
  if (!last) return ''
  return chartLinePath(points) + ' L ' + last.x.toFixed(2) + ' 142 L ' + first.x.toFixed(2) + ' 142 Z'
}

function niceChartMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  const step = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10
  return step * magnitude
}

function formatValidatorMembership(node: PublicNode): string {
  const consensus = node.consensus
  if (!consensus || consensus.validator == null || consensus.freshness === 'unknown' || ['starting', 'disabled', 'unsupported'].includes(consensus.state)) return 'Unknown'
  return consensus.validator ? 'True' : 'False'
}

function formatRate(value: number | null | undefined): string {
  if (value == null) return 'Unknown'
  const units = ['B/s', 'KiB/s', 'MiB/s', 'GiB/s']
  let scaled = value
  let unit = 0
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024
    unit += 1
  }
  const digits = scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2
  return scaled.toFixed(digits) + ' ' + units[unit]
}

function formatCountAxis(value: number): string {
  return Math.round(value).toLocaleString()
}

function formatMillisecondsAxis(value: number): string {
  if (value >= 1000) return Number((value / 1000).toFixed(value >= 10_000 ? 0 : 1)) + 's'
  return Math.round(value) + 'ms'
}

/** The fixed public metrics response owns the window; the section label reads the
 *  real windowSeconds instead of inventing a range. The retained-sample explanation
 *  that used to sit opposite the label is deliberately gone. */
function describeMetricWindow(history: PublicNodeMetricHistory | undefined): string {
  if (!history) return 'Latest 60 seconds'
  const seconds = Number.isFinite(history.windowSeconds) && history.windowSeconds > 0 ? history.windowSeconds : 60
  return 'Latest ' + seconds + ' seconds'
}

function formatPercentAxis(value: number): string {
  if (value >= 100) return '100%'
  if (value <= 0) return '0%'
  return value.toFixed(value < 10 ? 1 : 0) + '%'
}

function latestBlockInterval(history: ReturnType<typeof usePublicNodeHistory>['data']): { value: string; detail: string } {
  const blocks = history?.filter((item) => item.height != null && item.blockTimeMs != null).slice(0, 2) ?? []
  if (blocks.length < 2) return { value: 'Unknown', detail: 'Two Block Summaries are required' }
  const [latest, previous] = blocks
  if (latest.height == null || previous.height == null || latest.blockTimeMs == null || previous.blockTimeMs == null || latest.height - previous.height !== 1) {
    return { value: 'Unknown', detail: 'Consecutive Block Summaries unavailable' }
  }
  const elapsed = latest.blockTimeMs - previous.blockTimeMs
  if (elapsed < 0) return { value: 'Unknown', detail: 'Block timestamps are inconsistent' }
  const value = elapsed < 1000 ? elapsed + ' ms' : (elapsed / 1000).toFixed(2) + ' s'
  return { value, detail: 'Block ' + latest.height.toLocaleString() + ' − ' + previous.height.toLocaleString() }
}

/** A process value is the retained current Public Projection value; when the
 *  process component is not Current it is explicitly marked as last-good. */
function processStatusDetail(state: string | null | undefined): string | undefined {
  const label = nodeComponentStateLabel(state)
  if (label === 'Current') return undefined
  return 'last-good value retained · collection ' + label
}

function formatResyncDetail(node: PublicNode): string {
  if (node.resyncProgress) return node.resyncProgress
  return node.resyncState === 'normal' ? 'No resync in progress' : 'Resync progress is Unknown'
}

function formatReferenceHead(node: PublicNode): string {
  return node.networkReferenceHead == null ? 'Unknown' : node.networkReferenceHead.toLocaleString()
}

function formatReferenceDetail(node: PublicNode): string {
  if (node.networkReferenceHead == null) return 'Observed Network Head unavailable'
  return 'Observed Network Head · ' + (node.networkReferenceConfidence || 'unknown') + ' confidence'
}

function nodeComponentStateLabel(value: string | null | undefined): string {
  const state = typeof value === 'string' ? value.trim().toLowerCase() : undefined
  switch (state) {
    case 'ok': return 'Current'
    case 'starting': return 'Starting'
    case 'error': return 'Error'
    case 'disabled': return 'Disabled'
    case 'unsupported': return 'Unsupported'
    default: return 'Unknown'
  }
}

type NodeHealthPresentation = {
  label: 'Healthy' | 'Unhealthy' | 'Unknown'
  tone: 'ok' | 'warning' | 'error' | 'neutral'
}

function nodeHealthPresentation(value: string | null | undefined): NodeHealthPresentation {
  const health = typeof value === 'string' ? value.trim().toLowerCase() : undefined
  switch (health) {
    case 'healthy': return { label: 'Healthy', tone: 'ok' }
    case 'unhealthy': return { label: 'Unhealthy', tone: 'error' }
    default: return { label: 'Unknown', tone: 'neutral' }
  }
}

function nodeDisplayName(node: Pick<PublicNode, 'displayName' | 'nodeId'>): string {
  const displayName = typeof node.displayName === 'string' ? node.displayName.trim() : ''
  return displayName || node.nodeId
}

function formatNumber(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function formatPercent(value: number | null | undefined): string {
  return value == null ? 'Unknown' : value.toFixed(1) + '%'
}

function peerCount(insight: PublicNode['peers']): string {
  return formatNumber(insight.peerCount)
}

function peerBreakdown(insight: PublicNode['peers']): string {
  const collection = peerInsightCollectionStatus(insight)
  const freshness = peerInsightFreshnessStatus(insight)
  const value = peerInsightValueStatus(insight)
  const hasValue = value !== 'Unknown'
  const directionSummary = formatNumber(insight.inboundCount) + ' inbound · ' + formatNumber(insight.outboundCount) + ' outbound'
  const qualifiers: string[] = []

  if (collection === 'Error') qualifiers.push('Collection failed')
  else if (collection !== 'Current') qualifiers.push('Collection ' + collection.toLowerCase())
  if (freshness === 'Stale') qualifiers.push('Stale')
  if (freshness !== 'Current' && freshness !== 'Stale') qualifiers.push('Freshness unknown')

  if (!hasValue) {
    if (qualifiers.length === 0) qualifiers.push('Value unknown')
    qualifiers.push('No successful Peer snapshot is available')
    qualifiers.push(directionSummary)
    return qualifiers.join('; ')
  }

  if (qualifiers.length > 0) {
    qualifiers.push('Showing last successful snapshot')
    if (value === 'Empty') qualifiers.push('authoritative zero')
    qualifiers.push(directionSummary)
    return qualifiers.join('; ')
  }

  if (value === 'Empty') return 'Empty; authoritative successful zero; ' + directionSummary
  return directionSummary
}
