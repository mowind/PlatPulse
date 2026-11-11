import { describe, expect, it } from 'vitest'
import { TransportError } from './transport'
import {
  CHANNEL_TEST_REFUSALS,
  SILENCE_CREATE_REFUSALS,
  mutationOutcome,
} from './mutationOutcome'

function domainError(
  code: string,
  status: number,
  wellFormedEnvelope = true,
): TransportError {
  return new TransportError('domain', code, 'Server message', {
    status,
    wellFormedEnvelope,
  })
}

describe('mutationOutcome', () => {
  it('treats a documented status and code in a complete envelope as a refusal', () => {
    expect(mutationOutcome(domainError('invalid_json', 400), SILENCE_CREATE_REFUSALS)).toBe(
      'refused',
    )
    expect(
      mutationOutcome(domainError('csrf_validation_failed', 403), SILENCE_CREATE_REFUSALS),
    ).toBe('refused')
  })

  it('does not accept a code that belongs to another endpoint', () => {
    expect(
      mutationOutcome(domainError('invalid_query', 400), SILENCE_CREATE_REFUSALS),
    ).toBe('unknown')
    expect(
      mutationOutcome(domainError('invalid_json', 400), CHANNEL_TEST_REFUSALS),
    ).toBe('unknown')
  })

  it('treats an unknown code or a wrong status as not a refusal', () => {
    expect(mutationOutcome(domainError('not_a_server_code', 400), SILENCE_CREATE_REFUSALS)).toBe(
      'unknown',
    )
    expect(mutationOutcome(domainError('invalid_json', 500), SILENCE_CREATE_REFUSALS)).toBe(
      'unknown',
    )
  })

  it('treats a gateway 502/504 JSON body as not a refusal', () => {
    expect(mutationOutcome(domainError('bad_gateway', 502), SILENCE_CREATE_REFUSALS)).toBe(
      'unknown',
    )
    expect(mutationOutcome(domainError('gateway_timeout', 504), SILENCE_CREATE_REFUSALS)).toBe(
      'unknown',
    )
    expect(mutationOutcome(domainError('invalid_json', 502), SILENCE_CREATE_REFUSALS)).toBe(
      'unknown',
    )
  })

  it('treats an incomplete envelope or an HTML body as not a refusal', () => {
    expect(
      mutationOutcome(domainError('invalid_json', 400, false), SILENCE_CREATE_REFUSALS),
    ).toBe('unknown')
    expect(
      mutationOutcome(
        new TransportError('domain', 'request_failed', 'Internal Server Error', {
          status: 500,
          wellFormedEnvelope: false,
        }),
        SILENCE_CREATE_REFUSALS,
      ),
    ).toBe('unknown')
  })

  it('treats a dropped transport as not a refusal', () => {
    expect(
      mutationOutcome(
        new TransportError('transport', 'network_unavailable', 'Failed to fetch'),
        SILENCE_CREATE_REFUSALS,
      ),
    ).toBe('unknown')
  })

  it('treats 503 unavailable as not a refusal but 503 storage_conflict as a refusal', () => {
    expect(mutationOutcome(domainError('unavailable', 503), SILENCE_CREATE_REFUSALS)).toBe(
      'unknown',
    )
    expect(
      mutationOutcome(domainError('storage_conflict', 503), SILENCE_CREATE_REFUSALS),
    ).toBe('refused')
  })

  it('treats a non-transport error as not a refusal', () => {
    expect(mutationOutcome(new Error('Failed to fetch'), SILENCE_CREATE_REFUSALS)).toBe('unknown')
    expect(mutationOutcome('not an error', SILENCE_CREATE_REFUSALS)).toBe('unknown')
  })
})
