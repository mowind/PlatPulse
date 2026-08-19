import { useState } from 'react'
import { Link } from 'react-router'
import type { PublicNode, PublicValidatorAnalyticsResponse } from '../api/generated'
import { fetchNodeHistoryExport, fetchNodePeerHistory, type PublicHistoryItem } from '../api/public'
import { PeerInsight } from './PeerInsight'
import { PeerHistoryInsight, normalizePublicPeerHistory } from './PeerHistoryInsight'
import { ValidatorAnalytics } from './ValidatorAnalytics'
import { ValidatorInsight } from './ValidatorInsight'

type NodeDetailDashboardProps = {
  node: PublicNode
  history: PublicHistoryItem[]
  peerHistory: Awaited<ReturnType<typeof fetchNodePeerHistory>> | null
  peerHistoryError: boolean
  validatorAnalytics: PublicValidatorAnalyticsResponse | null
  onExportError: (message: string) => void
}

type DetailTab = 'details' | 'network'

/** Reference-inspired Node Detail dashboard using only PlatPulse Public data. */
export default function NodeDetailDashboard({
  node,
  history,
  peerHistory,
  peerHistoryError,
  validatorAnalytics,
  onExportError,
}: NodeDetailDashboardProps) {
  const [tab, setTab] = useState<DetailTab>('details')
  const healthTone = statusTone(node.health)
  const exportHistory = () => {
    void fetchNodeHistoryExport(node.nodeId)
      .then((items) => {
        const blob = new Blob([JSON.stringify(items, null, 2)], { type: 'application/json' })
        const url = URL.createObjectURL(blob)
        const anchor = document.createElement('a')
        anchor.href = url
        anchor.download = 'public-history.json'
        anchor.click()
        URL.revokeObjectURL(url)
      })
      .catch(() => onExportError('Unable to export block history'))
  }

  return (
    <section className="page node-detail-dashboard" aria-labelledby="node-detail-title">
      <div className="node-detail-heading-row">
        <div>
          <p className="node-detail-kicker">PLATPULSE / NODE DETAIL</p>
          <h1 id="node-detail-title">{node.displayName ?? 'Node detail'}</h1>
          <p className="node-detail-clock">Live public observation <strong>{formatFreshness(node.freshness)}</strong></p>
        </div>
        <span className={'node-detail-live node-detail-live-' + healthTone}><i aria-hidden="true" />{node.health}</span>
      </div>

      <article className="node-detail-summary">
        <div className="node-detail-summary-top">
          <Link className="node-detail-back" to={'/networks/' + node.networkKey} aria-label={'Back to ' + node.networkKey}>←</Link>
          <div className="node-detail-identity">
            <p className="node-detail-network"><Link to={'/networks/' + node.networkKey}>{node.networkKey}</Link></p>
            <h2>{node.displayName ?? node.nodeId}</h2>
            <p>{node.nodeId}</p>
          </div>
          <button className="node-detail-export" type="button" onClick={exportHistory}>Export public history</button>
        </div>

        <div className="node-detail-facts">
          <Fact label="Health" value={node.health} tone={healthTone} />
          <Fact label="RPC" value={node.rpcState} tone={statusTone(node.rpcState)} />
          <Fact label="Sync" value={node.syncState} tone={statusTone(node.syncState)} />
          <Fact label="Consensus" value={node.consensusState} tone={statusTone(node.consensusState)} />
          <Fact label="Process" value={node.processState} tone={statusTone(node.processState)} />
          <Fact label="Resync" value={node.resyncState + (node.resyncProgress ? ' · ' + node.resyncProgress : '')} tone={statusTone(node.resyncState)} />
        </div>

        <div className="node-detail-observations">
          <Observation label="Current head" value={formatNumber(node.currentHead)} />
          <Observation label="History boundary" value={formatNumber(node.historicalHighWatermark)} />
          <Observation label="Network reference" value={formatNumber(node.networkReferenceHead)} />
          <Observation label="Reference confidence" value={node.networkReferenceConfidence} />
          <Observation label="Host CPU" value={node.hostCpuPercent == null ? 'Unknown' : node.hostCpuPercent.toFixed(1) + '%'} />
          <Observation label="Peer count" value={node.peers?.peerCount == null ? 'Unknown' : String(node.peers.peerCount)} />
        </div>
        <p className="node-detail-health-reason">{node.healthReason}</p>
      </article>

      <div className="node-detail-tabs" role="tablist" aria-label="Node Detail sections">
        <button type="button" role="tab" aria-selected={tab === 'details'} className={tab === 'details' ? 'active' : ''} onClick={() => setTab('details')}>Details</button>
        <button type="button" role="tab" aria-selected={tab === 'network'} className={tab === 'network' ? 'active' : ''} onClick={() => setTab('network')}>Network</button>
      </div>

      {tab === 'details' ? (
        <>
          <div className="node-detail-chart-grid" aria-label="Current Node observations">
            <SignalCard label="Host CPU" value={node.hostCpuPercent == null ? 'Unknown' : node.hostCpuPercent.toFixed(1) + '%'} detail="Latest accepted host observation" tone={node.hostCpuPercent == null ? 'neutral' : node.hostCpuPercent > 85 ? 'bad' : node.hostCpuPercent > 65 ? 'warn' : 'good'} progress={node.hostCpuPercent ?? undefined} />
            <SignalCard label="Current head" value={formatNumber(node.currentHead)} detail="Latest public chain observation" tone="indigo" />
            <SignalCard label="History boundary" value={formatNumber(node.historicalHighWatermark)} detail="Accepted historical high-watermark" tone="violet" />
            <SignalCard label="Peers" value={node.peers?.peerCount == null ? 'Unknown' : String(node.peers.peerCount)} detail={node.peers?.freshness ?? 'Peer freshness unknown'} tone={statusTone(node.peers?.state ?? 'unknown')} />
            <SignalCard label="RPC" value={node.rpcState} detail="Node RPC collection state" tone={statusTone(node.rpcState)} />
            <SignalCard label="Consensus" value={node.consensusState} detail="Node consensus observation" tone={statusTone(node.consensusState)} />
          </div>

          <section className="node-detail-history-panel" aria-labelledby="node-history-title">
            <div className="node-detail-panel-heading">
              <div><p className="node-detail-kicker">CHAIN EVIDENCE</p><h2 id="node-history-title">Block history</h2></div>
              <span>{history.length.toLocaleString()} published rows</span>
            </div>
            {history.length === 0 ? <p className="node-detail-empty">No block history observed yet.</p> : <div className="node-detail-history-table-wrap"><table className="node-detail-history-table"><thead><tr><th scope="col">Height</th><th scope="col">Block time</th><th scope="col">Transactions</th><th scope="col">Source</th><th scope="col">Observed</th></tr></thead><tbody>{history.slice(0, 24).map((block, index) => <tr key={String(block.height ?? 'unknown') + '-' + index}><td>{formatNumber(block.height)}</td><td>{formatNumber(block.blockTimeMs)} ms</td><td>{formatNumber(block.transactionCount)}</td><td>{block.source ?? 'Unknown'}</td><td>{formatObservedAt(block.observedAt)}</td></tr>)}</tbody></table></div>}
          </section>

          {node.validator && <section className="node-detail-secondary-panel"><ValidatorInsight insight={node.validator} />{validatorAnalytics && <ValidatorAnalytics analytics={validatorAnalytics} compact />}</section>}
        </>
      ) : (
        <section className="node-detail-network-panel" aria-labelledby="node-network-title">
          <div className="node-detail-panel-heading"><div><p className="node-detail-kicker">NETWORK OBSERVATION</p><h2 id="node-network-title">Peer network</h2></div><span>{node.peers?.freshness ?? 'Freshness unknown'}</span></div>
          <PeerInsight insight={node.peers} />
          <PeerHistoryInsight history={peerHistory ? normalizePublicPeerHistory(peerHistory) : undefined} error={peerHistoryError} loading={!peerHistory && !peerHistoryError} />
        </section>
      )}
    </section>
  )
}

function Fact({ label, value, tone }: { label: string; value: string; tone: string }) {
  return <div className="node-detail-fact"><span>{label}</span><strong className={'node-detail-fact-' + tone}><i aria-hidden="true" />{value}</strong></div>
}

function Observation({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>
}

function SignalCard({ label, value, detail, tone, progress }: { label: string; value: string; detail: string; tone: string; progress?: number }) {
  const width = progress == null ? 0 : Math.max(0, Math.min(100, progress))
  return <article className={'node-detail-signal node-detail-signal-' + tone}><div className="node-detail-signal-top"><span>{label}</span><b>{value}</b></div><p>{detail}</p>{progress != null && <span className="node-detail-progress"><i style={{ width: width + '%' }} /></span>}<span className="node-detail-signal-line" aria-hidden="true" /></article>
}

function statusTone(value: string | null | undefined) {
  const normalized = (value ?? 'unknown').toLowerCase()
  if (/(healthy|current|ok|ready|synced|active|running|fresh|live|idle)/.test(normalized)) return 'good'
  if (/(stale|starting|unknown|unsupported|disabled|empty|resync|degraded|paused|attention)/.test(normalized)) return 'warn'
  if (/(error|failed|unhealthy|offline|retired|unavailable)/.test(normalized)) return 'bad'
  return 'neutral'
}

function formatNumber(value: number | null | undefined) {
  return value == null ? 'Unknown' : value.toLocaleString()
}

function formatFreshness(value: string | null | undefined) {
  if (!value) return 'Unknown'
  const date = new Date(value)
  if (Number.isNaN(date.valueOf())) return value
  const minutes = Math.max(0, Math.round((Date.now() - date.valueOf()) / 60000))
  return minutes < 1 ? 'Just now' : minutes + 'm ago'
}

function formatObservedAt(value: string | null | undefined) {
  if (!value) return 'Unknown'
  const date = new Date(value)
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleString()
}
