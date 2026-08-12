import { useState } from 'react'
import { updateNodeVisibility } from '../api/admin'
import { useAuth } from '../auth/AuthContext'

export default function AdminHome() {
  const { status } = useAuth()
  const [nodeId, setNodeId] = useState('')
  const [visibility, setVisibility] = useState<'private' | 'public'>('public')
  const [message, setMessage] = useState<string | null>(null)
  async function submit(event: React.FormEvent) {
    event.preventDefault()
    if (status.state !== 'authenticated') return
    try {
      const result = await updateNodeVisibility(nodeId, visibility, status.csrfToken)
      setMessage(`${result.nodeId} is now ${result.visibility}.`)
    } catch (error) { setMessage(error instanceof Error ? error.message : 'Unable to update visibility') }
  }
  return <section className="page"><h1>Admin diagnostics</h1><p>Owner-only visibility workflow. Endpoint and credential details remain hidden from Home.</p><form onSubmit={submit} className="visibility-form"><div className="field"><label htmlFor="node-id">Node ID</label><input id="node-id" value={nodeId} onChange={(event) => setNodeId(event.target.value)} required /></div><div className="field"><label htmlFor="visibility">Visibility</label><select id="visibility" value={visibility} onChange={(event) => setVisibility(event.target.value as 'private' | 'public')}><option value="public">Public</option><option value="private">Private</option></select></div><button className="primary-action" type="submit">Update visibility</button></form>{message && <p role="status">{message}</p>}</section>
}
