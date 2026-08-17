// Typed Admin adapter and query/realtime layer (design §6.1–§6.3).
//
// The generated OpenAPI client is imported through the project client
// singleton; this module owns request normalization, the dedicated Admin
// query cache (structurally isolated from Public queries), and the
// Admin-specific SSE invalidation stream. REST remains authoritative: SSE
// carries invalidation/reset signals only, and every invalidation refetches
// the matching Admin REST resource through the cache.

import { QueryClient, useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import {
  adminAgentAudit,
  adminGeoStatus,
  adminAgentDetail,
  adminEnrollmentToken,
  adminNetworkDetail,
  adminNetworks,
  adminNodeDetail,
  adminNodePeerChurn,
  adminNodePeerHistory,
  adminNodeTransfers,
  adminNodes,
  adminRecoveryToken,
  adminValidatorDetail,
  adminValidatorLinks,
  adminValidators,
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
  createNodeValidatorLink,
  createValidator,
  createSilence as createSilenceApi,
  deleteRuleOverride as deleteRuleOverrideApi,
  endValidatorLink,
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
  updateValidatorLink,
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
  backupArtifactDetail,
  backupCreate as backupCreateApi,
  backupVerify as backupVerifyApi,
  backupsList,
  cancelOperation as cancelOperationApi,
  doctorOverview,
  doctorRun as doctorRunApi,
  operationDetail,
  operationsList,
  restoreSubmit as restoreSubmitApi,
  restoreValidate as restoreValidateApi,
  retentionImpact as retentionImpactApi,
  retentionOverview,
  retentionRun as retentionRunApi,
  updateRetentionPolicy as updateRetentionPolicyApi,
  type BackupArtifactDetail as BackupArtifactDetailDto,
  type BackupArtifactSummary,
  type DoctorOverview,
  type OperationDetail,
  type OperationMutationResponse,
  type OperationSummary,
  type RestoreValidation,
  type RetentionOverview,
  type RetentionPolicyDto,
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
  type PeerChurnDiagnostic,
  type AdminPeerHistory,
  type AdminNodeListItem,
  type AdminOverview,
  type AgentAuditResponse,
  type GeoStatusDiagnostic,
  type AgentDiagnostic,
  type ApiErrorBody,
  type AuditResponse,
  type CreatePersonRequest,
  type EnrollmentTokenResponse,
  type NetworkCreateRequest,
  type NodeValidatorLink,
  type Validator,
  type ValidatorCreateRequest,
  type ValidatorDetail,
  type ValidatorLinkCreateRequest,
  type ValidatorLinkEndRequest,
  type ValidatorLinkMutationResponse,
  type ValidatorLinkUpdateRequest,
  type ValidatorMutationResponse,
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
  geo: ['admin', 'geo'] as const,
  agents: ['admin', 'agents'] as const,
  agentDetail: (agentId: string) => ['admin', 'agents', agentId] as const,
  agentAudit: (agentId: string) => ['admin', 'agents', agentId, 'audit'] as const,
  nodes: ['admin', 'nodes'] as const,
  nodeDetail: (nodeId: string) => ['admin', 'nodes', nodeId] as const,
  nodePeerChurn: (nodeId: string) => ['admin', 'nodes', nodeId, 'peer-churn'] as const,
  nodePeerHistory: (nodeId: string) => ['admin', 'nodes', nodeId, 'peer-history'] as const,
  nodeTransfers: (nodeId: string) => ['admin', 'nodes', nodeId, 'transfers'] as const,
  nodeValidatorLinks: (nodeId: string) => ['admin', 'nodes', nodeId, 'validator-links'] as const,
  validators: ['admin', 'validators'] as const,
  validatorDetail: (validatorId: string) => ['admin', 'validators', validatorId] as const,
  validatorLinks: ['admin', 'validator-links'] as const,
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
  notificationRoot: ['admin', 'notifications'] as const,
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
  operationsRoot: ['admin', 'operations'] as const,
  operations: (filters: OperationFilters) => ['admin', 'operations', filters] as const,
  operationDetail: (operationId: string) =>
    ['admin', 'operations', operationId] as const,
  retention: ['admin', 'retention'] as const,
  retentionImpact: (family: string, days: number) =>
    ['admin', 'retention', 'impact', family, days] as const,
  backups: ['admin', 'backups'] as const,
  backupDetail: (artifactId: string) => ['admin', 'backups', artifactId] as const,
  restoreValidate: (artifactId: string) =>
    ['admin', 'restore', 'validate', artifactId] as const,
  doctor: ['admin', 'doctor'] as const,
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
const adminRealtimeClosers = new Set<() => void>()

export function subscribeAdminAccessReset(listener: AccessResetListener): () => void {
  accessResetListeners.add(listener)
  return () => accessResetListeners.delete(listener)
}

function notifyAdminAccessReset(): void {
  for (const close of adminRealtimeClosers) close()
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

export async function fetchAdminGeoStatus(signal?: AbortSignal): Promise<GeoStatusDiagnostic> {
  return requestAdmin(
    () => adminGeoStatus({ signal }),
    'Unable to load Geo database status',
  )
}

export function useAdminGeoStatus(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.geo, generation],
    queryFn: ({ signal }) => fetchAdminGeoStatus(signal),
  })
}


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
  })
}

export function useAdminDiagnostics(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.diagnostics, generation],
    queryFn: ({ signal }) => fetchAdminDiagnostics(signal),
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
    enabled: agentId.length > 0,
  })
}

export function useAdminAgentAudit(generation: number, agentId: string) {
  return useQuery({
    queryKey: [...adminKeys.agentAudit(agentId), generation],
    queryFn: ({ signal }) => fetchAdminAgentAudit(agentId, signal),
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

export function useAdminNodePeerChurn(generation: number, nodeId: string) {
  return useQuery({
    queryKey: [...adminKeys.nodePeerChurn(nodeId), generation],
    queryFn: ({ signal }) => fetchAdminNodePeerChurn(nodeId, signal),
    // A different Node's churn history must never render under this Node.
    enabled: nodeId.length > 0,
  })
}

export async function fetchAdminNodePeerChurn(
  nodeId: string,
  signal?: AbortSignal,
): Promise<PeerChurnDiagnostic> {
  return requestAdmin(
    () => adminNodePeerChurn({ path: { node_id: nodeId }, signal }),
    'Unable to load Peer churn',
  )
}
export function useAdminNodePeerHistory(generation: number, nodeId: string) {
  return useQuery({
    queryKey: [...adminKeys.nodePeerHistory(nodeId), generation],
    queryFn: ({ signal }) => fetchAdminNodePeerHistory(nodeId, signal),
    enabled: nodeId.length > 0,
  })
}

export async function fetchAdminNodePeerHistory(
  nodeId: string,
  signal?: AbortSignal,
): Promise<AdminPeerHistory> {
  return requestAdmin(
    () => adminNodePeerHistory({ path: { node_id: nodeId }, signal }),
    'Unable to load Peer history',
  )
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
export async function fetchAdminValidators(
  networkKey?: string,
  signal?: AbortSignal,
): Promise<Validator[]> {
  return requestAdmin(
    () => adminValidators({ query: networkKey ? { networkKey } : undefined, signal }),
    'Unable to load Validators',
  )
}

export function useAdminValidators(generation: number, networkKey?: string) {
  return useQuery({
    queryKey: [...adminKeys.validators, networkKey ?? null, generation],
    queryFn: ({ signal }) => fetchAdminValidators(networkKey, signal),
  })
}

export async function fetchAdminValidator(
  validatorId: string,
  signal?: AbortSignal,
): Promise<ValidatorDetail> {
  return requestAdmin(
    () => adminValidatorDetail({ path: { validator_id: validatorId }, signal }),
    'Unable to load the Validator',
  )
}

export function useAdminValidatorDetail(generation: number, validatorId: string) {
  return useQuery({
    queryKey: [...adminKeys.validatorDetail(validatorId), generation],
    queryFn: ({ signal }) => fetchAdminValidator(validatorId, signal),
    enabled: validatorId.length > 0,
  })
}

export async function fetchAdminValidatorLinks(
  filters: { networkKey?: string; validatorId?: string; nodeId?: string } = {},
  signal?: AbortSignal,
): Promise<NodeValidatorLink[]> {
  return requestAdmin(
    () => adminValidatorLinks({ query: filters, signal }),
    'Unable to load Node Validator Links',
  )
}

export function useAdminValidatorLinks(
  generation: number,
  filters: { networkKey?: string; validatorId?: string; nodeId?: string } = {},
) {
  return useQuery({
    queryKey: [...adminKeys.validatorLinks, filters, generation],
    queryFn: ({ signal }) => fetchAdminValidatorLinks(filters, signal),
  })
}

export async function registerValidator(
  networkKey: string,
  request: ValidatorCreateRequest,
  csrfToken: string,
): Promise<ValidatorMutationResponse> {
  const response = await requestAdmin(
    () =>
      createValidator({
        path: { network_key: networkKey },
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to register the Validator',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

export async function createValidatorLink(
  nodeId: string,
  request: ValidatorLinkCreateRequest,
  csrfToken: string,
): Promise<ValidatorLinkMutationResponse> {
  const response = await requestAdmin(
    () =>
      createNodeValidatorLink({
        path: { node_id: nodeId },
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to create the Node Validator Link',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

export async function editValidatorLink(
  linkId: string,
  request: ValidatorLinkUpdateRequest,
  csrfToken: string,
): Promise<ValidatorLinkMutationResponse> {
  const response = await requestAdmin(
    () =>
      updateValidatorLink({
        path: { link_id: linkId },
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to update the Node Validator Link',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
}

export async function endValidator(
  linkId: string,
  request: ValidatorLinkEndRequest,
  csrfToken: string,
): Promise<ValidatorLinkMutationResponse> {
  const response = await requestAdmin(
    () =>
      endValidatorLink({
        path: { link_id: linkId },
        body: request,
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to end the Node Validator Link',
  )
  void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  return response
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

function invalidateAdminResource(resource: string, resourceId?: string): void {
  const keys: Array<readonly unknown[]> = (() => {
    switch (resource) {
      case 'node':
        return [adminKeys.overview, adminKeys.diagnostics, adminKeys.nodes, adminKeys.networks]
      case 'peer':
        return [adminKeys.overview, adminKeys.nodes, adminKeys.networks]
      case 'geo':
        return [adminKeys.geo, adminKeys.nodes, adminKeys.networks]
      case 'network':
        return [adminKeys.networks, adminKeys.validators, adminKeys.validatorLinks]
      case 'validator':
      case 'validator-link':
        return [adminKeys.validators, adminKeys.validatorLinks, adminKeys.nodes]
      case 'access':
        return [adminKeys.access, adminKeys.people, adminKeys.sessions]
      case 'alerts':
        return [
          adminKeys.overview,
          adminKeys.alertRules,
          adminKeys.alertIncidents,
          adminKeys.alertSilences,
          adminKeys.alertMaintenance,
        ]
      case 'notifications':
        return [adminKeys.notificationRoot]
      case 'operations':
        return [adminKeys.overview, adminKeys.operationsRoot]
      case 'retention':
        return [adminKeys.retention]
      case 'backups':
        return [adminKeys.backups]
      case 'doctor':
        return [adminKeys.doctor]
      default:
        return [adminKeys.all]
    }
  })()
  if (resourceId && resource === 'node') {
    keys.push(adminKeys.nodeDetail(resourceId), adminKeys.nodePeerChurn(resourceId), adminKeys.nodePeerHistory(resourceId), adminKeys.nodeTransfers(resourceId))
  }
  if (resourceId && resource === 'network') keys.push(adminKeys.networkDetail(resourceId))
  if (resourceId && resource === 'validator') keys.push(adminKeys.validatorDetail(resourceId))
  void Promise.all(keys.map((queryKey) => adminQueryClient.invalidateQueries({ queryKey })))
}

function handleAdminInvalidation(data: string): void {
  try {
    const event = JSON.parse(data) as { resource?: unknown; resourceId?: unknown; reset?: unknown }
    if (event.reset === true) {
      void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
      return
    }
    const resource = typeof event.resource === 'string' ? event.resource : 'collection'
    const resourceId = typeof event.resourceId === 'string' ? event.resourceId : undefined
    invalidateAdminResource(resource, resourceId)
  } catch {
    // A malformed signal is never trusted as data; refetch the bounded Admin
    // namespace instead of allowing a stale sensitive panel to persist.
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
  }
}



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
    const closeStream = () => events.close()
    adminRealtimeClosers.add(closeStream)
    // The Server sends `event: reset` only when the session is no longer
    // current (expired, revoked, role-changed); that is an access-generation
    // transition. Every `invalidation` (including buffered replay resets)
    // only refetches the authoritative Admin REST namespace.
    const onReset = () => {
      events.close()
      accessReset.current()
    }
    const onInvalidation = (event: Event) => {
      const message = event as MessageEvent<string>
      handleAdminInvalidation(message.data)
    }
    events.onopen = () => setStatus('connected')
    events.onerror = () => setStatus('disconnected')
    events.addEventListener('invalidation', onInvalidation)
    events.addEventListener('reset', onReset)
    return () => {
      adminRealtimeClosers.delete(closeStream)
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

// ---------------------------------------------------------------------------
// Operations, retention, backup, and Doctor (issue #50): durable, REST-
// authoritative Operations with progress, warnings, errors, request ID, and
// Audit links. Every mutation returns an Operation reference immediately
// and invalidates the Admin cache; SSE only accelerates refetch.
// ---------------------------------------------------------------------------

/** Immutable Operation listing filters. */
export type OperationFilters = {
  status?: string
  kind?: string
}

/** Fixed display labels for Operation statuses (webui.md §5.5). */
export function operationStatusLabel(status: string | null | undefined): string {
  switch (status) {
    case 'queued':
      return 'Queued'
    case 'running':
      return 'Running'
    case 'succeeded':
      return 'Succeeded'
    case 'succeeded_with_warnings':
      return 'Succeeded with warnings'
    case 'failed':
      return 'Failed'
    case 'cancelled':
      return 'Cancelled'
    default:
      return 'Unknown'
  }
}

/** Fixed display labels for Operation kinds. */
export function operationKindLabel(kind: string | null | undefined): string {
  switch (kind) {
    case 'retention_run':
      return 'Retention run'
    case 'backup_create':
      return 'Backup creation'
    case 'backup_verify':
      return 'Backup verification'
    case 'doctor_run':
      return 'Doctor'
    case 'restore':
      return 'Restore'
    default:
      return kind ?? 'Unknown'
  }
}

export async function fetchAdminOperations(
  filters: OperationFilters,
  signal?: AbortSignal,
): Promise<OperationSummary[]> {
  return requestAdmin(
    () =>
      operationsList({
        query: {
          status: filters.status || undefined,
          kind: filters.kind || undefined,
        },
        signal,
      }),
    'Unable to load Operations',
  )
}

export function useAdminOperations(generation: number, filters: OperationFilters) {
  return useQuery({
    queryKey: [...adminKeys.operations(filters), generation],
    queryFn: ({ signal }) => fetchAdminOperations(filters, signal),
  })
}

/** Operation detail: no placeholder — one Operation's DTO must never
 * render under another Operation's URL. */
export async function fetchAdminOperation(
  operationId: string,
  signal?: AbortSignal,
): Promise<OperationDetail> {
  return requestAdmin(
    () => operationDetail({ path: { operation_id: operationId }, signal }),
    'Unable to load the Operation',
  )
}

export function useAdminOperation(generation: number, operationId: string) {
  return useQuery({
    queryKey: [...adminKeys.operationDetail(operationId), generation],
    queryFn: ({ signal }) => fetchAdminOperation(operationId, signal),
    enabled: operationId.length > 0,
  })
}

/** Cancel a queued or running Operation. Confirmed and audited; refetches
 * authoritative state (no optimistic update). */
export async function cancelOperationEntry(
  operationId: string,
  csrfToken: string,
): Promise<OperationMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        cancelOperationApi({
          path: { operation_id: operationId },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to cancel the Operation',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    // The Operation may already be terminal on the Server; reload so the
    // detail shows the true outcome.
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Retention policies with safety bounds, protected state, last run. */
export async function fetchAdminRetention(signal?: AbortSignal): Promise<RetentionOverview> {
  return requestAdmin(
    () => retentionOverview({ signal }),
    'Unable to load retention policies',
  )
}

export function useAdminRetention(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.retention, generation],
    queryFn: ({ signal }) => fetchAdminRetention(signal),
  })
}

/** Read-only impact preview: never writes and never audits. The edit form
 * calls this before typed confirmation (webui.md §8.4). It is still a
 * CSRF-guarded POST (JSON body), so it carries the session token. */
export function useRetentionImpact(generation: number, family: string, days: number, csrfToken: string) {
  return useQuery({
    queryKey: [...adminKeys.retentionImpact(family, days), generation],
    queryFn: ({ signal }) =>
      requestAdmin(
        () =>
          retentionImpactApi({
            body: { family, retentionDays: days },
            headers: { 'X-CSRF-Token': csrfToken },
            signal,
          }),
        'Unable to preview the retention impact',
      ),
    enabled: family.length > 0 && days >= 0 && csrfToken.length > 0,
  })
}

/** Apply a retention policy change within its fixed safety bounds.
 * Confirmed, audited, refetched — never optimistic. */
export async function updateRetentionPolicyEntry(
  family: string,
  retentionDays: number,
  csrfToken: string,
): Promise<{ policy: RetentionPolicyDto; auditEventId: number }> {
  try {
    const response = await requestAdmin(
      () =>
        updateRetentionPolicyApi({
          path: { family },
          body: { retentionDays },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to update the retention policy',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Queue a retention run; returns the Operation reference immediately. */
export async function runRetentionEntry(
  families: string[] | null,
  csrfToken: string,
): Promise<OperationMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        retentionRunApi({
          body: { families: families ?? undefined },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to start the retention run',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Backup artifact list: sanitized metadata only (no paths, no contents). */
export async function fetchAdminBackups(signal?: AbortSignal): Promise<BackupArtifactSummary[]> {
  return requestAdmin(
    () => backupsList({ signal }),
    'Unable to load backup artifacts',
  )
}

export function useAdminBackups(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.backups, generation],
    queryFn: ({ signal }) => fetchAdminBackups(signal),
  })
}

export async function fetchAdminBackup(
  artifactId: string,
  signal?: AbortSignal,
): Promise<BackupArtifactDetailDto> {
  return requestAdmin(
    () => backupArtifactDetail({ path: { artifact_id: artifactId }, signal }),
    'Unable to load the backup artifact',
  )
}

export function useAdminBackup(generation: number, artifactId: string) {
  return useQuery({
    queryKey: [...adminKeys.backupDetail(artifactId), generation],
    queryFn: ({ signal }) => fetchAdminBackup(artifactId, signal),
    enabled: artifactId.length > 0,
  })
}

/** Queue a backup creation (typed confirmation happens in the page). */
export async function createBackupEntry(csrfToken: string): Promise<OperationMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        backupCreateApi({
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to start the backup',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Queue a backup verification (checksum, read-only integrity, schema). */
export async function verifyBackupEntry(
  artifactId: string,
  csrfToken: string,
): Promise<OperationMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        backupVerifyApi({
          path: { artifact_id: artifactId },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to start the backup verification',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/**
 * Read-only Restore validation (issue #51, webui.md §8.4): fresh
 * checksum, integrity, and schema compatibility of the artifact file,
 * computed by the Server before any typed confirmation. Never writes and
 * never audits.
 */
export async function validateRestoreEntry(
  artifactId: string,
  csrfToken: string,
): Promise<RestoreValidation> {
  return requestAdmin(
    () =>
      restoreValidateApi({
        body: { artifactId },
        headers: { 'X-CSRF-Token': csrfToken },
      }),
    'Unable to validate the backup for restore',
  )
}

/** Restore validation outcome for the selected artifact (mutation-style
 * POST, so it is CSRF-guarded; the page calls it on demand). */
export function useRestoreValidation(
  generation: number,
  artifactId: string,
  csrfToken: string,
  enabled: boolean,
) {
  return useQuery({
    queryKey: [...adminKeys.restoreValidate(artifactId), generation],
    queryFn: ({ signal }) =>
      requestAdmin(
        () =>
          restoreValidateApi({
            body: { artifactId },
            headers: { 'X-CSRF-Token': csrfToken },
            signal,
          }),
        'Unable to validate the backup for restore',
      ),
    enabled: artifactId.length > 0 && csrfToken.length > 0 && enabled,
  })
}

/**
 * Queue the highest-risk Restore Operation. The typed confirmation must
 * equal the selected backup file name; the Server re-validates everything
 * in the worker and then refuses while the Server is running
 * (`restore_requires_stopped_server`), preserving the current database.
 */
export async function submitRestoreEntry(
  artifactId: string,
  confirmation: string,
  csrfToken: string,
): Promise<OperationMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        restoreSubmitApi({
          body: { artifactId, confirmation },
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to start the Restore',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** The most recent read-only Doctor report and its checks. */
export async function fetchAdminDoctor(signal?: AbortSignal): Promise<DoctorOverview> {
  return requestAdmin(
    () => doctorOverview({ signal }),
    'Unable to load the Doctor report',
  )
}

export function useAdminDoctor(generation: number) {
  return useQuery({
    queryKey: [...adminKeys.doctor, generation],
    queryFn: ({ signal }) => fetchAdminDoctor(signal),
  })
}

/** Queue a read-only Doctor run. Doctor never auto-fixes, deletes,
 * migrates, or rotates secrets. */
export async function runDoctorEntry(csrfToken: string): Promise<OperationMutationResponse> {
  try {
    const response = await requestAdmin(
      () =>
        doctorRunApi({
          headers: { 'X-CSRF-Token': csrfToken },
        }),
      'Unable to start the Doctor run',
    )
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    return response
  } catch (error) {
    void adminQueryClient.invalidateQueries({ queryKey: adminKeys.all })
    throw error
  }
}

/** Doctor check status vocabulary: Server-owned words shown as sent. */
export function doctorCheckStatusLabel(status: string | null | undefined): string {
  switch (status) {
    case 'pass':
      return 'Pass'
    case 'warning':
      return 'Warning'
    case 'fail':
      return 'Fail'
    case 'not_configured':
      return 'Not configured'
    case 'skipped':
      return 'Skipped'
    default:
      return 'Unknown'
  }
}

/** Badge tone for Operation statuses (shared by every Operation surface). */
export function operationTone(
  status: string | null | undefined,
): 'ok' | 'warning' | 'error' | 'neutral' {
  switch (status) {
    case 'succeeded':
      return 'ok'
    case 'succeeded_with_warnings':
    case 'running':
    case 'queued':
      return 'warning'
    case 'failed':
    case 'cancelled':
      return 'error'
    default:
      return 'neutral'
  }
}

/** Badge tone for backup artifact verification state. */
export function verificationTone(
  verification: string | null | undefined,
): 'ok' | 'warning' | 'error' {
  switch (verification) {
    case 'ok':
      return 'ok'
    case 'failed':
      return 'error'
    default:
      return 'warning'
  }
}

/** Display label for backup artifact verification state. */
export function verificationLabel(verification: string | null | undefined): string {
  switch (verification) {
    case 'ok':
      return 'Verified'
    case 'failed':
      return 'Verification failed'
    case 'pending':
      return 'Not verified'
    default:
      return 'Unknown'
  }
}
