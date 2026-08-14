// Typed Admin adapter and query/realtime layer (design §6.1–§6.3).
//
// The generated OpenAPI client is imported through the project client
// singleton; this module owns request normalization, the dedicated Admin
// query cache (structurally isolated from Public queries), and the
// Admin-specific SSE invalidation stream. REST remains authoritative: SSE
// carries invalidation/reset signals only, and every invalidation refetches
// the matching Admin REST resource through the cache.

import { QueryClient, keepPreviousData, useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import {
  adminAgentAudit,
  adminAgentDetail,
  adminEnrollmentToken,
  adminNetworkDetail,
  adminNetworks,
  adminNodeDetail,
  adminNodeTransfers,
  adminNodes,
  adminRecoveryToken,
  adminRevokeCredential,
  adminRotateCredential,
  cancelNodeTransfer as cancelNodeTransferApi,
  createNetwork,
  createNodeTransfer as createNodeTransferApi,
  diagnostics,
  overview,
  setNodeMetadata,
  setVisibility,
  updateNetwork,
  type AdminNetwork,
  type AdminNetworkDetail,
  type AdminNodeDetail,
  type AdminNodeListItem,
  type AdminOverview,
  type AgentAuditResponse,
  type AgentDiagnostic,
  type ApiErrorBody,
  type EnrollmentTokenResponse,
  type NetworkCreateRequest,
  type NetworkResponse,
  type NetworkUpdateRequest,
  type NodeMetadataRequest,
  type NodeMetadataResponse,
  type NodeTransfer,
  type NodeTransferCreateRequest,
  type NodeTransferMutationResponse,
  type RecoveryTokenResponse,
  type RevokeResponse,
  type RotationResponse,
  type VisibilityRequest,
  type VisibilityResponse,
} from './generated'

/**
 * Dedicated Admin query cache. Public queries never share this object or
 * its keys, so authorization boundaries cannot cross through runtime cache
 * state (design §6.2, §7 `PATTERN-AUTH-GENERATION`).
 */
export const adminQueryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 0,
      refetchOnWindowFocus: false,
      staleTime: 30_000,
    },
  },
})

const adminKeys = {
  all: ['admin'] as const,
  overview: ['admin', 'overview'] as const,
  diagnostics: ['admin', 'diagnostics'] as const,
  agents: ['admin', 'agents'] as const,
  agentDetail: (agentId: string) => ['admin', 'agents', agentId] as const,
  agentAudit: (agentId: string) => ['admin', 'agents', agentId, 'audit'] as const,
  nodes: ['admin', 'nodes'] as const,
  nodeDetail: (nodeId: string) => ['admin', 'nodes', nodeId] as const,
  nodeTransfers: (nodeId: string) => ['admin', 'nodes', nodeId, 'transfers'] as const,
  networks: ['admin', 'networks'] as const,
  networkDetail: (networkKey: string) => ['admin', 'networks', networkKey] as const,
}

/**
 * The access generation the Admin cache currently belongs to. Cleared and
 * re-tagged on every authorization transition so a shell mount can tell
 * whether cached values may come from an older session.
 */
let adminCacheGeneration: number | null = null

/** A failed Admin call, keyed by the Server's stable error `code`, with the
 * request and Audit references an operator can use to look the failure up
 * (issue #46: confirmation and mutation errors expose references, never
 * sensitive details). */
export class AdminApiError extends Error {
  readonly code: string
  readonly requestId: string | null
  readonly fields: string[]

  constructor(
    code: string,
    message: string,
    requestId: string | null = null,
    fields: string[] = [],
  ) {
    super(message)
    this.name = 'AdminApiError'
    this.code = code
    this.requestId = requestId
    this.fields = fields
  }
}

/**
 * Access-generation reset bus. The Admin surface subscribes once per shell;
 * REST 401s (`auth_required`, e.g. an expired or revoked session) notify the
 * bus so the shell can close streams, clear the Admin cache, and re-check
 * the session before any older-generation response can render.
 */
type AccessResetListener = () => void
const accessResetListeners = new Set<AccessResetListener>()

export function subscribeAdminAccessReset(listener: AccessResetListener): () => void {
  accessResetListeners.add(listener)
  return () => accessResetListeners.delete(listener)
}

function notifyAdminAccessReset(): void {
  for (const listener of accessResetListeners) listener()
}

async function requestAdmin<T>(
  run: () => Promise<{ data?: T; error?: unknown }>,
  fallbackMessage: string,
): Promise<T> {
  const { data, error } = await run()
  if (error || !data) {
    const body = error as ApiErrorBody | undefined
    if (body?.error?.code === 'auth_required') notifyAdminAccessReset()
    throw new AdminApiError(
      body?.error?.code ?? 'unavailable',
      body?.error?.message ?? fallbackMessage,
      body?.error?.requestId ?? null,
      body?.error?.fields ?? [],
    )
  }
  return data
}

/** Server-owned attention overview (REST-authoritative). */
export async function fetchAdminOverview(signal?: AbortSignal): Promise<AdminOverview> {
  return requestAdmin(
    () => overview({ signal }),
    'Unable to load the Admin overview',
  )
}

/** Full Agent/Node diagnostics projected by the Server. */
export async function fetchAdminDiagnostics(signal?: AbortSignal): Promise<AgentDiagnostic[]> {
  return requestAdmin(
    () => diagnostics({ signal }),
    'Unable to load Admin diagnostics',
  )
}

export function useAdminOverview(generation: number) {
  return useQuery({
    // The generation is part of the key: data cached under an older access
    // generation is never rendered under the current one.
    queryKey: [...adminKeys.overview, generation],
    queryFn: ({ signal }) => fetchAdminOverview(signal),
    // Keep the last authoritative value visible while an SSE-triggered
    // refetch runs; a failed refetch never blanks the panel.
    placeholderData: keepPreviousData,
  })
}

export function useAdminDiagnostics(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.diagnostics, generation],
    queryFn: ({ signal }) => fetchAdminDiagnostics(signal),
    placeholderData: keepPreviousData,
  })
}

/** Server-owned Agent detail: identity, liveness, boot/report, credentials,
 * Inventory, and diagnostics as separate dimensions. */
export async function fetchAdminAgent(
  agentId: string,
  signal?: AbortSignal,
): Promise<AgentDiagnostic> {
  return requestAdmin(
    () => adminAgentDetail({ path: { agent_id: agentId }, signal }),
    'Unable to load the Agent',
  )
}

/** Redacted Audit trail scoped to one Agent. */
export async function fetchAdminAgentAudit(
  agentId: string,
  signal?: AbortSignal,
): Promise<AgentAuditResponse> {
  return requestAdmin(
    () => adminAgentAudit({ path: { agent_id: agentId }, signal }),
    'Unable to load the Agent audit trail',
  )
}

export function useAdminAgentDetail(generation: number, agentId: string) {
  return useQuery({
    queryKey: [...adminKeys.agentDetail(agentId), generation],
    queryFn: ({ signal }) => fetchAdminAgent(agentId, signal),
    placeholderData: keepPreviousData,
    enabled: agentId.length > 0,
  })
}

export function useAdminAgentAudit(generation: number, agentId: string) {
  return useQuery({
    queryKey: [...adminKeys.agentAudit(agentId), generation],
    queryFn: ({ signal }) => fetchAdminAgentAudit(agentId, signal),
    placeholderData: keepPreviousData,
    enabled: agentId.length > 0,
  })
}

/** Owner-only Node inventory (issue #45): per-Node rows with Server-owned
 * metadata, lifecycle guidance, freshness, health summary, and identity
 * disposition. */
export async function fetchAdminNodes(signal?: AbortSignal): Promise<AdminNodeListItem[]> {
  return requestAdmin(
    () => adminNodes({ signal }),
    'Unable to load the Node inventory',
  )
}

export function useAdminNodes(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.nodes, generation],
    queryFn: ({ signal }) => fetchAdminNodes(signal),
    placeholderData: keepPreviousData,
  })
}

/** Server-owned Node detail with per-Node observation dimensions. */
export async function fetchAdminNode(
  nodeId: string,
  signal?: AbortSignal,
): Promise<AdminNodeDetail> {
  return requestAdmin(
    () => adminNodeDetail({ path: { node_id: nodeId }, signal }),
    'Unable to load the Node',
  )
}

export function useAdminNodeDetail(generation: number, nodeId: string) {
  return useQuery({
    queryKey: [...adminKeys.nodeDetail(nodeId), generation],
    queryFn: ({ signal }) => fetchAdminNode(nodeId, signal),
    // No placeholder: another Node's DTO must never render under this
    // Node's URL (per-Node scoping and non-leaking unknown/private state).
    enabled: nodeId.length > 0,
  })
}

/** Two-phase Transfer history of one Node (issue #46): typed outcomes with
 * Server-owned effective status — pending, completed, cancelled, expired,
 * rejected, conflict, identity_mismatch — and Audit references. */
export async function fetchAdminNodeTransfers(
  nodeId: string,
  signal?: AbortSignal,
): Promise<NodeTransfer[]> {
  return requestAdmin(
    () => adminNodeTransfers({ path: { node_id: nodeId }, signal }),
    'Unable to load the transfer history',
  )
}

export function useAdminNodeTransfers(generation: number, nodeId: string) {
  return useQuery({
    queryKey: [...adminKeys.nodeTransfers(nodeId), generation],
    queryFn: ({ signal }) => fetchAdminNodeTransfers(nodeId, signal),
    // No placeholder: another Node's transfers must never render under
    // this Node's URL.
    enabled: nodeId.length > 0,
  })
}

/** Create a pending two-phase Transfer. The source Agent stays
 * authoritative until the target declares the Node ID with a validated
 * Network Identity; the response carries the typed Transfer and the
 * request/Audit references for the success view. */
export async function createNodeTransfer(
  nodeId: string,
  request: NodeTransferCreateRequest,
  csrfToken: string,
): Promise<NodeTransferMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        createNodeTransferApi({
          path: { node_id: nodeId },
          body: request,
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to start the Node transfer',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    // A failed mutation (e.g. a concurrent operator's pending transfer)
    // must reload the authoritative state instead of leaving a stale form
    // visible (webui.md §6.4: conflicts refetch current state).
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Cancel a pending Transfer. Only `pending` can be cancelled; ownership
 * never changes and the outcome is typed and audited. */
export async function cancelNodeTransfer(
  transferId: string,
  csrfToken: string,
): Promise<NodeTransferMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        cancelNodeTransferApi({
          path: { transfer_id: transferId },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to cancel the Node transfer',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    // The transfer may already be expired/terminal on the Server; reload
    // authoritative state so the timeline shows the true outcome.
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Owner-only Network Registry projection (design §7.1). */
export async function fetchAdminNetworks(signal?: AbortSignal): Promise<AdminNetwork[]> {
  return requestAdmin(
    () => adminNetworks({ signal }),
    'Unable to load the Network Registry',
  )
}

export function useAdminNetworks(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.networks, generation],
    queryFn: ({ signal }) => fetchAdminNetworks(signal),
    placeholderData: keepPreviousData,
  })
}

export async function fetchAdminNetwork(
  networkKey: string,
  signal?: AbortSignal,
): Promise<AdminNetworkDetail> {
  return requestAdmin(
    () => adminNetworkDetail({ path: { network_key: networkKey }, signal }),
    'Unable to load the Network',
  )
}

export function useAdminNetworkDetail(generation: number, networkKey: string) {
  return useQuery({
    queryKey: [...adminKeys.networkDetail(networkKey), generation],
    queryFn: ({ signal }) => fetchAdminNetwork(networkKey, signal),
    // No placeholder: another Network's DTO must never render under this
    // Network's URL.
    enabled: networkKey.length > 0,
  })
}

/**
 * Mutation seam: no optimistic state, no automatic retry. Success invalidates
 * the Admin namespace immediately so the panels refetch authoritative REST.
 */
export async function updateNodeVisibility(
  nodeId: string,
  visibility: VisibilityRequest['visibility'],
  csrfToken: string,
): Promise<VisibilityResponse> {
  const response = await requestAdmin(
    () =>
      setVisibility({
        path: { node_id: nodeId },
        body: { visibility },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to update Node visibility',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Update the Server-owned display name of a Node (issue #45). The Agent
 * Inventory remains authoritative for endpoint, Network key, and Node ID. */
export async function updateNodeMetadata(
  nodeId: string,
  displayName: NodeMetadataRequest['displayName'],
  csrfToken: string,
): Promise<NodeMetadataResponse> {
  const response = await requestAdmin(
    () =>
      setNodeMetadata({
        path: { node_id: nodeId },
        body: { displayName },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to update the Node display name',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Register a Network with the complete validated identity tuple. The
 * Registry is never created from observed Agent text (design §7.1). */
export async function createNetworkEntry(
  request: NetworkCreateRequest,
  csrfToken: string,
): Promise<NetworkResponse> {
  const response = await requestAdmin(
    () =>
      createNetwork({
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to register the Network',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Update the Registry tuple (display name and/or identity fields) with an
 * audited before/after state. */
export async function updateNetworkEntry(
  networkKey: string,
  request: NetworkUpdateRequest,
  csrfToken: string,
): Promise<NetworkResponse> {
  const response = await requestAdmin(
    () =>
      updateNetwork({
        path: { network_key: networkKey },
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to update the Network',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Create a one-time Enrollment Token (design §4.5). The secret appears
 * only in the success response; the caller renders it exactly once. */
export async function createEnrollmentToken(
  csrfToken: string,
  expiresInHours: number,
): Promise<EnrollmentTokenResponse> {
  const response = await requestAdmin(
    () =>
      adminEnrollmentToken({
        body: { expiresInHours },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to create an enrollment token',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Create a one-time Recovery Token for an existing Agent (design §4.5).
 * Exchange advances the Agent Epoch without a duplicate Agent. */
export async function createRecoveryToken(
  agentId: string,
  csrfToken: string,
  expiresInHours: number,
): Promise<RecoveryTokenResponse> {
  const response = await requestAdmin(
    () =>
      adminRecoveryToken({
        path: { agent_id: agentId },
        body: { expiresInHours },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to create a recovery token',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Rotate an Agent credential: fresh credential with an explicit overlap
 * window and optional immediate old-credential revocation (design §12.6). */
export async function rotateAgentCredential(
  agentId: string,
  csrfToken: string,
  overlapHours: number,
  revokePrevious: boolean,
): Promise<RotationResponse> {
  const response = await requestAdmin(
    () =>
      adminRotateCredential({
        path: { agent_id: agentId },
        body: { overlapHours, revokePrevious },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to rotate the credential',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Revoke one Agent credential immediately (design §12.6). */
export async function revokeAgentCredential(
  agentId: string,
  credentialId: string,
  csrfToken: string,
): Promise<RevokeResponse> {
  const response = await requestAdmin(
    () =>
      adminRevokeCredential({
        path: { agent_id: agentId, credential_id: credentialId },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to revoke the credential',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Abort in-flight Admin requests and drop every cached Admin value that
 * belongs to an older access generation. No-op when the cache is already
 * valid for the current generation. */
export function resetAdminCache(generation: number): void {
  if (adminCacheGeneration === generation) return
  adminCacheGeneration = generation
  void adminQueryClient.cancelQueries({ queryKey: adminKeys.all })
  adminQueryClient.clear()
}

export type RealtimeStatus = 'connecting' | 'connected' | 'disconnected'
export type RealtimeState = { status: RealtimeStatus; online: boolean }

/**
 * One Admin SSE stream per shell per tab. Invalidations refetch the exact
 * Admin REST namespace (coalesced by the query cache); the Server's
 * access `reset` event (session expired/revoked/role-changed) or an
 * `auth_required` REST response triggers the access-generation sequence
 * instead. Reconnects re-open the stream under the current generation.
 */
export function useAdminRealtime(
  generation: number,
  onAccessReset: () => void,
): RealtimeState {
  const [status, setStatus] = useState<RealtimeStatus>('connecting')
  const [online, setOnline] = useState(() =>
    typeof navigator === 'undefined' ? true : navigator.onLine,
  )
  const accessReset = useRef(onAccessReset)
  accessReset.current = onAccessReset

  useEffect(() => {
    const handleOnline = () => setOnline(true)
    const handleOffline = () => setOnline(false)
    window.addEventListener('online', handleOnline)
    window.addEventListener('offline', handleOffline)
    return () => {
      window.removeEventListener('online', handleOnline)
      window.removeEventListener('offline', handleOffline)
    }
  }, [])

  useEffect(() => {
    if (typeof EventSource === 'undefined') return
    const events = new EventSource('/api/admin/v1/events')
    // The Server sends `event: reset` only when the session is no longer
    // current (expired, revoked, role-changed); that is an access-generation
    // transition. Every `invalidation` (including buffered replay resets)
    // only refetches the authoritative Admin REST namespace.
    const onReset = () => accessReset.current()
    const onInvalidation = () => {
      void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    }
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', onInvalidation)
    events.addEventListener('reset', onReset)
    return () => {
      events.removeEventListener('invalidation', onInvalidation)
      events.removeEventListener('reset', onReset)
      events.close()
    }
  }, [generation])

  return { status, online }
}
