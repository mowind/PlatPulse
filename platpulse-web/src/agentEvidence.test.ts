import { describe, expect, it } from 'vitest'
import {
  agentEvidence,
  agentPreviewTier,
  foldedAttentionText,
  foldedRecordedText,
  foldedText,
  summarizeEvidence,
} from './agentEvidence'
import type { AgentDiagnostic, AttentionItem } from './api/generated'

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

  it("lets the Server's agent_spool_fatal item stand for the same fatal flag, matched by kind", () => {
    const evidence = agentEvidence({
      ...BASE,
      attention: [attention('agent_spool_fatal', 'critical', 'the Agent spool store is in a fatal state; durable reports are at risk')],
      host: { ...BASE.host!, spool_store_fatal: true },
    } as AgentDiagnostic)
    expect(texts(evidence.current)).toEqual(['the Agent spool store is in a fatal state; durable reports are at risk'])
  })
})

function attention(kind: AttentionItem['kind'], severity: AttentionItem['severity'], message: string): AttentionItem {
  return {
    id: kind + ':agent:agent-1',
    kind,
    severity,
    message,
    subject_kind: 'agent',
    subject_id: 'agent-1',
    subject_label: 'agent-1',
    observed_at: null,
    evidence_key: kind,
  }
}

describe('summarizeEvidence', () => {
  it('finds a fatal Spool store appended after ordinary current items', () => {
    const summary = summarizeEvidence(agentEvidence({
      ...BASE,
      attention: [attention('agent_offline', 'warning', 'the Agent has not reported within the liveness window')],
      credentials: [{ ...BASE.credentials[0], active: false }],
      host: { ...BASE.host!, spool_store_fatal: true },
    } as AgentDiagnostic))
    expect(summary.lead).toEqual({ group: 'current', item: expect.objectContaining({ key: 'store-fatal', severity: 'critical' }) })
    // The ordinary items keep their order behind it and are counted as current.
    expect(summary.ordered.current.map((item) => item.key)).toEqual(['store-fatal', 'attention-agent_offline:agent:agent-1', 'credentials'])
    expect(foldedAttentionText(summary)).toBe('+2 current')
  })

  it('leads with the most severe Server item and names folded criticals', () => {
    const summary = summarizeEvidence(agentEvidence({
      ...BASE,
      attention: [
        attention('agent_report_gap', 'warning', '2 report sequence gaps were recorded'),
        attention('agent_spool_overflow', 'critical', 'the Agent spool overflowed and discarded queued reports'),
        attention('agent_spool_fatal', 'critical', 'the Agent spool store is in a fatal state; durable reports are at risk'),
      ],
    } as AgentDiagnostic))
    // Equal severities keep the Server order.
    expect(summary.lead?.item.text).toBe('the Agent spool overflowed and discarded queued reports')
    expect(summary.folded).toMatchObject({ current: 2, currentCritical: 1 })
    expect(foldedAttentionText(summary)).toBe('+2 current (1 critical)')
  })

  it('counts Current and Not observed apart and never calls Not observed current', () => {
    const both = summarizeEvidence(agentEvidence({ ...BASE, credentials: [], host: null }))
    expect(both.counts).toEqual({ current: 1, unknown: 1, recorded: 0 })
    expect(both.lead).toEqual({ group: 'current', item: expect.objectContaining({ text: 'No credential issued' }) })
    expect(foldedAttentionText(both)).toBe('+1 not observed')

    const unobservedOnly = summarizeEvidence(agentEvidence({ ...BASE, host: null }))
    expect(unobservedOnly.counts.current).toBe(0)
    expect(unobservedOnly.lead).toEqual({ group: 'unknown', item: expect.objectContaining({ text: 'Host not observed yet' }) })
    expect(foldedText(unobservedOnly)).toBe('')
  })

  it('keeps history-only security events Recorded, never a current Critical', () => {
    const agent = { ...BASE, liveness: 'online', security_event_count: 3 } as AgentDiagnostic
    const summary = summarizeEvidence(agentEvidence(agent))
    expect(summary.counts).toEqual({ current: 0, unknown: 0, recorded: 1 })
    expect(summary.lead).toBeNull()
    expect(summary.recordedLead?.text).toBe('3 security events recorded')
    expect(agentPreviewTier(agent)).toEqual({ rank: 5, bucket: 'with recorded history' })
  })

  it('leads Recorded with an error-level item and counts folded error-level history', () => {
    const summary = summarizeEvidence(agentEvidence({
      ...BASE,
      sequence_gap_count: 2,
      host: { ...BASE.host!, spool_store_error: 'disk full', spool_report_too_large: true, spool_last_delivery_error: 'timeout' },
    } as AgentDiagnostic))
    expect(summary.recordedLead?.short).toBe('Spool store error')
    expect(summary.ordered.recorded.map((item) => item.key)).toEqual(['store-error', 'too-large', 'gaps', 'delivery-error'])
    expect(foldedRecordedText(summary)).toBe('+3 recorded (1 error)')
    // Recorded never feeds the Current count.
    expect(summary.counts.current).toBe(0)
  })
})

describe('agentPreviewTier', () => {
  it('ranks current severity, then reporting, then unobserved and recorded evidence', () => {
    const tier = (extra: Partial<AgentDiagnostic>) => agentPreviewTier({ ...BASE, liveness: 'online', ...extra } as AgentDiagnostic)
    expect(tier({ host: { ...BASE.host!, spool_store_fatal: true } }).rank).toBe(0)
    expect(tier({ attention: [attention('agent_spool_overflow', 'critical', 'overflow')] }).rank).toBe(0)
    expect(tier({ liveness: 'offline' })).toEqual({ rank: 1, bucket: 'offline' })
    expect(tier({ liveness: 'unknown' })).toEqual({ rank: 2, bucket: 'unknown reporting' })
    expect(tier({ credentials: [] })).toEqual({ rank: 3, bucket: 'with current attention' })
    expect(tier({ host: null })).toEqual({ rank: 4, bucket: 'not fully observed' })
    // Retained dropped ranges without a current Server item are history.
    expect(tier({ host: { ...BASE.host!, spool_dropped_sequence_from: 4, spool_dropped_sequence_to: 6 } }).rank).toBe(5)
    expect(tier({})).toEqual({ rank: 6, bucket: null })
  })
})
