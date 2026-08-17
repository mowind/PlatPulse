import { useMemo, useState, type FormEvent } from 'react'
import { Link, useParams } from 'react-router'
import {
  createValidatorLink,
  editValidatorLink,
  endValidator,
  registerValidator,
  useAdminNetworks,
  useAdminNodes,
  useAdminValidatorDetail,
  useAdminValidatorAnalytics,
  useAdminValidatorHistory,
  useAdminValidatorLinks,
  useAdminValidators,
} from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import type {
  NodeValidatorLink,
  Validator,
  ValidatorLinkCreateRequest,
  ValidatorLinkUpdateRequest,
} from '../api/generated'
import { ValidatorInsight } from '../components/ValidatorInsight'
import { ValidatorHistory } from '../components/ValidatorHistory'
import { ValidatorAnalytics } from '../components/ValidatorAnalytics'

const ROLES = ['primary', 'standby', 'observer'] as const

type LinkForm = {
  nodeId: string
  validatorId: string
  role: string
  validFrom: string
  validUntil: string
}

function localDateTimeValue(value: string | Date = new Date()): string {
  const date = typeof value === 'string' ? new Date(value) : value
  const pad = (part: number) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

function initialValidFrom(): string {
  return localDateTimeValue()
}

function formatDate(value: string | null | undefined): string {
  if (!value) return 'Open-ended'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString()
}

function displayNode(link: NodeValidatorLink, nodeNames: Map<string, string>): string {
  return link.nodeDisplayName || nodeNames.get(link.nodeId) || link.nodeId
}

/** Owner-only Validator registry and explicit Node Validator Link lifecycle. */
export default function AdminValidators() {
  const { generation, status } = useAuth()
  const csrfToken = status.state === 'authenticated' ? status.csrfToken : ''
  const networks = useAdminNetworks(generation)
  const nodes = useAdminNodes(generation)
  const validators = useAdminValidators(generation)
  const links = useAdminValidatorLinks(generation)
  const [networkKey, setNetworkKey] = useState('')
  const [validatorNodeId, setValidatorNodeId] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [linkForm, setLinkForm] = useState<LinkForm>({
    nodeId: '',
    validatorId: '',
    role: 'primary',
    validFrom: initialValidFrom(),
    validUntil: '',
  })
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const nodeNames = useMemo(
    () => new Map((nodes.data ?? []).map((node) => [node.node_id, node.display_name || node.node_id])),
    [nodes.data],
  )
  const availableNodes = (nodes.data ?? []).filter((node) => node.lifecycle === 'active')
  const selectedNode = availableNodes.find((node) => node.node_id === linkForm.nodeId)
  const availableValidators = (validators.data ?? []).filter(
    (validator) => !selectedNode || validator.networkKey === selectedNode.network_key,
  )

  async function submitValidator(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setBusy(true)
    setError(null)
    setMessage(null)
    try {
      await registerValidator(networkKey, {
        validatorNodeId: validatorNodeId.trim(),
        displayName: displayName.trim() || null,
      }, csrfToken)
      setValidatorNodeId('')
      setDisplayName('')
      setMessage('Validator registered.')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to register the Validator.')
    } finally {
      setBusy(false)
    }
  }

  async function submitLink(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const request: ValidatorLinkCreateRequest = {
      validatorId: linkForm.validatorId,
      role: linkForm.role,
      validFrom: new Date(linkForm.validFrom).toISOString(),
      validUntil: linkForm.validUntil ? new Date(linkForm.validUntil).toISOString() : null,
    }
    setBusy(true)
    setError(null)
    setMessage(null)
    try {
      await createValidatorLink(linkForm.nodeId, request, csrfToken)
      setMessage('Node Validator Link created.')
      setLinkForm((current) => ({ ...current, validFrom: initialValidFrom(), validUntil: '' }))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to create the Node Validator Link.')
    } finally {
      setBusy(false)
    }
  }

  async function editLink(linkId: string, request: ValidatorLinkUpdateRequest) {
    setBusy(true)
    setError(null)
    setMessage(null)
    try {
      await editValidatorLink(linkId, request, csrfToken)
      setMessage('Node Validator Link replaced; prior history was retained.')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to update the Node Validator Link.')
      throw caught
    } finally {
      setBusy(false)
    }
  }

  async function endLink(link: NodeValidatorLink) {
    if (!window.confirm(`End the ${link.role} link for ${displayNode(link, nodeNames)}?`)) return
    setBusy(true)
    setError(null)
    setMessage(null)
    try {
      await endValidator(link.linkId, { validUntil: new Date().toISOString() }, csrfToken)
      setMessage('Node Validator Link ended.')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'Unable to end the Node Validator Link.')
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="page">
      <h1>Validators</h1>
      <p className="muted">
        Validators are Server-managed Network identities. Links are explicit Owner decisions;
        Agent observations and key rotation never create or rewrite them.
      </p>
      {message && <p className="form-success" role="status">{message}</p>}
      {error && <p className="form-error" role="alert">{error}</p>}

      <div className="admin-grid">
        <section className="panel">
          <h2>Register Validator</h2>
          <form onSubmit={submitValidator} className="stack-form">
            <label>
              Network
              <select value={networkKey} onChange={(event) => setNetworkKey(event.target.value)} required>
                <option value="">Select a Network</option>
                {(networks.data ?? []).map((network) => (
                  <option key={network.network_key} value={network.network_key}>{network.display_name}</option>
                ))}
              </select>
            </label>
            <label>
              Validator Node ID
              <input value={validatorNodeId} onChange={(event) => setValidatorNodeId(event.target.value)} required />
            </label>
            <label>
              Display name <span className="muted">(optional)</span>
              <input value={displayName} onChange={(event) => setDisplayName(event.target.value)} />
            </label>
            <button className="primary-action" type="submit" disabled={busy || !csrfToken}>Register Validator</button>
          </form>
        </section>

        <section className="panel">
          <h2>Create Node Validator Link</h2>
          <form onSubmit={submitLink} className="stack-form">
            <label>
              Node
              <select value={linkForm.nodeId} onChange={(event) => setLinkForm({ ...linkForm, nodeId: event.target.value, validatorId: '' })} required>
                <option value="">Select an active Node</option>
                {availableNodes.map((node) => <option key={node.node_id} value={node.node_id}>{node.display_name || node.node_id}</option>)}
              </select>
            </label>
            <label>
              Validator
              <select value={linkForm.validatorId} onChange={(event) => setLinkForm({ ...linkForm, validatorId: event.target.value })} required>
                <option value="">Select a Validator</option>
                {availableValidators.map((validator) => <option key={validator.validatorId} value={validator.validatorId}>{validator.displayName || validator.validatorNodeId}</option>)}
              </select>
            </label>
            <label>
              Role
              <select value={linkForm.role} onChange={(event) => setLinkForm({ ...linkForm, role: event.target.value })}>
                {ROLES.map((role) => <option key={role} value={role}>{role}</option>)}
              </select>
            </label>
            <label>Valid from<input type="datetime-local" value={linkForm.validFrom} onChange={(event) => setLinkForm({ ...linkForm, validFrom: event.target.value })} required /></label>
            <label>Valid until <span className="muted">(optional)</span><input type="datetime-local" value={linkForm.validUntil} onChange={(event) => setLinkForm({ ...linkForm, validUntil: event.target.value })} /></label>
            <button className="primary-action" type="submit" disabled={busy || !csrfToken}>Create Link</button>
          </form>
        </section>
      </div>

      <section className="panel">
        <h2>Validator Registry</h2>
        {validators.isPending && <p role="status">Loading Validators…</p>}
        {validators.isError && <p className="form-error">Unable to load Validators.</p>}
        {validators.data?.length === 0 && <p className="muted">No Validators have been registered.</p>}
        <div className="admin-grid">
          {(validators.data ?? []).map((validator) => (
            <ValidatorCard
              key={validator.validatorId}
              validator={validator}
              links={links.data ?? []}
              nodeNames={nodeNames}
              onEnd={endLink}
              onEdit={editLink}
              busy={busy}
            />
          ))}
        </div>
      </section>
    </section>
  )
}

function ValidatorCard({
  validator,
  links,
  nodeNames,
  onEnd,
  onEdit,
  busy,
}: {
  validator: Validator
  links: NodeValidatorLink[]
  nodeNames: Map<string, string>
  onEnd: (link: NodeValidatorLink) => void
  onEdit: (linkId: string, request: ValidatorLinkUpdateRequest) => Promise<void>
  busy: boolean
}) {
  const validatorLinks = links.filter((link) => link.validatorId === validator.validatorId)
  const [editingLink, setEditingLink] = useState<NodeValidatorLink | null>(null)
  const [editRole, setEditRole] = useState('primary')
  const [editFrom, setEditFrom] = useState('')
  const [editUntil, setEditUntil] = useState('')

  function beginEdit(link: NodeValidatorLink) {
    setEditingLink(link)
    setEditRole(link.role)
    setEditFrom(localDateTimeValue(link.validFrom))
    setEditUntil(link.validUntil ? localDateTimeValue(link.validUntil) : '')
  }

  async function submitEdit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!editingLink) return
    try {
      await onEdit(editingLink.linkId, {
        role: editRole,
        validFrom: new Date(editFrom).toISOString(),
        validUntil: editUntil ? new Date(editUntil).toISOString() : null,
      })
      setEditingLink(null)
    } catch {
      // The parent displays the redacted mutation error.
    }
  }
  return (
    <article className="panel">
      <h3><Link to={`/admin/validators/${validator.validatorId}`}>{validator.displayName || validator.validatorNodeId}</Link></h3>
      <dl className="detail-list">
        <div><dt>Validator Node ID</dt><dd>{validator.validatorNodeId}</dd></div>
        <div><dt>Network</dt><dd>{validator.networkKey}</dd></div>
        <div><dt>Updated</dt><dd>{formatDate(validator.updatedAt)}</dd></div>
      </dl>
      {validator.insight && <ValidatorInsight insight={validator.insight} compact />}
      <h4>Node Validator Links</h4>
      {validatorLinks.length === 0 ? <p className="muted">No links.</p> : (
        <ul className="compact-list">
          {validatorLinks.map((link) => <li key={link.linkId}>
            <span><strong>{link.role}</strong> · {displayNode(link, nodeNames)}</span>
            <span className="table-secondary">{formatDate(link.validFrom)} → {formatDate(link.validUntil)}</span>
            <span className="page-actions">
              {!link.validUntil && <button type="button" className="small-action" onClick={() => onEnd(link)} disabled={busy}>End link</button>}
              {!link.validUntil && <button type="button" className="small-action" onClick={() => beginEdit(link)} disabled={busy}>Replace</button>}
            </span>
            {editingLink?.linkId === link.linkId && <form onSubmit={submitEdit} className="stack-form">
              <label>New role<select value={editRole} onChange={(event) => setEditRole(event.target.value)}>{ROLES.map((role) => <option key={role} value={role}>{role}</option>)}</select></label>
              <label>Replacement valid from<input type="datetime-local" value={editFrom} onChange={(event) => setEditFrom(event.target.value)} required /></label>
              <label>Replacement valid until <span className="muted">(optional)</span><input type="datetime-local" value={editUntil} onChange={(event) => setEditUntil(event.target.value)} /></label>
              <button className="primary-action" type="submit" disabled={busy}>Save replacement</button>
            </form>}
          </li>)}
        </ul>
      )}
    </article>
  )
}

export function AdminValidatorDetailPage() {
  const { validatorId = '' } = useParams()
  const { generation } = useAuth()
  const query = useAdminValidatorDetail(generation, validatorId)
  const history = useAdminValidatorHistory(generation, validatorId)
  const analytics = useAdminValidatorAnalytics(generation, validatorId)
  if (query.isPending) return <section className="page"><p role="status">Loading Validator…</p></section>
  if (query.isError || !query.data) return <section className="page"><p className="form-error" role="alert">Unable to load this Validator.</p></section>
  return (
    <section className="page">
      <p><Link to="/admin/validators">← Validators</Link></p>
      <h1>{query.data.displayName || query.data.validatorNodeId}</h1>
      <p className="muted">{query.data.networkKey} · {query.data.validatorNodeId}</p>
      {query.data.insight && <ValidatorInsight insight={query.data.insight} />}
      {analytics.isPending && <p role="status">Loading Validator analytics…</p>}
      {analytics.isError && <p className="form-error" role="alert">Unable to load Validator analytics.</p>}
      {analytics.data && <ValidatorAnalytics analytics={analytics.data} />}
      {history.isPending && <p role="status">Loading Validator history…</p>}
      {history.isError && <p className="form-error" role="alert">Unable to load Validator history.</p>}
      {history.data && <ValidatorHistory entries={history.data.entries} />}
        <h2>Link history</h2>
        {query.data.links.length === 0 ? <p className="muted">No Node Validator Links.</p> : <ul className="compact-list">{query.data.links.map((link) => <li key={link.linkId}><strong>{link.role}</strong> · {link.nodeDisplayName || link.nodeId}<span className="table-secondary">{formatDate(link.validFrom)} → {formatDate(link.validUntil)}</span></li>)}</ul>}
    </section>
  )
}
