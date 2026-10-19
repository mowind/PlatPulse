import { ChevronRight } from 'lucide-react'
import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react'
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
import type { AdminNetwork, AdminNetworkDetail, AdminNetworkNode } from '../api/generated'
import { DetailItem, DetailList } from '../components/DetailList'
import { AdminIdentifier } from '../components/AdminIdentifier'
import {
  ADMIN_DETAIL_BODY,
  ADMIN_DETAIL_ROW,
  ADMIN_PAGE_TITLE,
  ADMIN_PANEL_SURFACE,
  ADMIN_ROW,
  ADMIN_SECONDARY,
  ADMIN_SECTION_TITLE,
  ADMIN_TD,
  ADMIN_TH,
  AdminKeyValue,
  AdminKeyValueList,
  AdminPageHeader,
  AdminPanel,
} from '../components/AdminPage'

/**
 * PAGE-ADMIN-NETWORKS and PAGE-ADMIN-NETWORK-DETAIL (design §4.3, §7.1):
 * the Owner-only Network Registry. The complete validated identity tuple
 * (network_key, display name, genesis hash, chain ID, P2P network ID, and
 * address HRP) is managed through explicit Owner mutations only — observed
 * Agent text never creates or rewrites Registry entries, and mismatch
 * outcomes stay typed, audited, and visible.
 */

function shortId(id: string): string {
  return id.length > 11 ? id.slice(0, 8) + '…' : id
}

/** PAGE-ADMIN-NETWORKS: Registry list plus the explicit create workflow. */
export default function AdminNetworksList() {
  const { generation } = useAuth()
  const query = useAdminNetworks(generation)
  const [creating, setCreating] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  return (
    <section className="w-full min-w-0 space-y-4">
      <AdminPageHeader
        title="Networks"
        description="The Network Registry is the only authority for Network identity."
        info={{
          label: 'About the Network Registry',
          content: (
            <p>
              The Network Registry is the only authority for Network identity. Entries are created
              and updated only by explicit Owner actions; Agent observations are compared against
              this tuple and never rewrite it.
            </p>
          ),
        }}
        actions={
          <Button
            type="button"
            variant={creating ? 'outline' : 'default'}
            onClick={() => setCreating((value) => !value)}
            aria-expanded={creating}
            aria-controls="network-create-form"
          >
            {creating ? 'Close form' : 'Register a Network'}
          </Button>
        }
      />
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
        <CardX size="medium" className={ADMIN_PANEL_SURFACE}>
          <Empty description="No Networks registered yet. Register the first one above." />
        </CardX>
      )}
      {query.data && query.data.length > 0 && (
        <AdminPanel
          slot="network-registry"
          flush
          title="Network Registry"
          count={
            <span className="text-xs text-muted-foreground tabular-nums">
              {query.data.length} Network{query.data.length === 1 ? '' : 's'}
            </span>
          }
        >
          <div className="overflow-x-auto max-md:px-3 max-md:pt-3">
            <table data-stack data-slot="network-table" className="w-full text-sm">
              <caption className="sr-only">
                Network Registry: Network, chain ID, Node counts and reported mismatches; the complete
                identity tuple opens per row
              </caption>
              <thead>
                <tr className="border-b border-border/60">
                  <th scope="col" className={ADMIN_TH}>Network</th>
                  <th scope="col" className={ADMIN_TH}>Chain ID</th>
                  <th scope="col" className={ADMIN_TH}>Nodes</th>
                  <th scope="col" className={ADMIN_TH}>Mismatches</th>
                  <th scope="col" className={cn(ADMIN_TH, 'w-px whitespace-nowrap')}>Details</th>
                </tr>
              </thead>
              <tbody>
                {query.data.map((network) => (
                  <NetworkRow key={network.network_key} network={network} />
                ))}
              </tbody>
            </table>
          </div>
        </AdminPanel>
      )}
    </section>
  )
}

function NetworkRow({ network }: { network: AdminNetwork }) {
  const [open, setOpen] = useState(false)
  const toggleRef = useRef<HTMLButtonElement>(null)
  const detailId = 'network-detail-' + network.network_key
  const collapseOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape' || !open) return
    setOpen(false)
    toggleRef.current?.focus()
  }
  return (
    <>
      <tr className={ADMIN_ROW}>
        <th scope="row" data-label="Network" className={cn(ADMIN_TD, 'min-w-0 py-1 text-left font-normal')}>
          <Link
            className="inline-flex min-h-11 min-w-11 items-center break-words font-medium underline-offset-4 hover:underline"
            to={'/admin/networks/' + network.network_key}
          >
            {network.display_name}
          </Link>
          <small className="-mt-2 block font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {network.network_key}
          </small>
        </th>
        <td data-label="Chain ID" className={cn(ADMIN_TD, 'min-w-0 tabular-nums')}>
          {network.chain_id}
        </td>
        <td data-label="Nodes" className={cn(ADMIN_TD, 'min-w-0 tabular-nums')}>
          {network.active_node_count} active · {network.retired_node_count} retired
        </td>
        <td data-label="Mismatches" className={cn(ADMIN_TD, 'min-w-0')}>
          {network.mismatched_node_count > 0 ? (
            <span className="flex flex-wrap items-center gap-2">
              <StatusBadge status="Mismatched" tone="error" />
              <span className="tabular-nums">
                {network.mismatched_node_count} Node{network.mismatched_node_count === 1 ? '' : 's'}
              </span>
            </span>
          ) : (
            <span className={ADMIN_SECONDARY}>No mismatch reported</span>
          )}
        </td>
        <td data-label="Details" className={cn(ADMIN_TD, 'min-w-0 md:w-px md:whitespace-nowrap')}>
          <Button
            ref={toggleRef}
            variant="ghost"
            size="sm"
            className="min-h-11 gap-1 px-2 md:-my-2.5"
            aria-expanded={open}
            aria-controls={detailId}
            onClick={() => setOpen((value) => !value)}
            onKeyDown={collapseOnEscape}
          >
            <ChevronRight size={16} aria-hidden="true" className={cn('transition-transform', open && 'rotate-90')} />
            Details <span className="sr-only">for {network.display_name}</span>
          </Button>
        </td>
      </tr>
      {open && (
        <tr data-slot="detail-row" className={ADMIN_DETAIL_ROW}>
          <td colSpan={5} id={detailId} onKeyDown={collapseOnEscape} className="p-0">
            <div className={ADMIN_DETAIL_BODY}>
              <h3 className={ADMIN_SECTION_TITLE}>Registered identity tuple</h3>
              <AdminKeyValueList>
                <AdminKeyValue label="Network key">
                  <code className="font-mono text-xs">{network.network_key}</code>
                </AdminKeyValue>
                <AdminKeyValue label="Chain ID">
                  <span className="tabular-nums">{network.chain_id}</span>
                </AdminKeyValue>
                <AdminKeyValue label="P2P network ID">
                  <span className="tabular-nums">{network.p2p_network_id}</span>
                </AdminKeyValue>
                <AdminKeyValue label="Address HRP">{network.address_hrp}</AdminKeyValue>
                <AdminKeyValue label="Genesis hash" wide>
                  <AdminIdentifier value={network.genesis_hash} label="Genesis hash" />
                </AdminKeyValue>
              </AdminKeyValueList>
              <p className={ADMIN_SECONDARY}>
                Edit the tuple and review per-Node identity on the{' '}
                <Link
                  className="inline-flex min-h-11 items-center font-medium text-foreground underline-offset-4 hover:underline"
                  to={'/admin/networks/' + network.network_key}
                >
                  Network detail
                </Link>
                .
              </p>
            </div>
          </td>
        </tr>
      )}
    </>
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
      bordered={false}
      segmented
      className={ADMIN_PANEL_SURFACE}
      id="network-create-form"
      header={
        <div className="min-w-0 space-y-0.5">
          <h2 className={ADMIN_SECTION_TITLE}>Register a Network</h2>
          <p className={ADMIN_SECONDARY}>
            The complete identity tuple is required and audited. The Server never creates a
            Network from observed Agent text.
          </p>
        </div>
      }
    >
      <form onSubmit={submit} className="grid max-w-3xl grid-cols-1 gap-x-4 gap-y-3 sm:grid-cols-6">
        <div className="flex min-w-0 flex-col gap-1 sm:col-span-3">
          <label className="text-xs font-medium text-muted-foreground" htmlFor="network-key">Network key</label>
          <Input
            id="network-key"
            value={form.networkKey}
            onChange={(event) => setField('networkKey', event.target.value)}
            required
          />
        </div>
        <div className="flex min-w-0 flex-col gap-1 sm:col-span-3">
          <label className="text-xs font-medium text-muted-foreground" htmlFor="network-display-name">Display name</label>
          <Input
            id="network-display-name"
            value={form.displayName}
            onChange={(event) => setField('displayName', event.target.value)}
            required
            maxLength={128}
          />
        </div>
        <div className="flex min-w-0 flex-col gap-1 sm:col-span-6">
          <label className="text-xs font-medium text-muted-foreground" htmlFor="network-genesis">Genesis hash</label>
          <Input
            id="network-genesis"
            className="font-mono"
            value={form.genesisHash}
            onChange={(event) => setField('genesisHash', event.target.value)}
            placeholder="0x…"
            required
          />
        </div>
        <div className="flex min-w-0 flex-col gap-1 sm:col-span-2">
          <label className="text-xs font-medium text-muted-foreground" htmlFor="network-chain-id">Chain ID</label>
          <Input
            id="network-chain-id"
            type="number"
            min={0}
            className="tabular-nums"
            value={form.chainId}
            onChange={(event) => setField('chainId', event.target.value)}
            required
          />
        </div>
        <div className="flex min-w-0 flex-col gap-1 sm:col-span-2">
          <label className="text-xs font-medium text-muted-foreground" htmlFor="network-p2p-id">P2P network ID</label>
          <Input
            id="network-p2p-id"
            type="number"
            min={0}
            className="tabular-nums"
            value={form.p2pNetworkId}
            onChange={(event) => setField('p2pNetworkId', event.target.value)}
            required
          />
        </div>
        <div className="flex min-w-0 flex-col gap-1 sm:col-span-2">
          <label className="text-xs font-medium text-muted-foreground" htmlFor="network-hrp">Address HRP</label>
          <Input
            id="network-hrp"
            value={form.addressHrp}
            onChange={(event) => setField('addressHrp', event.target.value)}
            required
            maxLength={16}
          />
        </div>
        <div className="flex flex-wrap items-center gap-3 sm:col-span-6">
          <Button type="submit" className="w-fit">
            Register Network
          </Button>
          {error && (
            <p className="min-w-0 text-sm text-destructive [overflow-wrap:anywhere]" role="alert">
              {error}
            </p>
          )}
        </div>
      </form>
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
      <section className="w-full min-w-0 space-y-4">
        <div className="space-y-1">
          <h1 className={ADMIN_PAGE_TITLE}>Network unavailable</h1>
          <p className="text-sm text-muted-foreground">This Network is no longer registered.</p>
        </div>
      </section>
    )
  }
  return (
    <section className="w-full min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className={ADMIN_PAGE_TITLE}>
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
          <IdentityTuplePanel
            // One edit round belongs to one Network.
            key={query.data.network_key}
            network={query.data}
            csrfToken={csrfToken}
            onSaved={() => query.refetch({ cancelRefetch: false })}
          />
          <NetworkNodesPanel networkKey={query.data.network_key} nodes={query.data.nodes} />
        </>
      )}
    </section>
  )
}

/** The editable Registry tuple as the form holds it (numbers as typed). */
type TupleFields = {
  displayName: string
  genesisHash: string
  chainId: string
  p2pNetworkId: string
  addressHrp: string
}

/** The exact update one confirmation sends. */
type TupleUpdate = {
  displayName: string
  genesisHash: string
  chainId: number
  p2pNetworkId: number
  addressHrp: string
}

function tupleFields(network: AdminNetworkDetail): TupleFields {
  return {
    displayName: network.display_name,
    genesisHash: network.genesis_hash,
    chainId: String(network.chain_id),
    p2pNetworkId: String(network.p2p_network_id),
    addressHrp: network.address_hrp,
  }
}

function sameTuple(a: TupleFields, b: TupleFields): boolean {
  return (
    a.displayName === b.displayName &&
    a.genesisHash === b.genesisHash &&
    a.chainId === b.chainId &&
    a.p2pNetworkId === b.p2pNetworkId &&
    a.addressHrp === b.addressHrp
  )
}

function tupleUpdate(fields: TupleFields): TupleUpdate {
  return {
    displayName: fields.displayName.trim(),
    genesisHash: fields.genesisHash.trim(),
    chainId: Number(fields.chainId),
    p2pNetworkId: Number(fields.p2pNetworkId),
    addressHrp: fields.addressHrp.trim(),
  }
}

/** The expected identity tuple and its audited edit. The read view is always
 *  the Server's current tuple. An edit round starts from the latest tuple,
 *  keeps its draft through background refreshes, and ends with Cancel (which
 *  discards it) or a successful update; there is no half-closed editor that
 *  keeps a draft or a confirmation. A confirmation holds the exact update it
 *  sends, and any field change withdraws it. While the update is in flight
 *  only this editor is locked and the request is never sent twice. */
function IdentityTuplePanel({
  network,
  csrfToken,
  onSaved,
}: {
  network: AdminNetworkDetail
  csrfToken: string
  /** Reads the authoritative Network detail again (joining a read in flight). */
  onSaved: () => Promise<{ isError: boolean; dataUpdatedAt: number }>
}) {
  const current = tupleFields(network)
  // One edit round: the draft and the Server tuple it started from.
  const [edit, setEdit] = useState<{ fields: TupleFields; base: TupleFields } | null>(null)
  // The exact update the open confirmation will send.
  const [confirmed, setConfirmed] = useState<TupleUpdate | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const inFlight = useRef(false)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const formRef = useRef<HTMLFormElement>(null)
  const firstFieldRef = useRef<HTMLInputElement>(null)
  const editRef = useRef<HTMLButtonElement>(null)
  const promptRef = useRef<HTMLSpanElement>(null)
  const focusNext = useRef<'field' | 'edit' | 'prompt' | null>(null)
  useEffect(() => {
    const target = focusNext.current
    if (!target) return
    focusNext.current = null
    ;({ field: firstFieldRef, edit: editRef, prompt: promptRef })[target].current?.focus()
  })

  // The Server tuple moved while this round was open: the draft is kept, and
  // a confirmation given against the earlier tuple no longer stands. Our own
  // update is excluded while it is in flight (including its refetch).
  const serverMoved = edit !== null && !submitting && !sameTuple(current, edit.base)
  if (serverMoved && confirmed !== null) setConfirmed(null)
  // The Server already holds the draft (for example a write whose answer was
  // lost reached it after all): nothing is left to save.
  const serverHasDraft =
    serverMoved && JSON.stringify(tupleUpdate(current)) === JSON.stringify(tupleUpdate(edit.fields))

  function startEdit() {
    setEdit({ fields: current, base: current })
    setConfirmed(null)
    setMessage(null)
    setError(null)
    focusNext.current = 'field'
  }

  function cancel() {
    if (inFlight.current) return
    setEdit(null)
    setConfirmed(null)
    setMessage(null)
    setError(null)
    focusNext.current = 'edit'
  }

  function setField(key: keyof TupleFields, value: string) {
    if (edit === null) return
    setEdit({ ...edit, fields: { ...edit.fields, [key]: value } })
    setConfirmed(null)
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (inFlight.current || edit === null) return
    if (!event.currentTarget.reportValidity()) return
    setMessage(null)
    setError(null)
    setConfirmed(tupleUpdate(edit.fields))
    focusNext.current = 'prompt'
  }

  async function confirm() {
    if (inFlight.current || confirmed === null) return
    // Confirm is a plain button: it re-runs the form's own validity rules
    // instead of trusting that the Save step was valid.
    if (formRef.current && !formRef.current.reportValidity()) {
      setConfirmed(null)
      return
    }
    const update = confirmed
    inFlight.current = true
    setSubmitting(true)
    setError(null)
    try {
      let result: Awaited<ReturnType<typeof updateNetworkEntry>>
      try {
        result = await updateNetworkEntry(network.network_key, update, csrfToken)
      } catch (caught) {
        // The draft and the confirmation stay, so the Owner can retry it.
        setError(caught instanceof Error ? caught.message : 'Unable to update the Network')
        return
      }
      // The read view is the Server's tuple; wait for it to be read back. A
      // read that finished before the write did is not that read-back.
      const writtenAt = Date.now()
      let readBack = await onSaved()
      if (!readBack.isError && readBack.dataUpdatedAt < writtenAt) readBack = await onSaved()
      const active = document.activeElement
      if (!active || active === document.body || panelRef.current?.contains(active)) {
        focusNext.current = 'edit'
      }
      setMessage(
        'Updated ' +
          result.displayName +
          '.' +
          (readBack.isError
            ? ' The Network could not be read back, so this page may still show the earlier tuple.'
            : ''),
      )
      setEdit(null)
      setConfirmed(null)
    } finally {
      inFlight.current = false
      setSubmitting(false)
    }
  }

  const fields = edit?.fields
  return (
    <CardX
      ref={panelRef}
      size="medium"
      className={ADMIN_PANEL_SURFACE}
      header={
        <>
          <h2 className={ADMIN_SECTION_TITLE}>Expected identity tuple</h2>
          {!edit && (
            <Button ref={editRef} variant="link" size="sm" onClick={startEdit}>
              Edit tuple
            </Button>
          )}
        </>
      }
    >
      {fields ? (
        <form
          ref={formRef}
          id="network-edit-form"
          onSubmit={submit}
          className="grid max-w-2xl gap-3"
          aria-busy={submitting}
        >
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-name">Display name</label>
            <Input
              ref={firstFieldRef}
              id="network-edit-name"
              value={fields.displayName}
              onChange={(event) => setField('displayName', event.target.value)}
              required
              maxLength={128}
              disabled={submitting}
            />
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-genesis">Genesis hash</label>
            <Input
              id="network-edit-genesis"
              value={fields.genesisHash}
              onChange={(event) => setField('genesisHash', event.target.value)}
              required
              disabled={submitting}
            />
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-chain">Chain ID</label>
              <Input
                id="network-edit-chain"
                type="number"
                min={0}
                value={fields.chainId}
                onChange={(event) => setField('chainId', event.target.value)}
                required
                disabled={submitting}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-p2p">P2P network ID</label>
              <Input
                id="network-edit-p2p"
                type="number"
                min={0}
                value={fields.p2pNetworkId}
                onChange={(event) => setField('p2pNetworkId', event.target.value)}
                required
                disabled={submitting}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium tracking-wider text-muted-foreground" htmlFor="network-edit-hrp">Address HRP</label>
              <Input
                id="network-edit-hrp"
                value={fields.addressHrp}
                onChange={(event) => setField('addressHrp', event.target.value)}
                required
                maxLength={16}
                disabled={submitting}
              />
            </div>
          </div>
          {serverMoved && (
            <div className="grid gap-1 text-xs text-muted-foreground" data-slot="tuple-server-moved">
              <p>
                {serverHasDraft
                  ? 'The Server now holds this tuple — an earlier attempt or another Owner saved it. Nothing is left to save; Cancel closes the editor.'
                  : 'The expected identity tuple changed on the Server while you were editing. Your draft is kept and has not been saved; Cancel to start again from the current tuple.'}
              </p>
              {!serverHasDraft && (
                <p className="[overflow-wrap:anywhere]">
                  Server now: {network.display_name} · Chain ID {network.chain_id} · P2P network ID{' '}
                  {network.p2p_network_id} · Address HRP {network.address_hrp} · Genesis hash{' '}
                  <code className="break-all">{network.genesis_hash}</code>
                </p>
              )}
            </div>
          )}
          <div className="flex flex-wrap items-center gap-2">
            {confirmed ? (
              <>
                <span ref={promptRef} tabIndex={-1} className="text-xs text-muted-foreground outline-none">
                  Update the expected identity tuple? Existing Nodes whose observed identity
                  contradicts the new tuple surface as typed mismatches; no Node state changes,
                  and Block History already rejected on a mismatch is not reclaimed.
                </span>
                <Button
                  type="button"
                  aria-disabled={submitting || undefined}
                  className="aria-disabled:cursor-progress aria-disabled:opacity-70"
                  onClick={() => void confirm()}
                >
                  {submitting ? 'Saving…' : 'Confirm tuple update'}
                </Button>
                <Button
                  variant="outline"
                  type="button"
                  disabled={submitting}
                  onClick={() => {
                    setConfirmed(null)
                    focusNext.current = 'field'
                  }}
                >
                  Keep editing
                </Button>
              </>
            ) : (
              <Button type="submit">Save tuple</Button>
            )}
            <Button variant="outline" type="button" disabled={submitting} onClick={cancel}>
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <DetailList>
          <DetailItem label="Display name">{network.display_name}</DetailItem>
          <DetailItem label="Network key">
            <code className="break-all text-xs">{network.network_key}</code>
          </DetailItem>
          <DetailItem label="Chain ID">
            <span className="font-medium tabular-nums">{network.chain_id}</span>
          </DetailItem>
          <DetailItem label="P2P network ID">
            <span className="font-medium tabular-nums">{network.p2p_network_id}</span>
          </DetailItem>
          <DetailItem label="Address HRP">{network.address_hrp}</DetailItem>
          <DetailItem label="Genesis hash">
            <code className="break-all text-xs">{network.genesis_hash}</code>
          </DetailItem>
          <DetailItem label="Registered">{formatObservedAt(network.created_at)}</DetailItem>
          <DetailItem label="Updated">{formatObservedAt(network.updated_at)}</DetailItem>
        </DetailList>
      )}
      <p className={message && !submitting ? 'mt-3 text-sm text-success' : 'sr-only'} role="status">
        {submitting ? 'Saving the identity tuple…' : message}
      </p>
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
      className={ADMIN_PANEL_SURFACE}
      contentClassName={nodes.length > 0 ? 'p-0' : undefined}
      header={
        <>
          <h2 className={ADMIN_SECTION_TITLE}>Nodes on this Network</h2>
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
          this Registry tuple. Correct the observation on the Node itself: its build
          and its configuration, not this Registry, and then restart that Host's
          Agent — the identity stamp is refreshed only when the Agent re-reads its
          Node. Rewriting the tuple to match one Node is not a remedy — it would
          contradict every other Node on this Network. Block History already rejected
          from those Nodes is never merged and never backfilled, so the affected
          heights stay a permanent gap. See "Node Network Identity at onboarding and
          after a Node build upgrade" in docs/deployment.md.
        </div>
      )}
      {nodes.length === 0 && (
        <Empty description="No Nodes declared on this Network yet." />
      )}
      {nodes.length > 0 && (
        <div className="overflow-x-auto max-md:px-3 max-md:pt-3">
          <table data-stack data-slot="network-nodes-table" className="w-full text-sm">
            <caption className="sr-only">
              Nodes on {networkKey} with per-Node identity dispositions
            </caption>
            <thead>
              <tr className="border-b border-border/60">
                <th scope="col" className={ADMIN_TH}>Node</th>
                <th scope="col" className={ADMIN_TH}>Health</th>
                <th scope="col" className={ADMIN_TH}>Freshness</th>
                <th scope="col" className={ADMIN_TH}>Identity</th>
                <th scope="col" className={ADMIN_TH}>Visibility</th>
                <th scope="col" className={ADMIN_TH}>Lifecycle</th>
                <th scope="col" className={ADMIN_TH}>Head</th>
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
    <tr className={ADMIN_ROW}>
      <th scope="row" data-label="Node" className={cn(ADMIN_TD, 'min-w-0 text-left font-normal')}>
        <Link
          className="inline-flex min-h-11 min-w-11 items-center break-words font-medium underline-offset-4 hover:underline"
          to={'/admin/nodes/' + node.node_id}
        >
          {node.display_name ?? node.node_id}
        </Link>
        <small className="block font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]" title={'Full Node ID: ' + node.node_id}>
          {shortId(node.node_id)}
        </small>
      </th>
      <td data-label="Health" className={cn(ADMIN_TD, 'min-w-0')}>
        <StatusBadge status={node.health} tone={health} />
        <small className="mt-0.5 block text-xs text-muted-foreground break-words">{node.health_reason}</small>
      </td>
      <td data-label="Freshness" className={cn(ADMIN_TD, 'min-w-0')}>
        <StatusBadge status={freshnessLabel(node.freshness)} tone={freshness} />
      </td>
      <td data-label="Identity" className={cn(ADMIN_TD, 'min-w-0')}>
        <StatusBadge status={identity.label} tone={identity.tone} />
        {node.identity.mismatched_fields.length > 0 && (
          <>
            <small className="mt-0.5 block text-xs text-muted-foreground">
              {node.identity.mismatched_fields.join(', ')}
            </small>
            {node.identity.observed && (
              <small className="mt-0.5 block text-xs text-muted-foreground break-words">
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
      <td data-label="Visibility" className={cn(ADMIN_TD, 'min-w-0')}>{visibilityBadge(node.visibility).label}</td>
      <td data-label="Lifecycle" className={cn(ADMIN_TD, 'min-w-0')}>{lifecycleLabel(node.lifecycle).label}</td>
      <td data-label="Head" className={cn(ADMIN_TD, 'min-w-0')}>
        <span className="font-medium tabular-nums">{node.current_head ?? 'Unknown'}</span>
      </td>
    </tr>
  )
}
