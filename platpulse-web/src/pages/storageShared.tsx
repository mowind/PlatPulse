import { useAdminStorage } from '../api/admin'
import { StatusBadge } from '../components/StatusBadge'
import { DetailItem, DetailList, errorMessage } from './operationsShared'

/**
 * The Storage Backend the Server is using, for the Owner-only surfaces that
 * need it (issue #256, ADR 0013). Doctor reads it beside its checks and
 * Settings reports it as configuration, so both render the same facts from the
 * one read-only Admin endpoint instead of repeating the plumbing.
 *
 * The Admin never offers to switch backends: the backend is chosen in the
 * Server configuration file, and a surface that could change it at runtime
 * would decide where the Server's data lives behind the Operator's back.
 * Nothing here reaches a Public Projection, a Public DTO, or SSE.
 */
export function StorageBackendFacts({ generation }: { generation: number }) {
  const storage = useAdminStorage(generation)
  const overview = storage.data

  if (!overview && storage.isPending) {
    return (
      <p role="status" className="flex flex-wrap items-center gap-2 text-sm">
        <StatusBadge status="Starting" tone="neutral" /> Loading the Storage Backend…
      </p>
    )
  }
  if (!overview && storage.isError) {
    return (
      <p role="alert" className="text-sm">
        {errorMessage(storage.error, 'Unable to load the Storage Backend')}
      </p>
    )
  }
  if (!overview) return null

  const postgres = overview.postgres
  // An unreadable version stays Unknown: the surface never invents a plausible
  // server version, and "below the supported minimum" is only claimed when the
  // Server actually reported a number (ADR 0013).
  const tooOld =
    postgres?.serverVersionNum != null &&
    postgres.serverVersionNum < postgres.minimumServerVersionNum

  return (
    <div data-slot="storage-backend-facts" className="space-y-3">
      <DetailList>
        <DetailItem label="Storage Backend">
          {overview.backend === 'postgres' ? 'PostgreSQL' : 'SQLite'}
        </DetailItem>
        {postgres ? (
          <>
            <DetailItem label="Server version">
              {postgres.serverVersion ?? 'Unknown'}
            </DetailItem>
            <DetailItem label="Host">
              {postgres.host + ':' + String(postgres.port)}
            </DetailItem>
            <DetailItem label="Database">{postgres.database}</DetailItem>
            <DetailItem label="User">{postgres.user}</DetailItem>
          </>
        ) : (
          <DetailItem label="Database file">
            {overview.databasePath ?? 'Unknown'}
          </DetailItem>
        )}
      </DetailList>
      {storage.isRefetchError && (
        // The facts above are the last read the Server succeeded with; a
        // failed refresh must say so instead of passing them off as current.
        <p role="alert" className="text-sm">
          Failed to refresh; showing the Storage Backend as it was last read from the Server.
        </p>
      )}
      {tooOld && (
        <p role="alert" className="text-sm">
          This PostgreSQL server is older than the minimum this Server supports. Upgrade the database before
          relying on it.
        </p>
      )}
      <p className="text-xs text-muted-foreground">
        {postgres
          ? 'The database disk, its backups, and its integrity belong to the database operator, so this Server neither measures nor protects them.'
          : 'The Server keeps its own database file here, so offline backups, restores, integrity checks, and capacity protection all apply.'}{' '}
        The backend is chosen in the Server configuration file; no Admin surface switches it.
      </p>
    </div>
  )
}
