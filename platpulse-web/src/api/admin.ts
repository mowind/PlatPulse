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
  auditList,
  alertIncidentDetail,
  alertIncidents,
  alertMaintenance,
  alertMaintenanceDetail,
  alertRuleDetail,
  alertRules,
  alertSilences,
  alertSilenceDetail,
  cancelMaintenanceWindow as cancelMaintenanceWindowApi,
  cancelNodeTransfer as cancelNodeTransferApi,
  cancelSilence as cancelSilenceApi,
  createMaintenanceWindow as createMaintenanceWindowApi,
  createNetwork,
  createSilence as createSilenceApi,
  deleteRuleOverride as deleteRuleOverrideApi,
  previewAlertRule as previewAlertRuleApi,
  updateAlertRule as updateAlertRuleApi,
  upsertRuleOverride as upsertRuleOverrideApi,
  createNodeTransfer as createNodeTransferApi,
  createPerson,
  diagnostics,
  getAccessSettings,
  overview,
  peopleList,
  resetPersonPassword,
  revokeOtherSessions,
  revokeSession,
  sessionsList,
  setAccessSettings,
  setNodeMetadata,
  setPersonRole,
  setPersonStatus,
  setVisibility,
  updateNetwork,
  type AccessSettingsResponse,
  type AdminNetwork,
  type AlertRuleDetail,
  type AlertRuleSummary,
  type AlertRuleUpdateRequest,
  type AlertRuleUpdateResponse,
  type IncidentDetail,
  type IncidentListResponse,
  type MaintenanceCreateRequest,
  type MaintenanceDto,
  type MaintenanceMutationResponse,
  notificationChannels,
  notificationChannelDetail,
  notificationDeliveries,
  notificationDeliveryDetail,
  notificationEventDetail,
  notificationEvents,
  retryDelivery,
  testNotificationChannel,
  type ChannelDto,
  type ChannelTestResponse,
  type DeliveryRetryResponse,
  type NotificationDeliveryDetail,
  type NotificationDeliveriesResponse,
  type NotificationEventDetail,
  type NotificationEventsResponse,
  type RuleOverrideResponse,
  type RuleOverrideUpsertRequest,
  type RulePreviewResponse,
  type SilenceCreateRequest,
  type SilenceDto,
  type SilenceMutationResponse,
  type AdminNetworkDetail,
  type AdminNodeDetail,
  type AdminNodeListItem,
  type AdminOverview,
  type AgentAuditResponse,
  type AgentDiagnostic,
  type ApiErrorBody,
  type AuditResponse,
  type CreatePersonRequest,
  type EnrollmentTokenResponse,
  type NetworkCreateRequest,
  type NetworkResponse,
  type NetworkUpdateRequest,
  type NodeMetadataRequest,
  type NodeMetadataResponse,
  type NodeTransfer,
  type NodeTransferCreateRequest,
  type NodeTransferMutationResponse,
  type PeopleResponse,
  type Person,
  type RecoveryTokenResponse,
  type ResetPasswordRequest,
  type RevokeOthersResponse,
  type RevokeResponse,
  type RevokeSessionResponse,
  type RotationResponse,
  type SessionsResponse,
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
  people: ['admin', 'people'] as const,
  sessions: ['admin', 'sessions'] as const,
  audit: (filters: AuditFilters) => ['admin', 'audit', filters] as const,
  access: ['admin', 'access'] as const,
  alertRules: ['admin', 'alerts', 'rules'] as const,
  alertRuleDetail: (ruleKey: string) => ['admin', 'alerts', 'rules', ruleKey] as const,
  alertIncidents: ['admin', 'alerts', 'incidents'] as const,
  alertIncidentDetail: (incidentId: string) =>
    ['admin', 'alerts', 'incidents', incidentId] as const,
  alertSilences: ['admin', 'alerts', 'silences'] as const,
  alertMaintenance: ['admin', 'alerts', 'maintenance'] as const,
  notificationEvents: ['admin', 'notifications', 'events'] as const,
  notificationEventDetail: (eventId: string) =>
    ['admin', 'notifications', 'events', eventId] as const,
  notificationDeliveries: (filters: DeliveryFilters) =>
    ['admin', 'notifications', 'deliveries', filters] as const,
  notificationDeliveryDetail: (deliveryId: string) =>
    ['admin', 'notifications', 'deliveries', deliveryId] as const,
  notificationChannels: ['admin', 'notifications', 'channels'] as const,
  notificationChannelDetail: (channelId: string) =>
    ['admin', 'notifications', 'channels', channelId] as const,
}

/** Immutable Audit listing filters (issue #47). */
export type AuditFilters = {
  eventKind?: string
  targetKind?: string
  before?: number
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

/** Owner-only People listing (issue #47): allowlisted rows with role,
 * disabled state, and active Session count — never passwords or hashes. */
export async function fetchAdminPeople(signal?: AbortSignal): Promise<PeopleResponse> {
  return requestAdmin(() => peopleList({ signal }), 'Unable to load People')
}

export function useAdminPeople(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.people, generation],
    queryFn: ({ signal }) => fetchAdminPeople(signal),
    placeholderData: keepPreviousData,
  })
}

/** Create a user. The password is hashed by the Server and never returned;
 * the response carries the allowlisted Person row only. */
export async function createPersonEntry(
  request: CreatePersonRequest,
  csrfToken: string,
): Promise<Person> {
  const response = await requestAdmin(
    () =>
      createPerson({
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to create the user',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Change a user's role. The Server revokes the user's Sessions when the
 * role changes; a changed own role ends this session on the next request. */
export async function changePersonRole(
  userId: string,
  role: string,
  csrfToken: string,
): Promise<Person> {
  const response = await requestAdmin(
    () =>
      setPersonRole({
        path: { user_id: userId },
        body: { role },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to change the role',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Enable or disable a user. Disabling revokes the user's Sessions
 * immediately; the final valid Owner is protected by the Server. */
export async function setPersonDisabled(
  userId: string,
  disabled: boolean,
  csrfToken: string,
): Promise<Person> {
  const response = await requestAdmin(
    () =>
      setPersonStatus({
        path: { user_id: userId },
        body: { disabled },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    disabled ? 'Unable to disable the user' : 'Unable to enable the user',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Reset a user's password; the Server revokes all of the user's Sessions
 * and never returns the password. */
export async function resetPersonPasswordEntry(
  userId: string,
  password: ResetPasswordRequest['password'],
  csrfToken: string,
): Promise<void> {
  await requestAdmin(
    () =>
      resetPersonPassword({
        path: { user_id: userId },
        body: { password },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to reset the password',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
}

/** Coarse, non-sensitive Session listing (issue #47). */
export async function fetchAdminSessions(signal?: AbortSignal): Promise<SessionsResponse> {
  return requestAdmin(() => sessionsList({ signal }), 'Unable to load Sessions')
}

export function useAdminSessions(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.sessions, generation],
    queryFn: ({ signal }) => fetchAdminSessions(signal),
    placeholderData: keepPreviousData,
  })
}

/** Revoke one Session. The affected user's streams close and their tabs
 * re-resolve authorization through the access-generation signal. */
export async function revokeSessionEntry(
  sessionId: string,
  csrfToken: string,
): Promise<RevokeSessionResponse> {
  const response = await requestAdmin(
    () =>
      revokeSession({
        path: { session_id: sessionId },
        // The revoke carries no parameters, but the declared JSON body
        // keeps every Admin mutation on the same trust boundary (design
        // §12.4): JSON content type plus the synchronizer CSRF header.
        body: {},
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to revoke the session',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Revoke every Session of the acting user except the current one. */
export async function revokeOtherSessionsEntry(csrfToken: string): Promise<RevokeOthersResponse> {
  const response = await requestAdmin(
    () =>
      revokeOtherSessions({
        body: {},
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to revoke the other sessions',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

/** Immutable redacted Audit listing (issue #47). */
export async function fetchAdminAudit(
  filters: AuditFilters,
  signal?: AbortSignal,
): Promise<AuditResponse> {
  return requestAdmin(
    () =>
      auditList({
        query: {
          limit: 50,
          event_kind: filters.eventKind,
          target_kind: filters.targetKind,
          before: filters.before,
        },
        signal,
      }),
    'Unable to load the Audit log',
  )
}

export function useAdminAudit(generation: number, filters: AuditFilters) {
  return useQuery({
    queryKey: [...adminKeys.audit(filters), generation],
    queryFn: ({ signal }) => fetchAdminAudit(filters, signal),
    placeholderData: keepPreviousData,
  })
}

/** Owner-only read of the anonymous Home (Guest) setting. */
export async function fetchAdminAccess(signal?: AbortSignal): Promise<AccessSettingsResponse> {
  return requestAdmin(() => getAccessSettings({ signal }), 'Unable to load access settings')
}

export function useAdminAccess(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.access, generation],
    queryFn: ({ signal }) => fetchAdminAccess(signal),
    placeholderData: keepPreviousData,
  })
}

/** Toggle anonymous Home (Guest) access. The Server closes open Guest
 * streams on disable and publishes a Public collection reset in both
 * directions. */
export async function updateAccessSettings(
  guestEnabled: boolean,
  csrfToken: string,
): Promise<AccessSettingsResponse> {
  const response = await requestAdmin(
    () =>
      setAccessSettings({
        body: { guestEnabled },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    guestEnabled ? 'Unable to enable anonymous Home' : 'Unable to disable anonymous Home',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
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

// ---------------------------------------------------------------------------
// Alerts (issue #48): typed Rules, evaluation state, Incidents, Silence and
// Maintenance. REST is authoritative; mutations never optimistically mark
// success and always invalidate the Admin cache (webui.md §6.4).
// ---------------------------------------------------------------------------

/** Owner-only typed Alert Rule catalog with per-rule evaluation summaries. */
export async function fetchAdminAlertRules(signal?: AbortSignal): Promise<AlertRuleSummary[]> {
  return requestAdmin(() => alertRules({ signal }), 'Unable to load the Alert Rules')
}

export function useAdminAlertRules(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.alertRules, generation],
    queryFn: ({ signal }) => fetchAdminAlertRules(signal),
    placeholderData: keepPreviousData,
  })
}

/** One Rule with immutable versions, overrides, and per-subject evaluation
 * state. No placeholder: another Rule's DTO must never render under this
 * Rule's URL. */
export async function fetchAdminAlertRule(
  ruleKey: string,
  signal?: AbortSignal,
): Promise<AlertRuleDetail> {
  return requestAdmin(
    () => alertRuleDetail({ path: { rule_key: ruleKey }, signal }),
    'Unable to load the Alert Rule',
  )
}

export function useAdminAlertRuleDetail(generation: number, ruleKey: string) {
  return useQuery({
    queryKey: [...adminKeys.alertRuleDetail(ruleKey), generation],
    queryFn: ({ signal }) => fetchAdminAlertRule(ruleKey, signal),
    enabled: ruleKey.length > 0,
  })
}

/** Edit a typed Rule: immutable version bump, Audit row, and authoritative
 * refetch. Disabling stops new evaluation without deleting history. */
export async function updateAlertRuleEntry(
  ruleKey: string,
  request: AlertRuleUpdateRequest,
  csrfToken: string,
): Promise<AlertRuleUpdateResponse> {
  try {
    const response = await requestAdmin(
      () =>
        updateAlertRuleApi({
          path: { rule_key: ruleKey },
          body: request,
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to update the Alert Rule',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Read-only preview: evaluates the rule (optionally with an unsaved draft)
 * against current facts without creating Incidents, Notifications, or state
 * rows. Never invalidates anything. */
export async function previewAlertRuleEntry(
  ruleKey: string,
  request: AlertRuleUpdateRequest,
  csrfToken: string,
  signal?: AbortSignal,
): Promise<RulePreviewResponse> {
  return requestAdmin(
    () =>
      previewAlertRuleApi({
        path: { rule_key: ruleKey },
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
        signal,
      }),
    'Unable to preview the Alert Rule',
  )
}

/** Upsert a Network/Node override (audited). */
export async function upsertRuleOverrideEntry(
  ruleKey: string,
  request: RuleOverrideUpsertRequest,
  csrfToken: string,
): Promise<RuleOverrideResponse> {
  try {
    const response = await requestAdmin(
      () =>
        upsertRuleOverrideApi({
          path: { rule_key: ruleKey },
          body: request,
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to save the Rule override',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Remove one Network/Node override (audited). */
export async function deleteRuleOverrideEntry(
  ruleKey: string,
  scopeKind: string,
  scopeValue: string,
  csrfToken: string,
): Promise<RuleOverrideResponse> {
  try {
    const response = await requestAdmin(
      () =>
        deleteRuleOverrideApi({
          path: { rule_key: ruleKey, scope_kind: scopeKind, scope_value: scopeValue },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to remove the Rule override',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

export type IncidentFilters = {
  state?: string
  severity?: string
  ruleKey?: string
  subjectKind?: string
  limit?: number
}

/** Owner-only Incident history. Incidents are never manually resolvable,
 * reopenable, or deletable; the list carries Server-owned state and
 * evidence references. */
export async function fetchAdminIncidents(
  filters: IncidentFilters,
  signal?: AbortSignal,
): Promise<IncidentListResponse> {
  return requestAdmin(
    () =>
      alertIncidents({
        query: {
          state: filters.state,
          severity: filters.severity,
          rule_key: filters.ruleKey,
          subject_kind: filters.subjectKind,
          limit: filters.limit,
        },
        signal,
      }),
    'Unable to load the Incidents',
  )
}

export function useAdminIncidents(generation: number, filters: IncidentFilters) {
  return useQuery({
    queryKey: [...adminKeys.alertIncidents, filters, generation],
    queryFn: ({ signal }) => fetchAdminIncidents(filters, signal),
    placeholderData: keepPreviousData,
  })
}

export async function fetchAdminIncident(
  incidentId: string,
  signal?: AbortSignal,
): Promise<IncidentDetail> {
  return requestAdmin(
    () => alertIncidentDetail({ path: { incident_id: incidentId }, signal }),
    'Unable to load the Incident',
  )
}

export function useAdminIncidentDetail(generation: number, incidentId: string) {
  return useQuery({
    queryKey: [...adminKeys.alertIncidentDetail(incidentId), generation],
    queryFn: ({ signal }) => fetchAdminIncident(incidentId, signal),
    // No placeholder: one Incident's DTO must never render under another.
    enabled: incidentId.length > 0,
  })
}

export type SilenceFilters = { status?: string }

export async function fetchAdminSilences(
  filters: SilenceFilters,
  signal?: AbortSignal,
): Promise<SilenceDto[]> {
  const response = await requestAdmin(
    () => alertSilences({ query: { status: filters.status }, signal }),
    'Unable to load the Silences',
  )
  return response.silences
}

export function useAdminSilences(generation: number, filters: SilenceFilters) {
  return useQuery({
    queryKey: [...adminKeys.alertSilences, filters, generation],
    queryFn: ({ signal }) => fetchAdminSilences(filters, signal),
    placeholderData: keepPreviousData,
  })
}

/** Create a time-bounded Silence. It suppresses delivery only; evaluation
 * and Incidents are untouched. */
export async function createSilenceEntry(
  request: SilenceCreateRequest,
  csrfToken: string,
): Promise<SilenceMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        createSilenceApi({
          body: request,
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to create the Silence',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Cancel an active Silence (audited, irreversible). */
export async function cancelSilenceEntry(
  silenceId: string,
  csrfToken: string,
): Promise<SilenceMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        cancelSilenceApi({
          path: { silence_id: silenceId },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to cancel the Silence',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

export type MaintenanceFilters = { status?: string }

export async function fetchAdminMaintenance(
  filters: MaintenanceFilters,
  signal?: AbortSignal,
): Promise<MaintenanceDto[]> {
  const response = await requestAdmin(
    () => alertMaintenance({ query: { status: filters.status }, signal }),
    'Unable to load the Maintenance Windows',
  )
  return response.windows
}

export function useAdminMaintenance(generation: number, filters: MaintenanceFilters) {
  return useQuery({
    queryKey: [...adminKeys.alertMaintenance, filters, generation],
    queryFn: ({ signal }) => fetchAdminMaintenance(filters, signal),
    placeholderData: keepPreviousData,
  })
}

/** Create a time-bounded Maintenance Window for an Agent, Node, or Network
 * scope with a typed expected-condition allowlist. */
export async function createMaintenanceEntry(
  request: MaintenanceCreateRequest,
  csrfToken: string,
): Promise<MaintenanceMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        createMaintenanceWindowApi({
          body: request,
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to create the Maintenance Window',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Cancel an active Maintenance Window (audited, irreversible). */
export async function cancelMaintenanceEntry(
  windowId: string,
  csrfToken: string,
): Promise<MaintenanceMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        cancelMaintenanceWindowApi({
          path: { window_id: windowId },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to cancel the Maintenance Window',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Owner-only read of one Silence (detail page). */
export async function fetchAdminSilence(
  silenceId: string,
  signal?: AbortSignal,
): Promise<SilenceDto> {
  return requestAdmin(
    () => alertSilenceDetail({ path: { silence_id: silenceId }, signal }),
    'Unable to load the Silence',
  )
}

/** Owner-only read of one Maintenance Window (detail page). */
export async function fetchAdminMaintenanceDetail(
  windowId: string,
  signal?: AbortSignal,
): Promise<MaintenanceDto> {
  return requestAdmin(
    () => alertMaintenanceDetail({ path: { window_id: windowId }, signal }),
    'Unable to load the Maintenance Window',
  )
}

// ---------------------------------------------------------------------------
// Notifications (issue #49): durable Notification Events, per-channel
// Delivery attempts with bounded retry/backoff, Retry-After, DeadLetter,
// manual retry (never a duplicate Event), and redacted channel policy.
// REST is authoritative; mutations never optimistically mark success and
// always invalidate the Admin cache (webui.md §6.4).
// ---------------------------------------------------------------------------

/** Outbox listing filters (issue #49). */
export type DeliveryFilters = {
  state?: string
  channel?: string
  before?: string
  limit?: number
}

export type NotificationEventsFilters = {
  eventKind?: string
  before?: string
  limit?: number
}

/** Notification Events with their per-channel Delivery summaries. */
export async function fetchAdminNotificationEvents(
  filters: NotificationEventsFilters,
  signal?: AbortSignal,
): Promise<NotificationEventsResponse> {
  return requestAdmin(
    () =>
      notificationEvents({
        query: {
          event_kind: filters.eventKind,
          before: filters.before,
          limit: filters.limit,
        },
        signal,
      }),
    'Unable to load the Notification Events',
  )
}

export function useAdminNotificationEvents(
  generation: number,
  filters: NotificationEventsFilters,
) {
  return useQuery({
    queryKey: [...adminKeys.notificationEvents, filters, generation],
    queryFn: ({ signal }) => fetchAdminNotificationEvents(filters, signal),
    placeholderData: keepPreviousData,
  })
}

/** One Notification Event with full Deliveries. */
export async function fetchAdminNotificationEvent(
  eventId: string,
  signal?: AbortSignal,
): Promise<NotificationEventDetail> {
  return requestAdmin(
    () => notificationEventDetail({ path: { event_id: eventId }, signal }),
    'Unable to load the Notification Event',
  )
}

export function useAdminNotificationEvent(generation: number, eventId: string) {
  return useQuery({
    queryKey: [...adminKeys.notificationEventDetail(eventId), generation],
    queryFn: ({ signal }) => fetchAdminNotificationEvent(eventId, signal),
    enabled: eventId.length > 0,
  })
}

/** Outbox rows (the Delivery list) with retry/dead-letter filters. */
export async function fetchAdminDeliveries(
  filters: DeliveryFilters,
  signal?: AbortSignal,
): Promise<NotificationDeliveriesResponse> {
  return requestAdmin(
    () =>
      notificationDeliveries({
        query: {
          state: filters.state,
          channel: filters.channel,
          before: filters.before,
          limit: filters.limit,
        },
        signal,
      }),
    'Unable to load the Notification Deliveries',
  )
}

export function useAdminDeliveries(generation: number, filters: DeliveryFilters) {
  return useQuery({
    queryKey: [...adminKeys.notificationDeliveries(filters), generation],
    queryFn: ({ signal }) => fetchAdminDeliveries(filters, signal),
    placeholderData: keepPreviousData,
  })
}

/** One Delivery with attempt history, provider results, Retry-After, and
 * DeadLetter outcome. No placeholder: one Delivery's DTO must never render
 * under another Delivery's URL. */
export async function fetchAdminDelivery(
  deliveryId: string,
  signal?: AbortSignal,
): Promise<NotificationDeliveryDetail> {
  return requestAdmin(
    () => notificationDeliveryDetail({ path: { delivery_id: deliveryId }, signal }),
    'Unable to load the Notification Delivery',
  )
}

export function useAdminDeliveryDetail(generation: number, deliveryId: string) {
  return useQuery({
    queryKey: [...adminKeys.notificationDeliveryDetail(deliveryId), generation],
    queryFn: ({ signal }) => fetchAdminDelivery(deliveryId, signal),
    enabled: deliveryId.length > 0,
  })
}

/** Configured channels with redacted destinations and provider refs. */
export async function fetchAdminChannels(signal?: AbortSignal): Promise<ChannelDto[]> {
  return requestAdmin(() => notificationChannels({ signal }), 'Unable to load the channels')
}

export function useAdminChannels(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.notificationChannels, generation],
    queryFn: ({ signal }) => fetchAdminChannels(signal),
    placeholderData: keepPreviousData,
  })
}

/** One configured channel (policy + redacted destination). */
export async function fetchAdminChannel(
  channelId: string,
  signal?: AbortSignal,
): Promise<ChannelDto> {
  return requestAdmin(
    () => notificationChannelDetail({ path: { channel_id: channelId }, signal }),
    'Unable to load the channel',
  )
}

export function useAdminChannelDetail(generation: number, channelId: string) {
  return useQuery({
    queryKey: [...adminKeys.notificationChannelDetail(channelId), generation],
    queryFn: ({ signal }) => fetchAdminChannel(channelId, signal),
    enabled: channelId.length > 0,
  })
}

/** Manual retry: re-arms one Delivery (new attempt on the next worker pass,
 * never a duplicate Event/Incident/transition). Duplicate parallel retries
 * and non-retryable states are typed 409s; the failure path refetches
 * authoritative state. */
export async function retryDeliveryEntry(
  deliveryId: string,
  csrfToken: string,
): Promise<DeliveryRetryResponse> {
  try {
    const response = await requestAdmin(
      () =>
        retryDelivery({
          path: { delivery_id: deliveryId },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to retry the Delivery',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    // The Delivery may already be queued (parallel retry) or resolved on
    // the Server; reload authoritative state.
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Owner test notification: creates a `test` Event (clearly separate from
 * business Incidents), sends synchronously, audits, and refetches. */
export async function testNotificationChannelEntry(
  channelId: string,
  csrfToken: string,
): Promise<ChannelTestResponse> {
  try {
    const response = await requestAdmin(
      () =>
        testNotificationChannel({
          path: { channel_id: channelId },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to send the test notification',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}
