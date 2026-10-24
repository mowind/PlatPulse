import { describe, expect, it } from 'vitest'
import type { SpoolBacklogAgeDiagnostic } from './api/generated'
import { spoolBacklogReading } from './spoolBacklog'

/** The Spool Backlog Age the Admin Agent and Node views read (ADR 0014, issue
 *  #272): the Server derives it at receipt and sends the basis beside it, and a
 *  reading it never attested stays missing instead of becoming a zero. */
describe('Spool Backlog Age', () => {
  const reading = (age_ms: number, basis: string): SpoolBacklogAgeDiagnostic => ({ age_ms, basis })

  it('reads a receipt-anchored age with no qualifier', () => {
    // A Report replayed two hours after it was generated still shows two hours.
    expect(spoolBacklogReading(reading(7_200_000, 'receipt'))).toEqual({
      age: '2h 0m',
      qualifier: null,
    })
    expect(spoolBacklogReading(reading(90_000, 'receipt'))).toEqual({ age: '1m', qualifier: null })
  })

  it('labels the age the Report measured itself as of report generation', () => {
    expect(spoolBacklogReading(reading(900_000, 'generation'))).toEqual({
      age: '15m',
      qualifier: 'as of report generation',
    })
  })

  it('keeps a basis this build does not know visible', () => {
    // A future basis never borrows the receipt-anchored words.
    expect(spoolBacklogReading(reading(60_000, 'report_skew'))).toEqual({
      age: '1m',
      qualifier: 'basis report_skew',
    })
    expect(spoolBacklogReading(reading(60_000, '  '))).toEqual({
      age: '1m',
      qualifier: 'basis not recorded',
    })
  })

  it('reads an authoritative empty backlog as zero, not as a missing reading', () => {
    expect(spoolBacklogReading(reading(0, 'receipt'))).toEqual({ age: '0s', qualifier: null })
  })

  it('has no reading when the Server attested none', () => {
    expect(spoolBacklogReading(null)).toBeNull()
    expect(spoolBacklogReading(undefined)).toBeNull()
  })

  it('never publishes an age no Server can attest', () => {
    expect(spoolBacklogReading(reading(-1, 'receipt'))?.age).toBe('Unknown')
    expect(spoolBacklogReading(reading(Number.NaN, 'receipt'))?.age).toBe('Unknown')
  })
})
