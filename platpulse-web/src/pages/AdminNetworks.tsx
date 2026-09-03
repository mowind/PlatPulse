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
import type { AdminNetwork, AdminNetworkNode, NodeIdentityStatus } from '../api/generated'

/**
 * PAGE-ADMIN-NETWORKS and PAGE-ADMIN-NETWORK-DETAIL (design §4.3, §7.1):
 * the Owner-only Network Registry. The complete validated identity tuple
 * (network_key, display name, genesis hash, chain ID, P2P network ID, and
 * address HRP) is managed through explicit Owner mutations only — observed
 * Agent text never creates or rewrites Registry entries, and mismatch
 * outcomes stay typed, audited, and visible.
 */

function shortId(id: string): string {
  return id.length > 11 ? `${id.slice(0, 8)}…` : id
}

function identityBadge(identity: NodeIdentityStatus): {
  label: string
  tone: 'ok' | 'warning' | 'error' | 'neutral'
} {
  switch (identity.state) {
    case 'matched':
      return { label: 'Matched', tone: 'ok' }
    case 'mismatched':
      return { label: 'Mismatched', tone: 'error' }
    default:
      return { label: 'Unknown', tone: 'neutral' }
  }
}

/** PAGE-ADMIN-NETWORKS: Registry list plus the explicit create workflow. */
export default function AdminNetworksList() {
  const { generation } = useAuth()
  const query = useAdminNetworks(generation)
  const [creating, setCreating] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  return (
    <section className="page">
      <h1>Networks</h1>
      <p className="muted">
        The Network Registry is the only authority for Network identity. Entries are created
        and updated only by explicit Owner actions; Agent observations are compared against
        this tuple and never rewrite it.
      </p>
      <div className="page-actions">
        <button
          type="button"
          className="primary-action"
          onClick={() => setCreating((value) => !value)}
          aria-expanded={creating}
          aria-controls="network-create-form"
        >
          {creating ? 'Close form' : 'Register a Network'}
        </button>
      </div>
      {notice && (
        <p className="form-success" role="status">
          {notice}
        </p>
      )}
      {creating && (
        <NetworkCreateForm
          onRegistered={(displayName) => {
            setNotice(`Registered ${displayName}.`)
            setCreating(false)
          }}
        />
      )}
      {!query.data && query.isPending && (
        <p className="panel-state" role="status">
          <StatusBadge status="Starting" tone="neutral" /> Loading the Network Registry…
        </p>
      )}
      {!query.data && query.isError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" />{' '}
          {query.error instanceof Error ? query.error.message : 'Unable to load Networks'}
          <button type="button" className="text-action" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
      {query.data && query.isRefetchError && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
          successful Registry values.
        </p>
      )}
      {query.data && query.data.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Networks registered yet. Register the
          first one above.
        </p>
      )}
      {query.data && query.data.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">Network Registry identity tuples and Node counts</caption>
            <thead>
              <tr>
                <th scope="col">Network</th>
                <th scope="col">Chain ID</th>
                <th scope="col">P2P network</th>
                <th scope="col">HRP</th>
                <th scope="col">Genesis</th>
                <th scope="col">Nodes</th>
                <th scope="col">Mismatches</th>
              </tr>
            </thead>
            <tbody>
              {query.data.map((network) => (
                <NetworkRow key={network.network_key} network={network} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}

function NetworkRow({ network }: { network: AdminNetwork }) {
  return (
    <tr>
      <th scope="row" data-label="Network">
        <Link className="agent-link" to={`/admin/networks/${network.network_key}`}>
          {network.display_name}
        </Link>
        <small className="muted">{network.network_key}</small>
      </th>
      <td data-label="Chain ID">{network.chain_id}</td>
      <td data-label="P2P network">{network.p2p_network_id}</td>
      <td data-label="HRP">{network.address_hrp}</td>
      <td data-label="Genesis">
        <code title={network.genesis_hash}>{shortId(network.genesis_hash)}</code>
      </td>
      <td data-label="Nodes">
        {network.active_node_count} active · {network.retired_node_count} retired
      </td>
      <td data-label="Mismatches">
        {network.mismatched_node_count > 0 ? (
          <>
            <StatusBadge status="Mismatched" tone="error" />
            <span className="muted">{network.mismatched_node_count} Node{network.mismatched_node_count === 1 ? '' : 's'}</span>
          </>
        ) : (
          <StatusBadge status="Current" tone="ok" />
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
    <article id="network-create-form" className="panel" aria-labelledby="network-create-heading">
      <div className="panel-heading">
        <h2 id="network-create-heading">Register a Network</h2>
      </div>
      <p className="panel-copy">
        The complete identity tuple is required and audited. The Server never creates a
        Network from observed Agent text.
      </p>
      <form onSubmit={submit} className="stack-form">
        <div className="field">
          <label htmlFor="network-key">Network key</label>
          <input
            id="network-key"
            value={form.networkKey}
            onChange={(event) => setField('networkKey', event.target.value)}
            required
          />
        </div>
        <div className="field">
          <label htmlFor="network-display-name">Display name</label>
          <input
            id="network-display-name"
            value={form.displayName}
            onChange={(event) => setField('displayName', event.target.value)}
            required
            maxLength={128}
          />
        </div>
        <div className="field">
          <label htmlFor="network-genesis">Genesis hash</label>
          <input
            id="network-genesis"
            value={form.genesisHash}
            onChange={(event) => setField('genesisHash', event.target.value)}
            placeholder="0x…"
            required
          />
        </div>
        <div className="field-row">
          <div className="field">
            <label htmlFor="network-chain-id">Chain ID</label>
            <input
              id="network-chain-id"
              type="number"
              min={0}
              value={form.chainId}
              onChange={(event) => setField('chainId', event.target.value)}
              required
            />
          </div>
          <div className="field">
            <label htmlFor="network-p2p-id">P2P network ID</label>
            <input
              id="network-p2p-id"
              type="number"
              min={0}
              value={form.p2pNetworkId}
              onChange={(event) => setField('p2pNetworkId', event.target.value)}
              required
            />
          </div>
          <div className="field">
            <label htmlFor="network-hrp">Address HRP</label>
            <input
              id="network-hrp"
              value={form.addressHrp}
              onChange={(event) => setField('addressHrp', event.target.value)}
              required
              maxLength={16}
            />
          </div>
        </div>
        <button className="primary-action" type="submit">
          Register Network
        </button>
      </form>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </article>
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
      <section className="page">
        <h1>Network unavailable</h1>
        <p>This Network is no longer registered.</p>
      </section>
    )
  }
  return (
    <section className="page">
      <h1>
        {query.data?.display_name ?? networkKey}
        <span className="heading-muted">{networkKey}</span>
      </h1>
      <p className="muted">
        <Link className="text-action" to="/admin/networks">
          All Networks
        </Link>{' '}
        · expected identity is Registry-owned; observed identity is compared, never trusted.
      </p>
      {!query.data && (
        <>
          {query.isPending && (
            <p className="panel-state" role="status">
              <StatusBadge status="Starting" tone="neutral" /> Loading Network state…
            </p>
          )}
          {query.isError && (
            <p className="panel-state" role="alert">
              <StatusBadge status="Error" tone="error" />{' '}
              {query.error instanceof Error ? query.error.message : 'Unable to load the Network'}
              <button type="button" className="text-action" onClick={() => void query.refetch()}>
                Try again
              </button>
            </p>
          )}
        </>
      )}
      {query.data && (
        <>
          {query.isRefetchError && (
            <p className="panel-state" role="alert">
              <StatusBadge status="Error" tone="error" /> Failed to refresh; showing the last
              successful Registry values.
            </p>
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
      setMessage(`Updated ${result.displayName}.`)
      setEditing(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update the Network')
    }
  }

  return (
    <article className="panel">
      <div className="panel-heading">
        <h2>Expected identity tuple</h2>
        <button
          type="button"
          className="text-action"
          onClick={() => setEditing((value) => !value)}
          aria-expanded={editing}
          aria-controls="network-edit-form"
        >
          {editing ? 'Close editor' : 'Edit tuple'}
        </button>
      </div>
      {editing ? (
        <form id="network-edit-form" onSubmit={submit} className="stack-form">
          <div className="field">
            <label htmlFor="network-edit-name">Display name</label>
            <input
              id="network-edit-name"
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
              required
              maxLength={128}
            />
          </div>
          <div className="field">
            <label htmlFor="network-edit-genesis">Genesis hash</label>
            <input
              id="network-edit-genesis"
              value={genesisHash}
              onChange={(event) => setGenesisHash(event.target.value)}
              required
            />
          </div>
          <div className="field-row">
            <div className="field">
              <label htmlFor="network-edit-chain">Chain ID</label>
              <input
                id="network-edit-chain"
                type="number"
                min={0}
                value={chainId}
                onChange={(event) => setChainId(event.target.value)}
                required
              />
            </div>
            <div className="field">
              <label htmlFor="network-edit-p2p">P2P network ID</label>
              <input
                id="network-edit-p2p"
                type="number"
                min={0}
                value={p2pNetworkId}
                onChange={(event) => setP2pNetworkId(event.target.value)}
                required
              />
            </div>
            <div className="field">
              <label htmlFor="network-edit-hrp">Address HRP</label>
              <input
                id="network-edit-hrp"
                value={addressHrp}
                onChange={(event) => setAddressHrp(event.target.value)}
                required
                maxLength={16}
              />
            </div>
          </div>
          <div className="action-row">
            {confirming ? (
              <>
                <span className="muted">
                  Update the expected identity tuple? Existing Nodes whose observed identity
                  contradicts the new tuple surface as typed mismatches; no Node state changes.
                </span>
                <button
                  className="primary-action"
                  type="button"
                  onClick={() => void confirm()}
                >
                  Confirm tuple update
                </button>
                <button
                  className="secondary-action"
                  type="button"
                  onClick={() => setConfirming(false)}
                >
                  Keep editing
                </button>
              </>
            ) : (
              <button className="primary-action" type="submit">
                Save tuple
              </button>
            )}
            <button
              className="secondary-action"
              type="button"
              onClick={() => {
                setEditing(false)
                setConfirming(false)
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <dl className="detail-list">
          <div>
            <dt>Network key</dt>
            <dd>
              <code>{network.network_key}</code>
            </dd>
          </div>
          <div>
            <dt>Display name</dt>
            <dd>{network.display_name}</dd>
          </div>
          <div>
            <dt>Genesis hash</dt>
            <dd>
              <code>{network.genesis_hash}</code>
            </dd>
          </div>
          <div>
            <dt>Chain ID</dt>
            <dd>{network.chain_id}</dd>
          </div>
          <div>
            <dt>P2P network ID</dt>
            <dd>{network.p2p_network_id}</dd>
          </div>
          <div>
            <dt>Address HRP</dt>
            <dd>{network.address_hrp}</dd>
          </div>
          <div>
            <dt>Registered</dt>
            <dd>{formatObservedAt(network.created_at)}</dd>
          </div>
          <div>
            <dt>Updated</dt>
            <dd>{formatObservedAt(network.updated_at)}</dd>
          </div>
        </dl>
      )}
      {message && (
        <p className="form-success" role="status">
          {message}
        </p>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </article>
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
    <article className="panel">
      <div className="panel-heading">
        <h2>Nodes on this Network</h2>
        <span className="panel-count">{nodes.length}</span>
      </div>
      {mismatched > 0 && (
        <p className="panel-state" role="alert">
          <StatusBadge status="Mismatched" tone="error" /> {mismatched} Node
          {mismatched === 1 ? ' observes' : 's observe'} an identity that contradicts
          this Registry tuple. Their history is not merged until the observation or
          the tuple is corrected.
        </p>
      )}
      {nodes.length === 0 && (
        <p className="panel-state">
          <StatusBadge status="Empty" tone="ok" /> No Nodes declared on this Network yet.
        </p>
      )}
      {nodes.length > 0 && (
        <div className="table-wrap">
          <table className="node-table">
            <caption className="sr-only">
              Nodes on {networkKey} with per-Node identity dispositions
            </caption>
            <thead>
              <tr>
                <th scope="col">Node</th>
                <th scope="col">Identity</th>
                <th scope="col">Health</th>
                <th scope="col">Freshness</th>
                <th scope="col">Head</th>
                <th scope="col">Lifecycle</th>
                <th scope="col">Visibility</th>
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
    </article>
  )
}

function NetworkNodeRow({ node }: { node: AdminNetworkNode }) {
  const identity = identityBadge(node.identity)
  const health = node.health === 'healthy' ? 'ok' : node.health === 'unhealthy' ? 'error' : 'neutral'
  const freshness =
    node.freshness === 'current' ? 'ok' : node.freshness === 'stale' ? 'warning' : 'neutral'
  return (
    <tr>
      <th scope="row" data-label="Node">
        <Link className="agent-link" to={`/admin/nodes/${node.node_id}`}>
          {node.display_name ?? node.node_id}
        </Link>
        <small className="muted" title={node.node_id}>
          Node ID · {shortId(node.node_id)}
        </small>
      </th>
      <td data-label="Identity">
        <StatusBadge status={identity.label} tone={identity.tone} />
        {node.identity.mismatched_fields.length > 0 && (
          <>
            <small className="muted">{node.identity.mismatched_fields.join(', ')}</small>
            {node.identity.observed && (
              <small className="muted">
                Observed:{' '}
                {Object.entries(node.identity.observed)
                  .filter(([, value]) => value != null)
                  .map(([key, value]) => `${key.replaceAll('_', ' ')} ${value}`)
                  .join(' · ')}
              </small>
            )}
          </>
        )}
      </td>
      <td data-label="Health">
        <StatusBadge status={node.health} tone={health} />
        <small className="muted">{node.health_reason}</small>
      </td>
      <td data-label="Freshness">
        <StatusBadge status={freshnessLabel(node.freshness)} tone={freshness} />
      </td>
      <td data-label="Head">{node.current_head ?? 'Unknown'}</td>
      <td data-label="Lifecycle">{node.lifecycle === 'retired' ? 'Retired' : 'Active'}</td>
      <td data-label="Visibility">
        {node.visibility === 'public' ? 'Public' : 'Private'}
      </td>
    </tr>
  )
}
