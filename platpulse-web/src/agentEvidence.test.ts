import { describe, expect, it } from 'vitest'
import { agentEvidence } from './agentEvidence'
import type { AgentDiagnostic } from './api/generated'

const BASE = {
  agent_id: 'agent-1',
  attention: [],
  credentials: [{ credential_id: 'c1', active: true, created_at: '2026-08-12T00:00:00Z', revoked_at: null, revoke_after: null }],
  host: { components: [], updated_at: '2026-08-12T08:00:00Z', spool_queued_reports: 0, spool_in_flight: false },
  inventory: { accepted_revision: 1, accepted_sha256: null, accepted_declaration: 'agent_declared', last_rejection: null },
  sequence_gap_count: 0,
  security_event_count: 0,
  shutdown_forced: false,
} as unknown as AgentDiagnostic

const texts = (items: { text: string }[]) => items.map((item) => item.text)

describe('agentEvidence', () => {
  it('lists nothing for an observed, empty spool and zero counters', () => {
    expect(agentEvidence(BASE)).toEqual({ current: [], unknown: [], recorded: [] })
  })

  it('keeps never-observed evidence Unknown instead of zero', () => {
    expect(texts(agentEvidence({ ...BASE, host: null }).unknown)).toEqual(['Host not observed yet'])
    expect(texts(agentEvidence({ ...BASE, host: { components: [], updated_at: '2026-08-12T08:00:00Z' } }).unknown)).toEqual([
      'Spool not observed yet',
    ])
  })

  it('separates Server attention and credential state from recorded history', () => {
    const evidence = agentEvidence({
      ...BASE,
      attention: [{ id: 'a', kind: 'agent_offline', severity: 'warning', message: 'not reporting', subject_kind: 'agent', subject_id: 'agent-1', subject_label: 'agent-1', observed_at: null, evidence_key: 'k' }],
      credentials: [{ ...BASE.credentials[0], active: false, revoked_at: '2026-08-12T01:00:00Z' }],
      sequence_gap_count: 2,
      security_event_count: 1,
      host: { ...BASE.host!, spool_dropped_sequence_from: 4, spool_dropped_sequence_to: 6, spool_last_delivery_error: 'timeout' },
    } as AgentDiagnostic)
    expect(texts(evidence.current)).toEqual(['not reporting', 'No active credential'])
    expect(texts(evidence.recorded)).toEqual([
      '2 gap intervals recorded',
      '1 security event recorded',
      'Discarded reports recorded',
      'Delivery error recorded',
    ])
    expect(evidence.recorded.find((item) => item.key === 'dropped')?.tone).toBe('error')
  })

  it('says a credential was never issued rather than calling it invalid', () => {
    expect(texts(agentEvidence({ ...BASE, credentials: [] }).current)).toEqual(['No credential issued'])
  })

  it('treats a fatal store in the latest Host observation as current and retained errors as recorded', () => {
    const evidence = agentEvidence({
      ...BASE,
      host: {
        ...BASE.host!,
        spool_store_fatal: true,
        spool_store_error: 'disk full',
        components: [{ component: 'disk', state: 'error', error_code: 'io', error_message: null, attempted_at: null, observed_at: null, received_at: null, state_revision: 1, value_revision: 1 }],
      },
    } as AgentDiagnostic)
    expect(texts(evidence.current)).toEqual(['Spool store fatal in the latest Host observation'])
    expect(texts(evidence.recorded)).toEqual(['Spool store error recorded', '1 Host component issue'])
    expect(evidence.recorded[0].short).toBe('Spool store error')
  })

  it('adds no item for a list the projection omitted', () => {
    const partial = { ...BASE, attention: undefined, credentials: undefined, inventory: undefined } as unknown as AgentDiagnostic
    expect(agentEvidence(partial)).toEqual({ current: [], unknown: [], recorded: [] })
  })
})
