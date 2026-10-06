import { useMemo, useState } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  useAdminValidatorDetail,
  useAdminValidatorIdentities,
  useAdminValidators,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { DetailItem, DetailList } from '../components/DetailList'
import { StatusBadge, formatObservedAt, freshnessLabel } from '../components/StatusBadge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Input } from '../components/ui/input'
import { formatAmountExact } from '../lib/amount'
import { formatRatePercentExact } from '../lib/rate'
import { SURFACE_CARD_STATIC } from '../lib/surface'
import { cn } from '../lib/utils'
import {
  sortValidators,
  validatorActivity,
  validatorActivityEvidence,
  validatorCounterStateLabel,
  validatorDisplayName,
  validatorIdentityCoverage,
  validatorIdentityMatchesQuery,
  validatorIdentityNodeLabel,
  validatorIdentityNotice,
  validatorKeyLabel,
  validatorLastGoodAge,
  validatorMatchesQuery,
  validatorPublicAssociation,
  validatorRank,
  validatorRankEvidence,
  validatorRankOutcome,
  validatorSourceLabel,
  validatorStatusEvidence,
  validatorStatusLabel,
  validatorStatusNote,
  validatorStatusTone,
} from '../validators'
import type { AdminNodeValidatorIdentity, Validator, ValidatorDetail } from '../api/generated'
import { ValidatorTrendPanel } from './validatorTrendPanel'

/**
 * PAGE-ADMIN-VALIDATORS and PAGE-ADMIN-VALIDATOR-DETAIL (design §15.4;
 * webui.md §15): the Owner Validator surface. Identity is the automatic
 * Network-scoped correspondence between an Agent-observed full P2P public key
 * and a chain Validator, so it is a chain identity — never an ownership claim,
 * a manual role, or consensus membership. An unresolved or contradicted
 * identification state is Unknown and never absence, a Provider that cannot be
 * read keeps the last-good verdict marked retained, and a verdict without a
 * last-good instant shows no age instead of a zero. The Public projection is
 * not assumed to match this one: an open automatic Link and a projected
 * association are different facts.
 */

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD_STATIC)

function Unavailable({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div
      className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
      role="alert"
    >
      <StatusBadge status="Error" tone="error" />{' '}
      <span className="min-w-0 break-words">{message}</span>
      <Button variant="link" size="sm" onClick={onRetry}>
        Try again
      </Button>
    </div>
  )
}

/** PAGE-ADMIN-VALIDATORS: the Validator registry plus the automatic identity
 *  coverage the Server resolved for every Node it evaluated. */
export default function AdminValidatorsList() {
  const { generation } = useAuth()
  const validators = useAdminValidators(generation)
  const identities = useAdminValidatorIdentities(generation)
  const [query, setQuery] = useState('')

  const rows = useMemo(
    () => sortValidators((validators.data ?? []).filter((entry) => validatorMatchesQuery(entry, query))),
    [validators.data, query],
  )
  const coverage = useMemo(
    () => validatorIdentityCoverage(identities.data ?? []),
    [identities.data],
  )
  const identityRows = useMemo(
    () => (identities.data ?? []).filter((identity) => validatorIdentityMatchesQuery(identity, query)),
    [identities.data, query],
  )

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Validators</h1>
        <p className="text-sm text-muted-foreground">
          A Validator is resolved automatically from the Network plus the full P2P public key an
          Agent observed. The correspondence is a chain identity: this surface never claims
          ownership, consensus membership, or a manual role, and a PlatScan deployment that cannot
          be read leaves the last successful verdict in place instead of reporting an absence.
        </p>
      </div>
      <div className="flex flex-col gap-1 sm:max-w-md">
        <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="validator-search">
          Search Validators and Nodes
        </label>
        <Input
          id="validator-search"
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Display name, Node ID, or identity key"
        />
      </div>
      <AutomaticIdentitySection
        query={identities}
        coverage={coverage}
        rows={identityRows}
      />
      <ValidatorRegistrySection query={validators} rows={rows} />
    </section>
  )
}

/** The automatic identity dimension: what the Server established about the
 *  Node-to-Validator correspondence, and what it could not establish. */
function AutomaticIdentitySection({
  query,
  coverage,
  rows,
}: {
  query: ReturnType<typeof useAdminValidatorIdentities>
  coverage: ReturnType<typeof validatorIdentityCoverage>
  rows: AdminNodeValidatorIdentity[]
}) {
  // The summary is global: it is computed from every Node the Server answered
  // with, so a search can neither hide unresolved Nodes nor certify coverage.
  // A Server that has evaluated nothing is neither resolved nor unresolved.
  const badge =
    coverage.evaluated === 0
      ? { status: 'Nothing evaluated', tone: 'neutral' as const }
      : coverage.unresolved > 0
        ? { status: coverage.unresolved + ' unresolved', tone: 'warning' as const }
        : { status: 'Resolved', tone: 'ok' as const }
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      segmented
      header={
        <>
          <h2 className="text-lg font-semibold">Automatic Validator identity</h2>
          {query.data && (
            <span className="flex flex-wrap items-center gap-2">
              <StatusBadge status={badge.status} tone={badge.tone} />
              <span className="text-[11px] text-muted-foreground">
                {coverage.total} {coverage.total === 1 ? 'Node' : 'Nodes'} ·{' '}
                {coverage.evaluated} evaluated · {coverage.identified} identified
              </span>
            </span>
          )}
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        Coverage is counted per Node, not per Agent: one Agent can observe several Nodes, and an
        unresolved state says the evaluation did not establish an identity rather than that the Node
        does not stake.
      </p>
      {!query.data && query.isPending && (
        <p className="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Validator identity coverage…
        </p>
      )}
      {!query.data && query.isError && (
        <div className="mt-3">
          <Unavailable
            message={
              query.error instanceof Error
                ? query.error.message
                : 'Unable to load Validator identity coverage'
            }
            onRetry={() => void query.refetch()}
          />
        </div>
      )}
      {query.data && query.isRefetchError && (
        <div
          className="mt-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last successful
          identity coverage.
        </div>
      )}
      {query.data && rows.length === 0 && (
        <div className="mt-3">
          <Empty
            description={
              query.data.length === 0
                ? 'No Node has been evaluated for an automatic Validator identity yet.'
                : 'No Node matches this search.'
            }
          />
        </div>
      )}
      {query.data && rows.length > 0 && (
        <div className="mt-3 overflow-x-auto">
          <table data-stack data-slot="validator-identity-table" className="w-full text-sm">
            <caption className="sr-only">
              Automatic Validator identity per Node, with the evaluation state and the observation it
              used
            </caption>
            <thead>
              <tr className="border-b">
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Node</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Identity state</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Observed P2P key</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Validator</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Public association</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Evaluated</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((identity) => (
                <IdentityRow key={identity.nodeId} identity={identity} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </CardX>
  )
}

function IdentityRow({ identity }: { identity: AdminNodeValidatorIdentity }) {
  const notice = validatorIdentityNotice(identity)
  const association = validatorPublicAssociation(identity)
  return (
    <tr className="border-b border-border/60 align-top">
      <th scope="row" data-label="Node" className="min-w-0 px-3 py-3 text-left">
        <Link
          className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
          to={'/admin/nodes/' + identity.nodeId}
        >
          {validatorIdentityNodeLabel(identity)}
        </Link>
        <small className="mt-0.5 block text-[11px] text-muted-foreground break-all">
          {identity.networkKey}
        </small>
      </th>
      <td data-label="Identity state" className="min-w-0 px-3 py-3">
        <StatusBadge status={notice.label} tone={notice.tone} />
        <small className="mt-1 block text-[11px] text-muted-foreground break-words">
          {notice.description}
        </small>
      </td>
      <td data-label="Observed P2P key" className="min-w-0 px-3 py-3">
        <code className="break-all text-[11px]" title={identity.observedValidatorNodeKey ?? undefined}>
          {validatorKeyLabel(identity.observedValidatorNodeKey)}
        </code>
      </td>
      <td data-label="Validator" className="min-w-0 px-3 py-3">
        {identity.validatorId ? (
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to={'/admin/validators/' + identity.validatorId}
          >
            {validatorKeyLabel(identity.validatorNodeKey)}
          </Link>
        ) : (
          <span className="text-[11px] text-muted-foreground">
            Not established
          </span>
        )}
      </td>
      <td data-label="Public association" className="min-w-0 px-3 py-3">
        {association ? (
          <span className="flex flex-wrap items-center gap-2">
            <StatusBadge status={association.label} tone={association.tone} />
            <span className="text-[11px] text-muted-foreground">{association.description}</span>
          </span>
        ) : (
          <span className="text-[11px] text-muted-foreground">No Link to project</span>
        )}
      </td>
      <td data-label="Evaluated" className="min-w-0 px-3 py-3">
        {identity.evaluatedAt ? formatObservedAt(identity.evaluatedAt) : 'Never evaluated'}
        <small className="mt-0.5 block text-[11px] text-muted-foreground">
          Lifecycle {identity.lifecycle}
        </small>
      </td>
    </tr>
  )
}

/** The Validator registry itself: one row per chain identity the Server
 *  resolved from observed evidence. */
function ValidatorRegistrySection({
  query,
  rows,
}: {
  query: ReturnType<typeof useAdminValidators>
  rows: Validator[]
}) {
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      contentClassName="p-0"
      segmented
      title="Validator registry"
    >
      {!query.data && query.isPending && (
        <p className="flex flex-wrap items-center gap-2 p-3 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading Validators…
        </p>
      )}
      {!query.data && query.isError && (
        <div className="p-3">
          <Unavailable
            message={query.error instanceof Error ? query.error.message : 'Unable to load Validators'}
            onRetry={() => void query.refetch()}
          />
        </div>
      )}
      {query.data && query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 border-b border-border/60 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last successful
          Validator values.
        </div>
      )}
      {query.data && rows.length === 0 && (
        <div className="p-3">
          <Empty
            description={
              query.data.length === 0
                ? 'No Validator has been resolved yet. A Validator appears once an Agent observation establishes one.'
                : 'No Validator matches this search.'
            }
          />
        </div>
      )}
      {query.data && rows.length > 0 && (
        <div className="overflow-x-auto">
          <table data-stack data-slot="validator-table" className="w-full text-sm">
            <caption className="sr-only">
              Resolved Validators with independent verdict, rank, and staking-metric evidence
            </caption>
            <thead>
              <tr className="border-b">
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Validator</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Current status</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Last confirmed at</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Rank</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Stake</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Delegators</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Metrics evidence</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Nodes</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((validator) => (
                <ValidatorRow key={validator.validatorId} validator={validator} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </CardX>
  )
}

function ValidatorRow({ validator }: { validator: Validator }) {
  const insight = validator.insight ?? null
  return (
    <tr className="border-b border-border/60 align-top">
      <th scope="row" data-label="Validator" className="min-w-0 px-3 py-3 text-left">
        <Link
          className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
          to={'/admin/validators/' + validator.validatorId}
        >
          {validatorDisplayName(validator)}
        </Link>
        <small className="mt-0.5 block text-[11px] text-muted-foreground break-all">
          {validator.networkKey}
        </small>
      </th>
      <td data-label="Current status" className="min-w-0 px-3 py-3">
        <span className="flex flex-wrap items-center gap-2">
          <StatusBadge status={validatorStatusLabel(insight)} tone={validatorStatusTone(insight)} />
          <span className="text-[11px] text-muted-foreground">
            {validatorStatusEvidence(insight)}
          </span>
        </span>
        <small className="mt-0.5 block text-[11px] text-muted-foreground">
          Activity {validatorActivity(insight)} · {validatorActivityEvidence(insight)}
        </small>
      </td>
      <td data-label="Last confirmed at" className="min-w-0 px-3 py-3">
        {insight?.activityReceivedAt
          ? formatObservedAt(insight.activityReceivedAt)
          : 'No confirmed verdict'}
      </td>
      <td data-label="Rank" className="min-w-0 px-3 py-3">
        {validatorRank(insight)}
        <small className="mt-0.5 block text-[11px] text-muted-foreground">
          {validatorRankEvidence(insight)} · {validatorRankOutcome(insight)}
        </small>
        <small className="mt-0.5 block break-words text-[11px] text-muted-foreground">
          {insight?.rankLastGoodReceivedAt
            ? 'Rank confirmed at ' + formatObservedAt(insight.rankLastGoodReceivedAt)
            : 'Rank never observed'}
        </small>
        {insight?.rankDiagnostic && (
          <small className="mt-0.5 block break-words text-[11px] text-muted-foreground">
            {insight.rankDiagnostic}
          </small>
        )}
      </td>
      <td data-label="Stake" className="min-w-0 px-3 py-3">
        {formatAmountExact(insight?.stakeAmount)}
      </td>
      <td data-label="Delegators" className="min-w-0 px-3 py-3">
        {insight?.delegatorCount ?? 'Unknown'}
      </td>
      <td data-label="Metrics evidence" className="min-w-0 px-3 py-3">
        <span className="block text-[11px] text-muted-foreground">
          {!insight?.lastGoodReceivedAt
            ? 'Metrics not established'
            : insight.freshness === 'fresh'
              ? 'Fresh metrics'
              : insight.freshness === 'stale'
                ? 'Retained metrics (stale)'
                : 'Metrics freshness unknown'}
        </span>
        <small className="mt-0.5 block text-[11px] text-muted-foreground">
          Latest detail refresh {insight?.outcome ?? 'unknown'}
          {insight?.outcome === 'error' && insight.lastGoodReceivedAt && ' · Retained last-good metrics'}
          {insight?.outcome === 'empty' && ' · No metric sample'}
        </small>
        {insight?.diagnostic && (
          <small className="mt-0.5 block break-words text-[11px] text-muted-foreground">
            {insight.diagnostic}
          </small>
        )}
        {insight?.lastGoodReceivedAt && (
          <small className="mt-0.5 block text-[11px] text-muted-foreground">
            Last-good metric age {validatorLastGoodAge(insight.lastGoodAgeSeconds)}
          </small>
        )}
        <small className="mt-0.5 block break-words text-[11px] text-muted-foreground">
          {insight?.lastGoodReceivedAt
            ? 'Last-good metric received at ' + formatObservedAt(insight.lastGoodReceivedAt)
            : 'Metrics never observed'}
        </small>
      </td>
      <td data-label="Nodes" className="min-w-0 px-3 py-3">
        {validator.linkCount}
      </td>
    </tr>
  )
}

/** PAGE-ADMIN-VALIDATOR-DETAIL: one chain identity, its evidence, its staking
 *  metrics, and the Node associations the Server recorded for it. */
export function AdminValidatorDetail() {
  const { validatorId = '' } = useParams()
  const { generation } = useAuth()
  const query = useAdminValidatorDetail(generation, validatorId)
  const notFound =
    query.isError && query.error instanceof AdminApiError && query.error.code === 'not_found'

  if (notFound) {
    return (
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">Validator unavailable</h1>
          <p className="text-sm text-muted-foreground">
            No Validator is resolved under this identity on this Server.
          </p>
          <p className="text-sm">
            <Link
              autoFocus
              className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
              to="/admin/validators"
            >
              Back to Validators
            </Link>
          </p>
        </div>
      </section>
    )
  }

  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">
          {query.data ? validatorDisplayName(query.data) : validatorKeyLabel(validatorId)}
          <span className="mt-0.5 block text-xs font-medium text-muted-foreground break-all">
            {validatorId}
          </span>
        </h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to="/admin/validators"
          >
            All Validators
          </Link>{' '}
          · The identity is the chain key the automatic evaluation resolved from observed P2P
          evidence. Node health and consensus membership stay independent dimensions.
        </p>
      </div>
      {!query.data && (
        <>
          {query.isPending && (
            <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
              <StatusBadge status="Starting" tone="neutral" /> Loading the Validator…
            </p>
          )}
          {query.isError && (
            <Unavailable
              message={
                query.error instanceof Error ? query.error.message : 'Unable to load the Validator'
              }
              onRetry={() => void query.refetch()}
            />
          )}
        </>
      )}
      {query.data && (
        <>
          {query.isRefetchError && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
              successful Validator values.
            </div>
          )}
          <StatusPanel validator={query.data} />
          <RankPanel validator={query.data} />
          <EvidencePanel validator={query.data} />
          <StakingPanel validator={query.data} />
          <LinksPanel validator={query.data} />
          <ValidatorTrendPanel validatorId={validatorId} />
        </>
      )}
    </section>
  )
}

function StatusPanel({ validator }: { validator: ValidatorDetail }) {
  const insight = validator.insight ?? null
  const note = validatorStatusNote(insight)
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Current Validator status</h2>
          <StatusBadge status={validatorStatusLabel(insight)} tone={validatorStatusTone(insight)} />
        </>
      }
    >
      <p className="text-sm text-muted-foreground">
        The verdict states whether these keys are validly staking right now, from the last
        observation the Server could read. A locked or exiting identity is confirmed valid but is
        not normally participating.
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Evidence">{validatorStatusEvidence(insight)}</DetailItem>
          <DetailItem label="Last confirmed at">
            {insight?.activityReceivedAt
              ? formatObservedAt(insight.activityReceivedAt)
              : 'No confirmed verdict'}
          </DetailItem>
          <DetailItem label="Activity">
            {validatorActivity(insight)}{' '}
            <span className="text-muted-foreground">· {validatorActivityEvidence(insight)}</span>
          </DetailItem>
          <DetailItem label="Validator key">
            <code className="break-all text-[11px]" title={validator.validatorNodeId}>
              {validator.validatorNodeId}
            </code>
          </DetailItem>
        </DetailList>
      </div>
      {note && (
        <p className="mt-3 flex flex-wrap items-center gap-2 text-sm">
          <StatusBadge
            status={validatorStatusEvidence(insight)}
            tone={validatorStatusTone(insight)}
          />
          {note}
        </p>
      )}
    </CardX>
  )
}

function RankPanel({ validator }: { validator: ValidatorDetail }) {
  const insight = validator.insight ?? null
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Rank evidence</h2>}
    >
      <p className="text-sm text-muted-foreground">
        The Network ranking list refreshes independently of Validator detail. A failed refresh keeps
        the last-good rank marked stale; without successful rank evidence it stays Unknown, not Unranked.
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Rank">{validatorRank(insight)}</DetailItem>
          <DetailItem label="Rank freshness">
            <StatusBadge
              status={validatorRankEvidence(insight)}
              tone={insight?.rankFreshness === 'fresh' ? 'ok' : insight?.rankFreshness === 'stale' ? 'warning' : 'neutral'}
            />
          </DetailItem>
          <DetailItem label="Rank outcome">{validatorRankOutcome(insight)}</DetailItem>
          <DetailItem label="Rank attempted at">
            {insight?.rankAttemptedAt ? formatObservedAt(insight.rankAttemptedAt) : 'Not attempted'}
          </DetailItem>
          <DetailItem label="Rank last-good received at">
            {insight?.rankLastGoodReceivedAt ? formatObservedAt(insight.rankLastGoodReceivedAt) : 'Never observed'}
          </DetailItem>
          <DetailItem label="Rank last-good age">
            {insight?.rankLastGoodAgeSeconds == null ? 'No last-good observation' : validatorLastGoodAge(insight.rankLastGoodAgeSeconds)}
          </DetailItem>
          <DetailItem label="Rank diagnostic">{insight?.rankDiagnostic ?? 'None reported'}</DetailItem>
        </DetailList>
      </div>
    </CardX>
  )
}

function EvidencePanel({ validator }: { validator: ValidatorDetail }) {
  const insight = validator.insight ?? null
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Provider evidence</h2>}
    >
      <p className="text-sm text-muted-foreground">
        Source, freshness, and diagnostics of the last observation. A Provider failure is reported
        here as unavailable evidence — it is never turned into a negative verdict or a fresh zero.
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Source">{validatorSourceLabel(insight?.source)}</DetailItem>
          <DetailItem label="Freshness">
            {insight?.freshness === 'fresh' ? 'Fresh' : freshnessLabel(insight?.freshness)}
          </DetailItem>
          <DetailItem label="Outcome">{insight?.outcome ?? 'Not observed'}</DetailItem>
          <DetailItem label="Counter state">{validatorCounterStateLabel(insight)}</DetailItem>
          <DetailItem label="Observed at">
            {insight?.receivedAt ? formatObservedAt(insight.receivedAt) : 'Never observed'}
          </DetailItem>
          <DetailItem label="Provider timestamp">
            {insight?.providerTimestamp ? formatObservedAt(insight.providerTimestamp) : 'Not reported'}
          </DetailItem>
          <DetailItem label="Attempted at">
            {insight?.attemptedAt ? formatObservedAt(insight.attemptedAt) : 'Not attempted'}
          </DetailItem>
          <DetailItem label="Diagnostic">{insight?.diagnostic ?? 'None reported'}</DetailItem>
        </DetailList>
      </div>
    </CardX>
  )
}

function StakingPanel({ validator }: { validator: ValidatorDetail }) {
  const insight = validator.insight ?? null
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Staking metrics</h2>}
    >
      <p className="text-sm text-muted-foreground">
        Values come from the last-good Provider observation for this identity. A metric that was
        never reported is Unknown, never zero.
      </p>
      <div className="mt-3">
        <DetailList>
          <DetailItem label="Stake">{formatAmountExact(insight?.stakeAmount)}</DetailItem>
          <DetailItem label="Delegators">{insight?.delegatorCount ?? 'Unknown'}</DetailItem>
          <DetailItem label="Blocks produced">{insight?.blockCount ?? 'Unknown'}</DetailItem>
          <DetailItem label="Reward">{formatAmountExact(insight?.rewardAmount)}</DetailItem>
          <DetailItem label="Reward rate">{formatRatePercentExact(insight?.rewardRate)}</DetailItem>
          <DetailItem label="Epoch">{insight?.epoch ?? 'Unknown'}</DetailItem>
          <DetailItem label="Display name">{validator.displayName ?? 'Not set'}</DetailItem>
        </DetailList>
      </div>
    </CardX>
  )
}

function LinksPanel({ validator }: { validator: ValidatorDetail }) {
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={<h2 className="text-lg font-semibold">Node associations</h2>}
    >
      <p className="text-sm text-muted-foreground">
        Every Node the Server associates with this identity. A key change ends an interval and keeps
        the earlier ones readable; a Node Purge deletes the intervals owned by that Node together with
        the Node, so the association reads back as unavailable instead of being reconstructed, while
        this Validator identity and its recorded history remain.
      </p>
      {validator.links.length === 0 ? (
        <div className="mt-3">
          <Empty description="No Node association is recorded for this Validator." />
        </div>
      ) : (
        <div className="mt-3 overflow-x-auto">
          <table data-stack data-slot="validator-links-table" className="w-full text-sm">
            <caption className="sr-only">Node associations recorded for this Validator</caption>
            <thead>
              <tr className="border-b">
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Node</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Valid from</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Valid until</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Role</th>
              </tr>
            </thead>
            <tbody>
              {validator.links.map((link) => (
                <tr key={link.linkId} className="border-b border-border/60 align-top">
                  <th scope="row" data-label="Node" className="min-w-0 px-3 py-3 text-left">
                    <Link
                      className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
                      to={'/admin/nodes/' + link.nodeId}
                    >
                      {link.nodeDisplayName ?? link.nodeId}
                    </Link>
                  </th>
                  <td data-label="Valid from" className="min-w-0 px-3 py-3">
                    {formatObservedAt(link.validFrom)}
                  </td>
                  <td data-label="Valid until" className="min-w-0 px-3 py-3">
                    {link.validUntil ? (
                      <span className="flex flex-wrap items-center gap-2">
                        <StatusBadge status="Ended" tone="neutral" />
                        <span className="text-[11px] text-muted-foreground">
                          {formatObservedAt(link.validUntil)}
                        </span>
                      </span>
                    ) : (
                      <StatusBadge status="Open" tone="ok" />
                    )}
                  </td>
                  <td data-label="Role" className="min-w-0 px-3 py-3">
                    {link.role ? (
                      link.role
                    ) : (
                      <span className="text-[11px] text-muted-foreground">
                        Automatic identity, no manual role
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </CardX>
  )
}
