import { useState, type FormEvent } from 'react'
import { Link, useParams } from 'react-router'
import {
  AdminApiError,
  createNetworkEntry,
  updateNetworkEntry,
  useAdminNetworkDetail,
  useAdminNetworks,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { StatusBadge, formatObservedAt, freshnessLabel } from '../components/StatusBadge'
import { identityBadge, lifecycleLabel, visibilityBadge } from '../nodeLabels'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import { CardX } from '../components/ui/card-x'
import { Empty } from '../components/ui/empty'
import { Input } from '../components/ui/input'
import { cn } from '../lib/utils'
import { SURFACE_CARD } from '../lib/surface'
import type { AdminNetwork, AdminNetworkNode } from '../api/generated'

/**
 * PAGE-ADMIN-NETWORKS and PAGE-ADMIN-NETWORK-DETAIL (design §4.3, §7.1):
 * the Owner-only Network Registry. The complete validated identity tuple
 * (network_key, display name, genesis hash, chain ID, P2P network ID, and
 * address HRP) is managed through explicit Owner mutations only — observed
 * Agent text never creates or rewrites Registry entries, and mismatch
 * outcomes stay typed, audited, and visible.
 */

const CARD_SURFACE = cn('rounded-md border-none', SURFACE_CARD)

function shortId(id: string): string {
  return id.length > 11 ? id.slice(0, 8) + '…' : id
}

/** Emerald detail grid: label above its value, stacked on narrow viewports. */
function DetailList({ children }: { children: React.ReactNode }) {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>
}

function DetailItem({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wider text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 min-w-0 break-words text-sm">{children}</dd>
    </div>
  )
}

/** PAGE-ADMIN-NETWORKS: Registry list plus the explicit create workflow. */
export default function AdminNetworksList() {
  const { generation } = useAuth()
  const query = useAdminNetworks(generation)
  const [creating, setCreating] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  return (
    <section className="mx-auto w-full max-w-[1280px] space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">Networks</h1>
        <p className="text-sm text-muted-foreground">
          The Network Registry is the only authority for Network identity. Entries are created
          and updated only by explicit Owner actions; Agent observations are compared against
          this tuple and never rewrite it.
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <Button
          type="button"
          onClick={() => setCreating((value) => !value)}
          aria-expanded={creating}
          aria-controls="network-create-form"
        >
          {creating ? 'Close form' : 'Register a Network'}
        </Button>
      </div>
      {notice && (
        <p className="text-sm text-success" role="status">
          {notice}
        </p>
      )}
      {creating && (
        <NetworkCreateForm
          onRegistered={(displayName) => {
            setNotice('Registered ' + displayName + '.')
            setCreating(false)
          }}
        />
      )}
      {!query.data && query.isPending && (
        <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Network Registry…
        </p>
      )}
      {!query.data && query.isError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" />{' '}
          <span className="min-w-0 break-words">
            {query.error instanceof Error ? query.error.message : 'Unable to load Networks'}
          </span>
          <Button variant="link" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      )}
      {query.data && query.isRefetchError && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Registry values.
        </div>
      )}
      {query.data && query.data.length === 0 && (
        <CardX size="medium" className={CARD_SURFACE}>
          <Empty description="No Networks registered yet. Register the first one above." />
        </CardX>
      )}
      {query.data && query.data.length > 0 && (
        <CardX
          size="medium"
          className={CARD_SURFACE}
          contentClassName="p-0"
          segmented
          title="Network Registry"
        >
          <div className="overflow-x-auto">
            <table data-slot="network-table" className="w-full text-sm">
              <caption className="sr-only">Network Registry identity tuples and Node counts</caption>
              <thead>
                <tr className="border-b">
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Network</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Chain ID</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">P2P network</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">HRP</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Genesis</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Nodes</th>
                  <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Mismatches</th>
                </tr>
              </thead>
              <tbody>
                {query.data.map((network) => (
                  <NetworkRow key={network.network_key} network={network} />
                ))}
              </tbody>
            </table>
          </div>
        </CardX>
      )}
    </section>
  )
}

function NetworkRow({ network }: { network: AdminNetwork }) {
  return (
    <tr className="border-b border-border/60 align-top">
      <th scope="row" data-label="Network" className="min-w-0 px-3 py-3 text-left">
        <Link
          className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
          to={'/admin/networks/' + network.network_key}
        >
          {network.display_name}
        </Link>
        <small className="mt-0.5 block text-[11px] text-muted-foreground break-all">{network.network_key}</small>
      </th>
      <td data-label="Chain ID" className="min-w-0 px-3 py-3">
        <span className="font-bold leading-none tracking-tight">{network.chain_id}</span>
      </td>
      <td data-label="P2P network" className="min-w-0 px-3 py-3">
        <span className="font-bold leading-none tracking-tight">{network.p2p_network_id}</span>
      </td>
      <td data-label="HRP" className="min-w-0 px-3 py-3">{network.address_hrp}</td>
      <td data-label="Genesis" className="min-w-0 px-3 py-3">
        <code className="break-all text-[11px]" title={network.genesis_hash}>{shortId(network.genesis_hash)}</code>
      </td>
      <td data-label="Nodes" className="min-w-0 px-3 py-3">
        {network.active_node_count} active · {network.retired_node_count} retired
      </td>
      <td data-label="Mismatches" className="min-w-0 px-3 py-3">
        {network.mismatched_node_count > 0 ? (
          <span className="flex flex-wrap items-center gap-2">
            <StatusBadge status="Mismatched" tone="error" />
            <span className="text-[11px] text-muted-foreground">{network.mismatched_node_count} Node{network.mismatched_node_count === 1 ? '' : 's'}</span>
          </span>
        ) : (
          <span className="text-[11px] text-muted-foreground">No mismatch reported</span>
        )}
      </td>
    </tr>
  )
}

const EMPTY_FORM = {
  networkKey: '',
  displayName: '',
  genesisHash: '',
  chainId: '',
  p2pNetworkId: '',
  addressHrp: '',
}

function NetworkCreateForm({ onRegistered }: { onRegistered: (displayName: string) => void }) {
  const { status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const [form, setForm] = useState(EMPTY_FORM)
  const [error, setError] = useState<string | null>(null)

  const setField = (field: keyof typeof EMPTY_FORM, value: string) =>
    setForm((current) => ({ ...current, [field]: value }))

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)
    try {
      const result = await createNetworkEntry(
        {
          networkKey: form.networkKey.trim(),
          displayName: form.displayName.trim(),
          genesisHash: form.genesisHash.trim(),
          chainId: Number(form.chainId),
          p2pNetworkId: Number(form.p2pNetworkId),
          addressHrp: form.addressHrp.trim(),
        },
        csrfToken,
      )
      setForm(EMPTY_FORM)
      onRegistered(result.displayName)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to register the Network')
    }
  }

  return (
    <CardX
      role="article"
      size="medium"
      className={CARD_SURFACE}
      id="network-create-form"
      header={<h2 className="text-lg font-semibold">Register a Network</h2>}
    >
      <p className="text-sm text-muted-foreground">
        The complete identity tuple is required and audited. The Server never creates a
        Network from observed Agent text.
      </p>
      <form onSubmit={submit} className="mt-3 grid max-w-2xl gap-3">
        <div className="flex flex-col gap-1">
          <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-key">Network key</label>
          <Input
            id="network-key"
            value={form.networkKey}
            onChange={(event) => setField('networkKey', event.target.value)}
            required
          />
        </div>
        <div className="flex flex-col gap-1">
          <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-display-name">Display name</label>
          <Input
            id="network-display-name"
            value={form.displayName}
            onChange={(event) => setField('displayName', event.target.value)}
            required
            maxLength={128}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-genesis">Genesis hash</label>
          <Input
            id="network-genesis"
            value={form.genesisHash}
            onChange={(event) => setField('genesisHash', event.target.value)}
            placeholder="0x…"
            required
          />
        </div>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-chain-id">Chain ID</label>
            <Input
              id="network-chain-id"
              type="number"
              min={0}
              value={form.chainId}
              onChange={(event) => setField('chainId', event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-p2p-id">P2P network ID</label>
            <Input
              id="network-p2p-id"
              type="number"
              min={0}
              value={form.p2pNetworkId}
              onChange={(event) => setField('p2pNetworkId', event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-hrp">Address HRP</label>
            <Input
              id="network-hrp"
              value={form.addressHrp}
              onChange={(event) => setField('addressHrp', event.target.value)}
              required
              maxLength={16}
            />
          </div>
        </div>
        <Button type="submit" className="w-fit">
          Register Network
        </Button>
      </form>
      {error && (
        <p className="mt-3 text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </CardX>
  )
}

/** PAGE-ADMIN-NETWORK-DETAIL: expected identity tuple, per-Node identity
 * dispositions, and the audited edit workflow. */
export function AdminNetworkDetailPage() {
  const { networkKey = '' } = useParams()
  const { generation, status } = useAuth()
  const query = useAdminNetworkDetail(generation, networkKey)
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const notFound =
    query.isError && query.error instanceof AdminApiError && query.error.code === 'not_found'

  if (notFound) {
    return (
      <section className="mx-auto w-full max-w-[1280px] space-y-4">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">Network unavailable</h1>
          <p className="text-sm text-muted-foreground">This Network is no longer registered.</p>
        </div>
      </section>
    )
  }
  return (
    <section className="mx-auto w-full max-w-[1280px] space-y-4">
      <div className="space-y-1">
        <h1 className="text-lg font-semibold break-words">
          {query.data?.display_name ?? networkKey}
          <span className="mt-0.5 block text-xs font-medium text-muted-foreground break-all">{networkKey}</span>
        </h1>
        <p className="text-sm text-muted-foreground">
          <Link
            className="inline-flex min-h-11 min-w-11 items-center font-medium underline-offset-4 hover:underline"
            to="/admin/networks"
          >
            All Networks
          </Link>{' '}
          · expected identity is Registry-owned; observed identity is compared, never trusted.
        </p>
      </div>
      {!query.data && (
        <>
          {query.isPending && (
            <p className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground" role="status">
              <StatusBadge status="Starting" tone="neutral" /> Loading Network state…
            </p>
          )}
          {query.isError && (
            <div
              className="flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
              role="alert"
            >
              <StatusBadge status="Error" tone="error" />{' '}
              <span className="min-w-0 break-words">
                {query.error instanceof Error ? query.error.message : 'Unable to load the Network'}
              </span>
              <Button variant="link" size="sm" onClick={() => void query.refetch()}>
                Try again
              </Button>
            </div>
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
              successful Registry values.
            </div>
          )}
          <IdentityTuplePanel network={query.data} csrfToken={csrfToken} />
          <NetworkNodesPanel networkKey={query.data.network_key} nodes={query.data.nodes} />
        </>
      )}
    </section>
  )
}

function IdentityTuplePanel({
  network,
  csrfToken,
}: {
  network: ReturnType<typeof useAdminNetworkDetail>['data'] & object
  csrfToken: string
}) {
  const [editing, setEditing] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const [displayName, setDisplayName] = useState(network.display_name)
  const [genesisHash, setGenesisHash] = useState(network.genesis_hash)
  const [chainId, setChainId] = useState(String(network.chain_id))
  const [p2pNetworkId, setP2pNetworkId] = useState(String(network.p2p_network_id))
  const [addressHrp, setAddressHrp] = useState(network.address_hrp)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setMessage(null)
    setError(null)
    setConfirming(true)
  }

  async function confirm() {
    setConfirming(false)
    try {
      const result = await updateNetworkEntry(
        network.network_key,
        {
          displayName: displayName.trim(),
          genesisHash: genesisHash.trim(),
          chainId: Number(chainId),
          p2pNetworkId: Number(p2pNetworkId),
          addressHrp: addressHrp.trim(),
        },
        csrfToken,
      )
      setMessage('Updated ' + result.displayName + '.')
      setEditing(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update the Network')
    }
  }

  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      header={
        <>
          <h2 className="text-lg font-semibold">Expected identity tuple</h2>
          <Button
            variant="link"
            size="sm"
            onClick={() => setEditing((value) => !value)}
            aria-expanded={editing}
            aria-controls="network-edit-form"
          >
            {editing ? 'Close editor' : 'Edit tuple'}
          </Button>
        </>
      }
    >
      {editing ? (
        <form id="network-edit-form" onSubmit={submit} className="grid max-w-2xl gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-name">Display name</label>
            <Input
              id="network-edit-name"
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
              required
              maxLength={128}
            />
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-genesis">Genesis hash</label>
            <Input
              id="network-edit-genesis"
              value={genesisHash}
              onChange={(event) => setGenesisHash(event.target.value)}
              required
            />
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-chain">Chain ID</label>
              <Input
                id="network-edit-chain"
                type="number"
                min={0}
                value={chainId}
                onChange={(event) => setChainId(event.target.value)}
                required
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-p2p">P2P network ID</label>
              <Input
                id="network-edit-p2p"
                type="number"
                min={0}
                value={p2pNetworkId}
                onChange={(event) => setP2pNetworkId(event.target.value)}
                required
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-hrp">Address HRP</label>
              <Input
                id="network-edit-hrp"
                value={addressHrp}
                onChange={(event) => setAddressHrp(event.target.value)}
                required
                maxLength={16}
              />
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {confirming ? (
              <>
                <span className="text-[11px] text-muted-foreground">
                  Update the expected identity tuple? Existing Nodes whose observed identity
                  contradicts the new tuple surface as typed mismatches; no Node state changes.
                </span>
                <Button type="button" onClick={() => void confirm()}>
                  Confirm tuple update
                </Button>
                <Button
                  variant="outline"
                  type="button"
                  onClick={() => setConfirming(false)}
                >
                  Keep editing
                </Button>
              </>
            ) : (
              <Button type="submit">Save tuple</Button>
            )}
            <Button
              variant="outline"
              type="button"
              onClick={() => {
                setEditing(false)
                setConfirming(false)
              }}
            >
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <DetailList>
          <DetailItem label="Display name">{network.display_name}</DetailItem>
          <DetailItem label="Network key">
            <code className="break-all text-[11px]">{network.network_key}</code>
          </DetailItem>
          <DetailItem label="Chain ID">
            <span className="font-bold leading-none tracking-tight">{network.chain_id}</span>
          </DetailItem>
          <DetailItem label="P2P network ID">
            <span className="font-bold leading-none tracking-tight">{network.p2p_network_id}</span>
          </DetailItem>
          <DetailItem label="Address HRP">{network.address_hrp}</DetailItem>
          <DetailItem label="Genesis hash">
            <code className="break-all text-[11px]">{network.genesis_hash}</code>
          </DetailItem>
          <DetailItem label="Registered">{formatObservedAt(network.created_at)}</DetailItem>
          <DetailItem label="Updated">{formatObservedAt(network.updated_at)}</DetailItem>
        </DetailList>
      )}
      {message && (
        <p className="mt-3 text-sm text-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="mt-3 text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </CardX>
  )
}

function NetworkNodesPanel({
  networkKey,
  nodes,
}: {
  networkKey: string
  nodes: AdminNetworkNode[]
}) {
  const mismatched = nodes.filter((node) => node.identity.state === 'mismatched').length
  return (
    <CardX
      size="medium"
      className={CARD_SURFACE}
      contentClassName={nodes.length > 0 ? 'p-0' : undefined}
      header={
        <>
          <h2 className="text-lg font-semibold">Nodes on this Network</h2>
          <Badge variant="secondary">{nodes.length}</Badge>
        </>
      }
    >
      {mismatched > 0 && (
        <div
          className="m-3 flex flex-wrap items-center gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm"
          role="alert"
        >
          <StatusBadge status="Mismatched" tone="error" /> {mismatched} Node
          {mismatched === 1 ? ' observes' : 's observe'} an identity that contradicts
          this Registry tuple. Their history is not merged until the observation or
          the tuple is corrected.
        </div>
      )}
      {nodes.length === 0 && (
        <Empty description="No Nodes declared on this Network yet." />
      )}
      {nodes.length > 0 && (
        <div className="overflow-x-auto">
          <table data-slot="network-nodes-table" className="w-full text-sm">
            <caption className="sr-only">
              Nodes on {networkKey} with per-Node identity dispositions
            </caption>
            <thead>
              <tr className="border-b">
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Node</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Health</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Freshness</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Identity</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Visibility</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Lifecycle</th>
                <th scope="col" className="px-3 py-2 text-left text-xs font-medium text-muted-foreground">Head</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((node) => (
                <NetworkNodeRow key={node.node_id} node={node} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </CardX>
  )
}

function NetworkNodeRow({ node }: { node: AdminNetworkNode }) {
  const identity = identityBadge(node.identity)
  const health = node.health === 'healthy' ? 'ok' : node.health === 'unhealthy' ? 'error' : 'neutral'
  const freshness =
    node.freshness === 'current' ? 'ok' : node.freshness === 'stale' ? 'warning' : 'neutral'
  return (
    <tr className="border-b border-border/60 align-top">
      <th scope="row" data-label="Node" className="min-w-0 px-3 py-3 text-left">
        <Link
          className="inline-flex min-h-11 min-w-11 items-center break-all font-medium underline-offset-4 hover:underline"
          to={'/admin/nodes/' + node.node_id}
        >
          {node.display_name ?? node.node_id}
        </Link>
        <small className="mt-0.5 block text-[11px] text-muted-foreground break-all" title={node.node_id}>
          Node ID · {shortId(node.node_id)}
        </small>
      </th>
      <td data-label="Health" className="min-w-0 px-3 py-3">
        <StatusBadge status={node.health} tone={health} />
        <small className="mt-0.5 block text-[11px] text-muted-foreground break-words">{node.health_reason}</small>
      </td>
      <td data-label="Freshness" className="min-w-0 px-3 py-3">
        <StatusBadge status={freshnessLabel(node.freshness)} tone={freshness} />
      </td>
      <td data-label="Identity" className="min-w-0 px-3 py-3">
        <StatusBadge status={identity.label} tone={identity.tone} />
        {node.identity.mismatched_fields.length > 0 && (
          <>
            <small className="mt-0.5 block text-[11px] text-muted-foreground">
              {node.identity.mismatched_fields.join(', ')}
            </small>
            {node.identity.observed && (
              <small className="mt-0.5 block text-[11px] text-muted-foreground break-words">
                Observed:{' '}
                {Object.entries(node.identity.observed)
                  .filter(([, value]) => value != null)
                  .map(([key, value]) => key.replaceAll('_', ' ') + ' ' + value)
                  .join(' · ')}
              </small>
            )}
          </>
        )}
      </td>
      <td data-label="Visibility" className="min-w-0 px-3 py-3">{visibilityBadge(node.visibility).label}</td>
      <td data-label="Lifecycle" className="min-w-0 px-3 py-3">{lifecycleLabel(node.lifecycle).label}</td>
      <td data-label="Head" className="min-w-0 px-3 py-3">
        <span className="font-bold leading-none tracking-tight">{node.current_head ?? 'Unknown'}</span>
      </td>
    </tr>
  )
}
