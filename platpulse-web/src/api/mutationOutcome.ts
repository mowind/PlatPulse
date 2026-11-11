import { TransportError } from './transport'

/** What a failed Admin mutation proves about whether the Server applied it. */
export type MutationOutcome = 'refused' | 'unknown'

/**
 * The status/code pairs one endpoint documents as a refusal. A status maps to
 * the exact codes that endpoint may answer with for that status.
 */
export type RefusalTable = Readonly<Record<number, readonly string[]>>

/**
 * The guards every Admin route shares run before its handler: authentication,
 * the Owner role, proxy trust, and CSRF validation. Their refusals are
 * documented for every Admin mutation, so each endpoint table includes them.
 */
const ADMIN_GUARD_REFUSALS: RefusalTable = {
  401: ['auth_required'],
  403: [
    'csrf_validation_failed',
    'owner_required',
    'untrusted_proxy_headers',
    'proxy_scheme_required',
  ],
}

function adminRefusals(table: RefusalTable): RefusalTable {
  return { ...ADMIN_GUARD_REFUSALS, ...table }
}

/**
 * A mutation refusal must be proved before it is treated as a failure: only a
 * complete Server error envelope whose status and code are both documented by
 * that endpoint says the request was not applied. Everything else is unknown:
 * a transport drop, an intermediary's 502/504 JSON, an unknown code, a wrong
 * status, an incomplete envelope, or an HTML body. A 503 `unavailable` is not
 * a refusal in these tables because several handlers read after committing and
 * can answer it after the write is durable; only `storage_conflict` (raised by
 * a constraint violation inside the writing statement) is proof.
 */
export function mutationOutcome(error: unknown, refusals: RefusalTable): MutationOutcome {
  if (!(error instanceof TransportError)) return 'unknown'
  if (!error.wellFormedEnvelope || error.status === null) return 'unknown'
  return refusals[error.status]?.includes(error.code) ? 'refused' : 'unknown'
}

/** Create a Silence: validation and the write happen in one transaction. */
export const SILENCE_CREATE_REFUSALS = adminRefusals({
  400: ['invalid_json', 'alert_validation'],
  503: ['storage_conflict'],
})

/** Cancel a Silence: not found and already-terminal are documented refusals. */
export const SILENCE_CANCEL_REFUSALS = adminRefusals({
  404: ['silence_not_found'],
  409: ['silence_already_cancelled', 'silence_expired'],
  503: ['storage_conflict'],
})

/** Create a Maintenance Window. */
export const MAINTENANCE_CREATE_REFUSALS = adminRefusals({
  400: ['invalid_json', 'alert_validation'],
  503: ['storage_conflict'],
})

/** Cancel a Maintenance Window. */
export const MAINTENANCE_CANCEL_REFUSALS = adminRefusals({
  404: ['maintenance_not_found'],
  409: ['maintenance_already_cancelled', 'maintenance_expired'],
  503: ['storage_conflict'],
})

/** Retry one Delivery under an opaque request id. */
export const DELIVERY_RETRY_REFUSALS = adminRefusals({
  400: ['notification_request_id_invalid'],
  404: ['notification_delivery_not_found'],
  409: ['delivery_already_queued', 'delivery_not_retryable', 'notification_request_id_conflict'],
  503: ['storage_conflict'],
})

/** Send one controlled test notification through a channel. */
export const CHANNEL_TEST_REFUSALS = adminRefusals({
  400: ['notification_request_id_invalid'],
  404: ['channel_not_configured'],
  409: ['channel_disabled', 'notification_request_id_conflict'],
  429: ['test_cooldown_active'],
  503: ['storage_conflict'],
})

/** Queue a Doctor run. Its 503 can be raised after the operation is recorded. */
export const DOCTOR_RUN_REFUSALS = adminRefusals({
  503: ['storage_conflict'],
})

/** Queue a backup verification. Its 503 can be raised after the operation is recorded. */
export const BACKUP_VERIFY_REFUSALS = adminRefusals({
  404: ['backup_artifact_not_found'],
  503: ['storage_conflict'],
})

/** Cancel a queued or running operation. */
export const OPERATION_CANCEL_REFUSALS = adminRefusals({
  404: ['operation_not_found'],
  409: ['operation_not_cancellable'],
  503: ['storage_conflict'],
})

/** Update one retention policy. Its 503 can be raised after the write commits. */
export const RETENTION_UPDATE_REFUSALS = adminRefusals({
  400: ['retention_out_of_bounds'],
  404: ['retention_family_not_found'],
  409: ['retention_policy_version_conflict'],
  503: ['storage_conflict'],
})

/** Compose a retention preview. */
export const RETENTION_PREVIEW_REFUSALS = adminRefusals({
  400: ['invalid_query'],
  503: ['storage_conflict'],
})

/** Start a retention run for a preview. Its 503 can be raised after the run is recorded. */
export const RETENTION_RUN_REFUSALS = adminRefusals({
  400: ['operation_request_id_invalid'],
  404: ['retention_preview_not_found'],
  409: [
    'retention_preview_stale',
    'retention_run_in_progress',
    'operation_request_id_conflict',
  ],
  503: ['storage_conflict'],
})
