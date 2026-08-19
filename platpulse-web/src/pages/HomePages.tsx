import { useEffect, useRef, useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import {
  fetchNetwork,
  fetchNode,
  fetchNodeHistory,
  fetchNodePeerHistory,
  fetchValidatorAnalytics,
  fetchValidatorHistory,
} from '../api/public'
import type {
  PublicNetwork,
  PublicNode,
  PublicValidatorAnalyticsResponse,
  PublicValidatorHistoryResponse,
} from '../api/generated'
import { useHomeRealtimeContext } from '../layouts/HomeLayout'
import HomeNodeDetailPrototype from '../components/HomeNodeDetailPrototype'
import NodeDetailDashboard from '../components/NodeDetailDashboard'
import { PeerInsight } from '../components/PeerInsight'
import { GeoInsight } from '../components/GeoInsight'
import { ValidatorInsight } from '../components/ValidatorInsight'
import { ValidatorAnalytics } from '../components/ValidatorAnalytics'

export function NetworkPage() {
  const { networkKey = '' } = useParams()
  const { reloadKey, resetting } = useHomeRealtimeContext()
  const [network, setNetwork] = useState<PublicNetwork | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setNetwork(null)
    setError(null)
    if (resetting) return () => controller.abort()
    fetchNetwork(networkKey, controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) setNetwork(data)
      })
      .catch((caught: Error) => {
        if (!controller.signal.aborted && caught.name !== 'AbortError') setError(caught.message)
      })
    return () => controller.abort()
  }, [networkKey, reloadKey, resetting])

  const [validatorHistories, setValidatorHistories] = useState<Record<string, PublicValidatorHistoryResponse>>({})
  const [validatorAnalytics, setValidatorAnalytics] = useState<Record<string, PublicValidatorAnalyticsResponse>>({})

  useEffect(() => {
    const controller = new AbortController()
    setValidatorHistories({})
    setValidatorAnalytics({})
    if (!network || resetting) return () => controller.abort()
    Promise.all(network.validators.map(async (validator) => {
      try {
        return [validator.validatorId, await fetchValidatorHistory(validator.validatorId, 20, controller.signal)] as const
      } catch {
        return null
      }
    })).then((results) => {
      if (!controller.signal.aborted) setValidatorHistories(Object.fromEntries(results.filter((result): result is readonly [string, PublicValidatorHistoryResponse] => result !== null)))
    })
    Promise.all(network.validators.map(async (validator) => {
      try {
        return [validator.validatorId, await fetchValidatorAnalytics(validator.validatorId, 31, controller.signal)] as const
      } catch {
        return null
      }
    })).then((results) => {
      if (!controller.signal.aborted) setValidatorAnalytics(Object.fromEntries(results.filter((result): result is readonly [string, PublicValidatorAnalyticsResponse] => result !== null)))
    })
    return () => controller.abort()
  }, [network, resetting])

  if (resetting) return <section className="page"><p role="status">Revalidating Home access…</p></section>
  if (error) return <section className="page"><p role="alert" className="form-error">{error}</p><Link to="/">Back to Home</Link></section>
  if (!network) return <section className="page"><p role="status">Loading Network…</p></section>
  return <section className="page"><p><Link to="/">← All Networks</Link></p><h1>{network.displayName}</h1><p className="muted">{network.networkKey}</p><PeerInsight insight={network.peers} /><GeoInsight insight={network.geo} />{network.validators.length > 0 && <><h2>Validators</h2><div className="node-grid">{network.validators.map((validator) => <article className="node-card" key={validator.validatorId}><ValidatorInsight insight={validator} history={validatorHistories[validator.validatorId]?.entries} />{validatorAnalytics[validator.validatorId] && <ValidatorAnalytics analytics={validatorAnalytics[validator.validatorId]!} compact />}</article>)}</div></>}<div className="node-grid">{network.nodes.map((node) => <NodeCard node={node} key={node.nodeId} />)}</div></section>
}

export function NodePage() {
  const { nodeId = '' } = useParams()
  const [searchParams] = useSearchParams()
  const { reloadKey, resetting } = useHomeRealtimeContext()
  const [node, setNode] = useState<PublicNode | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [history, setHistory] = useState<Awaited<ReturnType<typeof fetchNodeHistory>>>([])
  const [peerHistory, setPeerHistory] = useState<Awaited<ReturnType<typeof fetchNodePeerHistory>> | null>(null)
  const [peerHistoryError, setPeerHistoryError] = useState(false)
  const [validatorAnalytics, setValidatorAnalytics] = useState<PublicValidatorAnalyticsResponse | null>(null)
  const previousNodeId = useRef<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    const nodeChanged = previousNodeId.current !== nodeId
    previousNodeId.current = nodeId
    if (nodeChanged || resetting) {
      setNode(null)
      setHistory([])
      setPeerHistory(null)
      setPeerHistoryError(false)
    }
    setError(null)
    if (resetting) return () => controller.abort()

    fetchNode(nodeId, controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) setNode(data)
      })
      .catch((caught: Error) => {
        if (!controller.signal.aborted && caught.name !== 'AbortError') setError(caught.message)
      })
    fetchNodeHistory(nodeId, controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) setHistory(data)
      })
      .catch((caught: Error) => {
        if (!controller.signal.aborted && caught.name !== 'AbortError') setHistory([])
      })
    fetchNodePeerHistory(nodeId, controller.signal)
      .then((value) => {
        if (controller.signal.aborted) return
        setPeerHistory(value)
        setPeerHistoryError(false)
      })
      .catch((caught: Error) => {
        if (!controller.signal.aborted && caught.name !== 'AbortError') setPeerHistoryError(true)
      })

    return () => controller.abort()
  }, [nodeId, reloadKey, resetting])

  useEffect(() => {
    const controller = new AbortController()
    setValidatorAnalytics(null)
    const validatorId = node?.validator?.validatorId
    if (!validatorId || resetting) return () => controller.abort()
    fetchValidatorAnalytics(validatorId, 31, controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) setValidatorAnalytics(data)
      })
      .catch((caught: Error) => {
        if (!controller.signal.aborted && caught.name !== 'AbortError') setValidatorAnalytics(null)
      })
    return () => controller.abort()
  }, [node?.validator?.validatorId, reloadKey, resetting])

  if (import.meta.env.DEV && searchParams.get('variant') && node) return <HomeNodeDetailPrototype node={node} history={history} />
  if (resetting) return <section className="page"><p role="status">Revalidating Node access…</p></section>
  if (error) return <section className="page"><p role="alert" className="form-error">{error}</p><Link to="/">Back to Home</Link></section>
  if (!node) return <section className="page"><p role="status">Loading Node…</p></section>
  return <NodeDetailDashboard node={node} history={history} peerHistory={peerHistory} peerHistoryError={peerHistoryError} validatorAnalytics={validatorAnalytics} onExportError={setError} />
}

function NodeCard({ node }: { node: PublicNode }) {
  return <article className="node-card"><h2><Link to={'/nodes/' + node.nodeId}>{node.displayName ?? node.nodeId}</Link></h2><p><span className={'status status-' + node.health}>{node.health}</span> {node.healthReason}</p><p className="muted">RPC: {node.rpcState} · Sync: {node.syncState} · Consensus: {node.consensusState} · Head: {node.currentHead ?? 'unknown'} · History: {node.historicalHighWatermark ?? 'unknown'} · {node.resyncState}</p>{node.validator && <ValidatorInsight insight={node.validator} compact />}<PeerInsight insight={node.peers} compact /></article>
}
