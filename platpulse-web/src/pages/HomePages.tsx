import { ArrowLeft, Check, ChevronDown, ChevronUp, Copy } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { Link, useLocation, useParams, useSearchParams } from 'react-router'
import {
  usePublicNode,
  usePublicNodeHistory,
  usePublicNodeMetrics,
} from '../api/public'
import type { PublicMetricPoint, PublicMetricSeriesCoverage, PublicNode, PublicNodeMetricHistory } from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import { homeReturnHref, homeReturnTab, readHomeReturn } from '../homeReturn'
import { peerInsightCollectionStatus, peerInsightFreshnessStatus, peerInsightValueStatus } from '../components/PeerInsight'
import { formatUtcDateTime, NodeHealthMarker } from '../components/StatusBadge'
import { LinkedValidatorDiagnostics, LinkedValidatorSection } from '../components/LinkedValidator'
import GeoMapBoundary from '../components/GeoMapBoundary'
import GeoWorldMap from '../components/GeoWorldMap'
import { RealtimeNotice } from '../components/RealtimeNotice'
import { formatNodeDataBytes } from '../formatBytes'
import { formatDuration } from '../formatDuration'
import { nodeDataProgress } from '../nodeData'
import { coverageNotes, coverageSegments, metricCoverage } from '../metricCoverage'
import NodeMetricChart from '../components/NodeMetricChart'
import { MetricWindowSelector } from '../components/MetricWindowSelector'
import {
  METRIC_WINDOW_PARAM,
  metricAxisTimeFormat,
  metricWindowSearch,
  metricWindowText,
  readMetricWindow,
  type MetricWindow,
  type MetricWindowText,
} from '../metricWindow'
import { METRIC_SERIES_COLORS, type MetricChartSeriesInput, type MetricCountNoun, type MetricSeriesKey } from '../components/nodeMetricChartOption'
import {
  countAxis,
  durationAxis,
  formatCountSample,
  formatMetricRate,
  formatMillisecondsSample,
  formatPercentSample,
  formatRateSample,
  percentAxis,
  rateAxis,
  type MetricAxisRule,
} from '../components/metricChartAxis'
import { MetricRow } from '../components/MetricRow'
import { InfoHint } from '../components/InfoHint'
import { ValidatorActivityBadge } from '../components/ValidatorActivityBadge'
import { HEAD_COMPARISON_TONE_CLASS, LastReportAge, networkHeadComparison, networkHeadOffset, validHeight } from '../components/NodeDetailObservations'
import { SummaryMetricCard } from '../components/SummaryMetricCard'
import { OverviewBand } from '../components/OverviewBand'
import { geoMapStatus, nodeGeoOverview } from '../homeGeo'
import { NODE_PEER_COUNTRIES_HEADING } from '../components/geoPresentation'
import { CardX } from '../components/ui/card-x'
import { Alert, AlertDescription } from '../components/ui/alert'
import { Spinner } from '../components/ui/spinner'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { DataTooltip } from '../components/ui/data-tooltip'
import { Button } from '../components/ui/button'
import { cn } from '../lib/utils'

/** The six value axes. Each title carries the unit and each tick is a bare
 *  number, like Emerald's "CPU %" over "0 · 30 · 60". Module-level: a rule's
 *  identity has to survive a render or the chart would rebuild its option. */
const CPU_AXIS = percentAxis('CPU')
const MEMORY_AXIS = percentAxis('Memory')
const NETWORK_AXIS = rateAxis('Speed')
const PEER_AXIS = countAxis('Connections')
const BLOCK_INTERVAL_AXIS = durationAxis('Duration')
const TRANSACTION_AXIS = countAxis('Txs')
// What a block-tier bucket counts (#248): a Block interval bucket counts the
// intervals it saw, a Transactions / block bucket the blocks.
const INTERVAL_NOUN: MetricCountNoun = ['interval', 'intervals']
const BLOCK_NOUN: MetricCountNoun = ['block', 'blocks']

const PAGE = 'min-w-0 p-4'
// Info/chart tier; the summary tiles sit one step brighter, the disclosures one step dimmer.
const CARD = cn('min-w-0 rounded-md border-none', SURFACE_CARD_STATIC)

export function NodePage() {
  const { nodeId = '' } = useParams()
  const { generation, resetting, realtime } = useHomeRealtimeContext()
  const nodeQuery = usePublicNode(nodeId, generation)
  const historyQuery = usePublicNodeHistory(nodeId, generation)
  // The Metric Window lives in the URL (webui.md §15.38): refresh, back/forward and a
  // shared link restore it. Live writes no parameter, and a value outside the
  // enum falls back to Live and is rewritten in place, adding no history entry.
  const [search, setSearch] = useSearchParams()
  const { window: metricWindow, canonical: metricWindowCanonical } = readMetricWindow(search.get(METRIC_WINDOW_PARAM))
  const metricsQuery = usePublicNodeMetrics(nodeId, generation, metricWindow)
  // The same-tab return (#224): Home's Node links record where the reader was
  // reading, and every way back hands that departure on again — rebuilt into an
  // ordinary Home URL through Home's own filter vocabulary, never navigated to
  // verbatim, so nothing a Node detail page could carry becomes a redirect.
  const location = useLocation()
  const departure = readHomeReturn(location.state, homeReturnTab())
  const returnState = departure === null ? undefined : { homeReturn: departure }
  useEffect(() => {
    if (metricWindowCanonical) return
    // The departure rides the history entry's state, so the rewrite keeps it.
    setSearch(metricWindowSearch(search, 'live'), { replace: true, state: location.state })
  }, [metricWindowCanonical, search, setSearch, location.state])
  const selectMetricWindow = (next: MetricWindow) => {
    setSearch(metricWindowSearch(search, next), { state: location.state })
  }

  if (resetting) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Revalidating Node access…</p></section>
  if (nodeQuery.isPending) return <section className={cn(PAGE, 'relative min-h-32')}><RealtimeNotice realtime={realtime} /><Spinner label="Loading Node"><span className="text-sm text-muted-foreground">Loading Node…</span></Spinner></section>
  if (nodeQuery.error && !nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><Alert variant="destructive" className="mt-3 rounded-md border-none bg-red-400/10"><AlertDescription>{nodeQuery.error instanceof Error ? nodeQuery.error.message : 'Unable to load Node'}</AlertDescription></Alert><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to={homeReturnHref(departure)} state={returnState}>Back to Home</Link></section>
  if (!nodeQuery.data) return <section className={PAGE}><RealtimeNotice realtime={realtime} /><p role="status" className="mt-3 text-sm text-muted-foreground">Node unavailable.</p><Link className="mt-3 inline-flex min-h-11 min-w-11 items-center text-sm font-medium text-emerald-600 hover:underline dark:text-emerald-400" to={homeReturnHref(departure)} state={returnState}>Back to Home</Link></section>

  const node = nodeQuery.data
  // The Node Peer Country View is a projection of exactly this Node. The Node
  // response has already loaded, so the map slot is never Starting or
  // Unavailable here; its status is the Server's own Geo state.
  const nodeGeo = nodeGeoOverview(node)
  const nodeGeoStatus = geoMapStatus(nodeGeo, { loading: false, hasProjection: true })
  const health = nodeHealthPresentation(node.health)
  // The three queries fail independently: a failed history refresh is stated
  // beside the history it concerns, never as a failure of the whole page.
  const blockInterval = historyQuery.data
    ? latestBlockInterval(historyQuery.data)
    : { value: 'Unknown', detail: historyQuery.isPending ? 'Loading block history…' : 'Block history unavailable' }
  const blockIntervalDetail = historyQuery.isRefetchError ? blockInterval.detail + ' · Block history update failed' : blockInterval.detail
  // An answer is drawn only for the window it answers: a newly selected window
  // never shows the previous window's data, it shows its own loading state.
  const metricHistory = metricsQuery.data && (metricsQuery.data.metricWindow?.window ?? 'live') === metricWindow
    ? metricsQuery.data
    : undefined
  const metricText = metricWindowText(metricWindow, metricWindowSeconds(metricHistory))
  // A background refresh failed but the last successful window is still held:
  // it keeps its own from/to and samples, and the page says it is retained
  // rather than letting it pass for the latest window.
  const metricHistoryRetained = metricsQuery.isRefetchError && metricHistory != null
  const metricHistoryMessage = metricHistory
    ? undefined
    : metricsQuery.isPending
      ? metricText.loading
      : metricsQuery.error
        ? 'Metric history unavailable'
        : undefined
  // The shared helper validates both byte counts, so the hero reads the one ratio
  // the Home card's Disk column and the Home Disk sort read (#242) instead of
  // guarding the same two fields a second time.
  const nodeDataProgressValue = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const metricWindowTitle = metricHistoryRetained ? 'Retained ' + metricText.adjective + ' window' : metricText.title
  const validatorNode = nodeUsesValidatorComposition(node)
  // Live keeps the deck exactly as it was; a longer window brings its own axis
  // labels and words to all six charts, the two block charts included (#248).
  const longWindow = metricWindow !== 'live'
  const windowText = longWindow ? metricText : undefined

  return <section className={PAGE}>
    <div data-slot="node-detail-breadcrumb" className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-medium text-muted-foreground">
        <Link className="inline-flex min-h-11 min-w-11 items-center hover:text-foreground" to={homeReturnHref(departure)} state={returnState}><ArrowLeft size={16} aria-hidden="true" />Back to Home</Link>
        <span aria-hidden="true">/</span>
        <span>Node detail</span>
      </div>
      <RealtimeNotice realtime={realtime} />
    </div>
    {nodeQuery.isRefetchError && <p role="status" className="mt-2 text-sm text-destructive">Node refresh failed; showing the last successful Node data.</p>}

    <NodeIdentityHeader node={node} />

    {health.tone !== 'ok' && <p className="m-0 mt-2 break-words text-sm text-warning-foreground dark:text-warning">{node.healthReason}</p>}

    {/* Both roles share geometry and DOM order; only performance is conditional.
        The band is Home's own OverviewBand, so card size, the 5:6 column split,
        the map track and the below-lg map-first order cannot drift between the
        two pages. */}
    <OverviewBand
      className="mt-4"
      dataSlot="node-overview"
      mapSlot="node-map"
      mapFirst
      metricsLabel="Node key summary"
      metrics={<>
        <SummaryTile label="Head" value={formatNumber(node.currentHead)} detail={headLagDetail(node)} />
        <SummaryTile label="Sync collection" value={nodeComponentStateLabel(node.syncState)} detail={syncStateDetail(node)} action={<InfoHint inline={false} align="end" label="About Sync collection">
          <p className="m-0">The state of this Node's latest sync-data collection: Current, Error, Unknown and so on.</p>
          <p className="m-0">It is not a chain-sync verdict. Current means the data was collected, not that the Node is fully synced; the Head tile carries the head lag.</p>
        </InfoHint>} />
        <SummaryTile label="Peers" value={peerCount(node.peers)} detail={peerDirectionSummary(node.peers)} />
        <SummaryTile label="Uptime" value={formatDuration(node.processUptimeMs)} />
        <SummaryTile label="Block interval" value={blockInterval.value} detail={blockInterval.value === 'Unknown' ? blockInterval.detail : undefined} />
        <SummaryTile label="Transactions / block" value={formatNumber(node.latestBlockTransactionCount)} />
      </>}
      map={<GeoMapBoundary label={NODE_PEER_COUNTRIES_HEADING}>
        <GeoWorldMap overview={nodeGeo} status={nodeGeoStatus} heading={NODE_PEER_COUNTRIES_HEADING} />
      </GeoMapBoundary>}
    />

    <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-3">
      <ChainStateGroup node={node} />
      <ProcessGroup node={node} nodeDataProgressValue={nodeDataProgressValue} />
      <HostResourcesGroup node={node} />
    </div>

    {validatorNode && <LinkedValidatorSection node={node} variant="detail" />}

    <section data-slot="node-metrics-section" className="mt-4" aria-labelledby="node-metrics-title">
      {/* The window selector leads the deck on the left; the selected segment
          already names the window, so the title is spoken but not printed. On
          phones the selector takes its own full-width row. */}
      <header data-slot="node-metrics-header" className="mb-2 flex min-w-0 flex-col gap-y-1 md:flex-row md:items-center md:gap-x-3">
        <h2 id="node-metrics-title" className="sr-only">{metricWindowTitle}</h2>
        <MetricWindowSelector value={metricWindow} onChange={selectMetricWindow} className="flex w-full md:w-auto md:shrink-0" />
        {metricHistoryRetained && metricHistory && <div className="flex min-w-0 items-center">
          <span role="status" data-slot="node-metrics-refresh-error" className="text-xs text-warning-foreground dark:text-warning">History update failed · Showing previous window</span>
          <InfoHint label="Retained window times">
            <p className="m-0">The charts show the last window that loaded successfully, not the {metricText.title.toLowerCase()}.</p>
            <p className="m-0" data-slot="node-metrics-retained-window">From {formatUtcDateTime(metricHistory.from)} to {formatUtcDateTime(metricHistory.to)}</p>
          </InfoHint>
        </div>}
      </header>
      {/* Keyed by Node: legend choices belong to one Node's charts and never
          travel to the next Node opened on the same route. */}
      <div key={nodeId} className="grid auto-rows-fr grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
        <NodeMetricCard
          label="Process CPU"
          value={formatPercent(node.processCpuPercent)}
          detail={processStatusDetail(node.processState)}
          series={[{ key: 'cpu', label: 'Process CPU', points: metricHistory?.processCpuPercent ?? [], coverage: metricCoverage(metricHistory, 'processCpuPercent'), area: true }]}
          history={metricHistory}
          axis={CPU_AXIS}
          valueFormat={formatPercentSample}
          historyMessage={metricHistoryMessage}
          retained={metricHistoryRetained}
          windowText={windowText}
        />
        <NodeMetricCard
          label="Process memory"
          value={formatPercent(node.processMemoryPercent)}
          detail={processStatusDetail(node.processState)}
          series={[{ key: 'memory', label: 'Process memory', points: metricHistory?.processMemoryPercent ?? [], coverage: metricCoverage(metricHistory, 'processMemoryPercent'), area: true }]}
          history={metricHistory}
          axis={MEMORY_AXIS}
          valueFormat={formatPercentSample}
          historyMessage={metricHistoryMessage}
          retained={metricHistoryRetained}
          windowText={windowText}
        />
        <NodeMetricCard
          label="Host network"
          // Emerald's network header: both current rates on one row, upload first,
          // each behind a compact direction chevron drawn in its curve's colour.
          readings={[
            { label: 'Upload', value: formatMetricRate(node.hostNetworkTxBytesPerSec), direction: 'up', color: METRIC_SERIES_COLORS.upload },
            { label: 'Download', value: formatMetricRate(node.hostNetworkRxBytesPerSec), direction: 'down', color: METRIC_SERIES_COLORS.download },
          ]}
          series={[
            { key: 'download', label: 'Download', points: metricHistory?.networkRxBytesPerSec ?? [], coverage: metricCoverage(metricHistory, 'networkRxBytesPerSec') },
            { key: 'upload', label: 'Upload', points: metricHistory?.networkTxBytesPerSec ?? [], coverage: metricCoverage(metricHistory, 'networkTxBytesPerSec') },
          ]}
          showLegend
          history={metricHistory}
          // Both directions share one axis, scaled to the higher of the two peaks.
          axis={NETWORK_AXIS}
          valueFormat={formatRateSample}
          historyMessage={metricHistoryMessage}
          retained={metricHistoryRetained}
          windowText={windowText}
        />
        <NodeMetricCard
          label="Peer connections"
          value={peerCount(node.peers)}
          detail={peerBreakdown(node.peers)}
          series={[
            { key: 'inbound', label: 'Inbound', points: metricHistory?.peerInboundCount ?? [], coverage: metricCoverage(metricHistory, 'peerInboundCount') },
            { key: 'outbound', label: 'Outbound', points: metricHistory?.peerOutboundCount ?? [], coverage: metricCoverage(metricHistory, 'peerOutboundCount') },
          ]}
          showLegend
          history={metricHistory}
          axis={PEER_AXIS}
          valueFormat={formatCountSample}
          historyMessage={metricHistoryMessage}
          retained={metricHistoryRetained}
          windowText={windowText}
        />
        <NodeMetricCard
          label="Block interval"
          kind="bar"
          countNoun={INTERVAL_NOUN}
          value={blockInterval.value}
          detail={blockIntervalDetail}
          series={[{ key: 'blockInterval', label: 'Block interval', points: metricHistory?.blockIntervalMs ?? [], coverage: metricCoverage(metricHistory, 'blockIntervalMs') }]}
          history={metricHistory}
          axis={BLOCK_INTERVAL_AXIS}
          valueFormat={formatMillisecondsSample}
          historyMessage={metricHistoryMessage}
          retained={metricHistoryRetained}
          windowText={windowText}
        />
        <NodeMetricCard
          label="Transactions / block"
          kind="bar"
          countNoun={BLOCK_NOUN}
          value={formatNumber(node.latestBlockTransactionCount)}
          series={[{ key: 'transactions', label: 'Transactions / block', points: metricHistory?.transactionCount ?? [], coverage: metricCoverage(metricHistory, 'transactionCount') }]}
          history={metricHistory}
          axis={TRANSACTION_AXIS}
          valueFormat={formatCountSample}
          historyMessage={metricHistoryMessage}
          retained={metricHistoryRetained}
          windowText={windowText}
        />
      </div>
    </section>

    {/* Advanced diagnostics, after the deck rather than inside the performance
        card, so expanding them never hides the monitoring charts. Only a
        confirmed Validator has them at all. */}
    {validatorNode && node.validator && <LinkedValidatorDiagnostics node={node} validator={node.validator} />}

  </section>
}

/** Node Detail's six key metrics: Home's exact summary card, read-only (no
 *  hover ring) and keeping its own `node-summary-tile` slot. */
function SummaryTile({ label, value, detail, action }: { label: string; value: ReactNode; detail?: ReactNode; action?: ReactNode }) {
  return <SummaryMetricCard label={label} value={value} caption={detail} action={action} hoverable={false} dataSlot="node-summary-tile" />
}

/** `dense` tightens the row rhythm only: the card and its title line stay the
 *  shared Node Detail card, so the three panels keep one geometry. A static
 *  definition of the whole card rides `hint` beside the title instead of a
 *  permanent footer line. */
function NodeInfoGroup({ title, label, hint, dense = false, children }: { title: string; label: string; hint?: ReactNode; dense?: boolean; children: ReactNode }) {
  return (
    <CardX
      bordered={false}
      role="region"
      aria-label={label}
      data-slot="node-info-group"
      className={CARD}
      contentClassName={cn('flex h-full min-w-0 flex-col', dense ? 'gap-1' : 'gap-1.5')}
    >
      <div className="flex min-w-0 items-center">
        <h2 className="m-0 flex flex-wrap items-baseline gap-x-2 gap-y-1 text-sm font-medium leading-5">
          {title}
        </h2>
        {hint}
      </div>
      <div className={cn('grid grid-cols-1', dense ? 'gap-1' : 'gap-1.5')}>{children}</div>
    </CardX>
  )
}



function nodeUsesValidatorComposition(node: PublicNode): boolean {
  // Identity correspondence alone is not a staking verdict. Retain a last-good
  // positive verdict, but qualify its currency in the header and diagnostics.
  return node.validator?.currentValidatorStatus === 'validator'
}

function nodeRoleLabel(node: PublicNode): string | null {
  if (nodeUsesValidatorComposition(node)) return 'Validator'
  return node.validator?.currentValidatorStatus === 'not_validator'
    && node.validator.currentValidatorStatusState === 'current' ? 'Observer' : null
}

function nodeSyncStateLabel(node: PublicNode): string | null {
  const label = nodeComponentStateLabel(node.syncState)
  return label === 'Unknown' ? null : label
}

/** The Sync collection tile names the Report's sync-data collection state, and
 *  its title says so; the full definition sits behind the tile's info control
 *  instead of a permanent caption. The Head tile already carries the head lag,
 *  and repeating it here would read as a chain verdict the evidence does not
 *  support. An unnamed state keeps the comparison's own reason visible, so the
 *  tile never reads as "fully synced" by omission. */
function syncStateDetail(node: PublicNode): string | undefined {
  return nodeSyncStateLabel(node) === null ? networkHeadComparison(node).reason : undefined
}

type HeadLagPresentation = { label: string; toneClass: string }

/** Never certify a last-good or low-confidence comparison as at network head.
 *  The gate, the signed offset and the display tone are the shared ones the Home
 *  card reads too (#239); only this page's wording is local to it. */
function headLagPresentation(node: PublicNode): HeadLagPresentation | null {
  const offset = networkHeadOffset(node)
  if (offset === null) return null
  const blocks = offset.blocks
  const label = blocks === 0 ? 'At network head'
    : Math.abs(blocks).toLocaleString() + ' block' + (Math.abs(blocks) === 1 ? '' : 's') + (blocks > 0 ? ' behind' : ' ahead')
  return { label, toneClass: HEAD_COMPARISON_TONE_CLASS[offset.tone] }
}

function headLagDetail(node: PublicNode): string | undefined {
  return headLagPresentation(node)?.label ?? networkHeadComparison(node).reason
}

function peerDirectionSummary(insight: PublicNode['peers']): string {
  // Keep collection/freshness warnings even when retained counts are available.
  return peerBreakdown(insight)
}


function processStateDot(state: string | null | undefined): string {
  const value = typeof state === 'string' ? state.trim().toLowerCase() : ''
  if (value === 'ok') return 'bg-success'
  if (value === 'starting') return 'bg-warning'
  if (value === 'error') return 'bg-destructive'
  return 'bg-muted-foreground'
}

function NodeIdentityHeader({ node }: { node: PublicNode }) {
  const syncStateLabel = nodeSyncStateLabel(node)
  const role = nodeRoleLabel(node)
  const validatorNode = nodeUsesValidatorComposition(node)
  const verdictState = node.validator?.currentValidatorStatusState
  return <header className="mt-1 flex min-w-0 flex-col gap-1" aria-labelledby="node-detail-title">
    <div data-slot="node-identity-main" className="flex min-w-0 flex-1 items-start gap-2">
      <NodeHealthMarker health={node.health} />
      <div className="min-w-0">
        <h1 id="node-detail-title" className="m-0 min-w-0 max-w-full break-words text-lg font-semibold leading-tight md:text-2xl">{nodeDisplayName(node)}</h1>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-muted-foreground">
          {syncStateLabel && <><span>{syncStateLabel}</span><span aria-hidden="true">·</span></>}
          {role && <><span>{role}{validatorNode && verdictState !== 'current' ? (verdictState === 'stale' ? ' (last confirmed · stale)' : ' (currency unconfirmed)') : ''}</span><span aria-hidden="true">·</span></>}
          {validatorNode && <><ValidatorActivityBadge validator={node.validator} identityReason={node.validatorIdentityReason} variant="inline" /><span aria-hidden="true">·</span></>}
          <LastReportAge timestamp={node.lastReportAt} variant="inline" />
        </div>
        <NodeIdLine key={node.nodeId} nodeId={node.nodeId} />
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
  return <p className="m-0 mt-0.5 flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-[11px] text-muted-foreground">
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
  // Three heights that agree in the healthy case are noise repeated three
  // times; Observed Head, Network Head and the lag are what the reader compares.
  // The labels stay exactly as they were — only weight and rhythm change.
  return <NodeInfoGroup dense title="Chain state" label="Node chain state">
    {(qc != null || locked != null || committed != null) && <div className="grid grid-cols-1 gap-1 border-b border-border/60 pb-1.5" role="group" aria-label="Consensus heads">
      {qc != null && <MetricRow layout="compact" label="QC Head" value={qc.toLocaleString()} />}
      {locked != null && <MetricRow layout="compact" label="Locked Head" value={locked.toLocaleString()} />}
      {committed != null && <MetricRow layout="compact" label="Committed Head" value={committed.toLocaleString()} />}
    </div>}
    {observed != null && <MetricRow label="Observed Head" value={observed.toLocaleString()} />}
    {network != null && <MetricRow label="Network Head" value={network.toLocaleString()} />}
    {lag != null && <MetricRow label="Head lag" value={<span className={lag.toneClass}>{lag.label}</span>} />}
    {!hasRows && <p className="m-0 text-[11px] text-muted-foreground">Chain state has not been observed yet.</p>}
    {staleParts.length > 0 && <p data-slot="chain-state-stale" className="m-0 text-[11px] text-muted-foreground">Last-good consensus observation · {staleParts.join(' · ')}</p>}
  </NodeInfoGroup>
}

function processCollectionLabel(state: string | null | undefined): string {
  return state?.trim().toLowerCase() === 'ok' ? 'Successful' : nodeComponentStateLabel(state)
}

function ProcessGroup({ node, nodeDataProgressValue }: { node: PublicNode; nodeDataProgressValue: number | null }) {
  const bytes = formatNodeDataBytes(finiteNonNegative(node.nodeDataDirectorySizeBytes) ? node.nodeDataDirectorySizeBytes : null, finiteNonNegative(node.nodeDataDirectoryCapacityBytes) ? node.nodeDataDirectoryCapacityBytes : null)
  const started = formatUtcDateTime(node.processStartedAt)
  return <NodeInfoGroup dense title="Process" label="PlatON process">
    <MetricRow label="Collection" labelAddon={<InfoHint label="About Collection">
      <p className="m-0">The result of the most recent process collection. It is not a live Running or Stopped process status.</p>
    </InfoHint>} value={<span className="inline-flex items-center gap-1.5"><span className={cn('inline-block size-2 shrink-0 rounded-full', processStateDot(node.processState))} aria-hidden="true" />{processCollectionLabel(node.processState)}</span>} />
    {started !== 'Unknown' && <MetricRow label="Started" value={started} />}
    {(bytes != null || nodeDataProgressValue != null) && <div className="grid grid-cols-1 gap-1.5" role="group" aria-label="Node data directory">
      {bytes != null && <MetricRow label="Data directory" value={bytes} />}
      {nodeDataProgressValue != null && <MetricRow label="Data usage" labelAddon={<InfoHint label="About Data usage">
        <p className="m-0">The Node data directory's size against the capacity of the filesystem it lives on. It is not whole-Host disk usage.</p>
      </InfoHint>} value={formatPercent(nodeDataProgressValue)} />}
    </div>}
  </NodeInfoGroup>
}

function finiteNonNegative(value: number | null | undefined): value is number {
  return value != null && Number.isFinite(value) && value >= 0
}

function HostResourcesGroup({ node }: { node: PublicNode }) {
  const resources = [['CPU', node.hostCpuPercent], ['Memory', node.hostMemoryPercent], ['Storage', node.hostStoragePercent]] as const
  const upload = finiteNonNegative(node.hostNetworkTxBytesPerSec)
  const download = finiteNonNegative(node.hostNetworkRxBytesPerSec)
  return <NodeInfoGroup dense title="Host resources" label="Shared Host resources" hint={<InfoHint label="About Host resources">
    <p className="m-0">Collected once per Agent and shared by every Node that Agent monitors, so these values describe the whole Host rather than this Node alone.</p>
  </InfoHint>}>
    {resources.map(([label, value]) => finiteNonNegative(value) ? <MetricRow key={label} label={label} value={formatPercent(value)} progress={value} progressStatus="info" /> : null)}
    {(upload || download) && <MetricRow label="Network" value={<span className="inline-flex flex-wrap items-center justify-end gap-x-2 tabular-nums">{upload && <span>↑ {formatMetricRate(node.hostNetworkTxBytesPerSec)}</span>}{download && <span>↓ {formatMetricRate(node.hostNetworkRxBytesPerSec)}</span>}</span>} />}
    {!resources.some(([, value]) => finiteNonNegative(value)) && !upload && !download && <p className="m-0 text-[11px] text-muted-foreground">Host resources have not been observed yet.</p>}
  </NodeInfoGroup>
}

type MetricSeries = {
  /** Emerald series colour slot; the card, the curve and the legend read it once. */
  key: MetricSeriesKey
  label: string
  points: PublicMetricPoint[]
  /** Server-answered observation/coverage evidence for this direction. */
  coverage?: PublicMetricSeriesCoverage
  /** Emerald's transparent vertical gradient, for the single-direction CPU and
   *  memory curves only. */
  area?: boolean
  /** The gradient's two stops, when one curve needs a fainter pair than Emerald's
   *  own recipe: memory can hold one value across the whole window, and a steady
   *  reading must not be painted as a slab. */
  areaOpacity?: readonly [number, number]
}

type MetricChartKind = 'line' | 'bar'

/** The chart's tooltip states each retained sample with its own unit — an
 *  unrounded percent, and a byte rate on the same binary ladder the card's
 *  top-right value reads it in (issue #249). The formatters are module-level so
 *  the option is not rebuilt on every render. */

/** One current reading of a two-direction card. */
type MetricReading = { label: string; value: string; direction: 'up' | 'down'; color: string }

function NodeMetricCard({ label, value, readings, detail, kind = 'line', series, showLegend = false, history, axis, valueFormat, historyMessage, retained = false, windowText, countNoun, className = '' }: {
  label: string
  /** The current value beside the title; its unit is part of the value
   *  ("12.3%"), so the title never repeats it. */
  value?: string
  /** A two-direction card's current readings, on one row beside the title in
   *  Emerald's order: each behind a direction chevron in its own curve's colour,
   *  named only for assistive technology. */
  readings?: MetricReading[]
  detail?: string
  kind?: MetricChartKind
  series: MetricSeries[]
  showLegend?: boolean
  history?: PublicNodeMetricHistory
  /** Ceiling, steps and unit-bearing title of the value axis. */
  axis: MetricAxisRule
  valueFormat: (value: number) => string
  historyMessage?: string
  /** The window is the last successful one, held after a failed refresh. */
  retained?: boolean
  /** A longer Metric Window's own words (ADR 0012); absent for Live, whose
   *  sentences stay exactly as they were. */
  windowText?: MetricWindowText
  /** What a bucket of this card counts, for its tooltip: the block charts
   *  count blocks and intervals rather than samples (#248). */
  countNoun?: MetricCountNoun
  className?: string
}) {
  const seconds = metricWindowSeconds(history)
  const fromMs = history ? Date.parse(history.from) : Number.NaN
  const toMs = history ? Date.parse(history.to) : Number.NaN
  const validWindow = Number.isFinite(fromMs) && Number.isFinite(toMs) && toMs > fromMs
  const selectedWindow = history?.metricWindow?.window ?? 'live'
  // One formatter per window and bounds, so the chart option is not rebuilt on
  // every render; Live keeps the chart's own clock labels.
  const timeFormat = useMemo(
    () => (windowText && validWindow ? metricAxisTimeFormat(selectedWindow, toMs) : undefined),
    [windowText, validWindow, selectedWindow, toMs],
  )
  // A direction series that has no retained sample must not disappear behind
  // its available partner: name it explicitly while the partner keeps plotting.
  const missingDirections = series.length > 1 && series.some((item) => item.points.length > 0)
    ? series.filter((item) => item.points.length === 0).map((item) => item.label)
    : []
  // Gaps, an unobserved tail and a never-observed direction are stated to the
  // chart's accessible description instead of as a line of small print under the
  // title: the reader must not have to infer absence from a straight line, and a
  // card that has nothing to say draws no extra line above its plot.
  const coverageText = coverageNotes(series)
  // The window is the evidence: observations outside it are dropped and the
  // Server's own silence list decides where a run ends, so nothing is joined
  // across a stretch nobody observed and nothing is filled with a zero.
  const segments = series.map((item) => (validWindow ? coverageSegments(item.points, fromMs, toMs, item.coverage?.gaps) : []))
  const plots: MetricChartSeriesInput[] = series.map((item, index) => ({
    label: item.label,
    color: METRIC_SERIES_COLORS[item.key],
    area: item.area,
    areaOpacity: item.areaOpacity,
    segments: segments[index] ?? [],
  }))
  const observations = segments.flat(2)
  // The accessible description states the same thing the curve does: which
  // directions, over which real window, and what the Server measured missing.
  // It is also the only place the coverage sentence is written down now: the
  // card draws no small print for it, but a screen reader still hears it.
  const adjective = windowText?.adjective ?? seconds + '-second'
  const description = (observations.length > 0
    ? series.map((item) => item.label).join(' and ') + (retained
      ? ' values from a retained ' + adjective + ' window, ' + formatUtcDateTime(history?.from) + ' to ' + formatUtcDateTime(history?.to) + '; the latest history update failed'
      : windowText
        ? ' values over ' + windowText.span + (observations.some((point) => point.bucket) ? ', drawn through each bucket\'s maximum' : '')
        : ' values from ' + seconds + ' seconds ago to now')
    : label + ': ' + (historyMessage ?? windowText?.empty ?? 'No samples in the last minute'))
    + (coverageText ? '. ' + coverageText : '')
  return (
    // Emerald's card, with its own header used for the header: `size="small"` gives
    // the same 12px inset to the title and to the plot, so the six plots start on
    // one column and the titles share a baseline. The title is the card's largest
    // type (16px, bold) and the readings sit beside it at 12px — the current value
    // never becomes the headline, and it never moves to the foot of the card.
    <CardX
      bordered={false}
      role="article"
      data-slot="node-metric-card"
      size="small"
      className={cn('min-w-0', CARD, className)}
      contentClassName="flex h-full min-w-0 flex-col gap-1.5"
      header={(
        <div data-slot="node-metric-header" className="flex w-full min-w-0 items-center justify-between gap-2">
          <h3 className="m-0 min-w-0 truncate text-base font-bold">{label}</h3>
          {readings
            ? (
                // One row on desktop, as Emerald's network header; a narrow card may
                // wrap the second reading rather than clip it.
                <div data-slot="node-metric-readings" className="flex min-w-0 flex-wrap items-center justify-end gap-x-2 gap-y-0.5 text-xs leading-none">
                  {readings.map((item, index) => (
                    <span key={item.label} data-slot="node-metric-reading" data-direction={item.direction} className="inline-flex items-center gap-0.5 whitespace-nowrap">
                      <DirectionChevron direction={item.direction} color={item.color} />
                      <span className="sr-only">{item.label + ' '}</span>
                      <strong data-slot={index === 0 ? 'node-metric-value' : 'node-metric-value-secondary'} className="font-semibold tabular-nums">{item.value}</strong>
                    </span>
                  ))}
                </div>
              )
            : <strong data-slot="node-metric-value" className="shrink-0 text-right text-xs font-semibold leading-none tabular-nums">{value}</strong>}
        </div>
      )}
    >
      {/* Facts and the direction note own this block: it is what grows when one card
          is taller than its neighbours, which is what lets the plot below stay pinned
          to the bottom of every card without a spacer propping it there. The coverage
          sentence is deliberately absent: it rides the chart's accessible description,
          so the deck stays as quiet as the Emerald cards it is ported from. */}
      <div className="min-w-0">
        {detail && <p className="m-0 break-words text-[11px] leading-4 text-muted-foreground">{detail}</p>}
        {missingDirections.length > 0 && <p className="m-0 text-[11px] italic text-muted-foreground">{missingDirections.join(' and ')} unavailable in this window</p>}
      </div>
      <NodeMetricChart
        label={label}
        kind={kind}
        from={fromMs}
        to={toMs}
        series={plots}
        axis={axis}
        valueFormat={valueFormat}
        seconds={seconds}
        {...(windowText ? { windowText } : {})}
        {...(timeFormat ? { timeFormat } : {})}
        {...(countNoun ? { countNoun } : {})}
        message={historyMessage}
        description={description}
        retained={retained}
        showLegend={showLegend}
      />
    </CardX>
  )
}

/** Emerald's compact direction chevron beside a reading, drawn in that series'
 *  own colour: the blue download and the purple upload are told apart by more
 *  than their order, and the legend under the plot repeats both. */
function DirectionChevron({ direction, color }: { direction: 'up' | 'down'; color: string }) {
  const Icon = direction === 'up' ? ChevronUp : ChevronDown
  return <Icon aria-hidden="true" data-slot="node-metric-direction" className="size-3 shrink-0" strokeWidth={2.5} style={{ color }} />
}









/** The window length the heading, every card and every chart's accessible name
 *  share, so a retained window is never described with two different lengths. */
function metricWindowSeconds(history: PublicNodeMetricHistory | undefined): number {
  const seconds = history?.windowSeconds
  return seconds !== undefined && Number.isFinite(seconds) && seconds > 0 ? Math.round(seconds) : 60
}



function latestBlockInterval(history: NonNullable<ReturnType<typeof usePublicNodeHistory>['data']>): { value: string; detail: string } {
  const blocks = history.filter((item) => item.height != null && item.blockTimeMs != null).slice(0, 2)
  if (blocks.length < 2) return { value: 'Unknown', detail: 'Two Block Summaries are required' }
  const [latest, previous] = blocks
  if (latest.height == null || previous.height == null || latest.blockTimeMs == null || previous.blockTimeMs == null || latest.height - previous.height !== 1) {
    return { value: 'Unknown', detail: 'Consecutive Block Summaries unavailable' }
  }
  const elapsed = latest.blockTimeMs - previous.blockTimeMs
  if (elapsed < 0) return { value: 'Unknown', detail: 'Block timestamps are inconsistent' }
  const value = elapsed < 1000 ? elapsed + ' ms' : (elapsed / 1000).toFixed(2) + ' s'
  return { value, detail: latest.height.toLocaleString() + ' → ' + previous.height.toLocaleString() }
}

/** A process value is the retained current Public Projection value; when the
 *  process component is not Current it is explicitly marked as last-good. */
function processStatusDetail(state: string | null | undefined): string | undefined {
  const label = nodeComponentStateLabel(state)
  if (label === 'Current') return undefined
  return 'last-good value retained · collection ' + label
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
  return !finiteNonNegative(value) ? 'Unknown' : value.toLocaleString()
}

function formatPercent(value: number | null | undefined): string {
  return !finiteNonNegative(value) ? 'Unknown' : value.toFixed(1) + '%'
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
