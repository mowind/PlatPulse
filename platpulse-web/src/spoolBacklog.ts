import type { SpoolBacklogAgeDiagnostic } from './api/generated'
import { formatDuration } from './formatDuration'

/**
 * What one Spool Backlog Age says (ADR 0014, issue #272).
 *
 * The Server derives the age when it receives the Report that carries the
 * current Spool observation, so the number means one thing anchored on the
 * Server receipt instant and another anchored on the Report's own generation
 * instant. It sends the basis beside the number, and this is the only place the
 * two are turned into words: a reading anchored at receipt needs no qualifier,
 * while the Report's own reading is labelled so a reader never compares it with
 * a receipt-anchored one.
 *
 * The Server owns the threshold that makes a backlog worth warning about, so
 * this module adds no tone, severity or colour of its own.
 */
export type SpoolBacklogReading = {
  /** The age as a duration, e.g. `2h 15m`; `0s` is an authoritative empty
   *  backlog rather than a missing reading. */
  age: string
  /** What the age is anchored on, shown after it; `null` for the receipt-anchored
   *  reading that needs none. An age the Server did not attest is never shown. */
  qualifier: string | null
}

/** The receipt-anchored reading needs no qualifier, and it is named here so the
 *  basis strings the Server sends are read in one place. */
const RECEIPT_BASIS = 'receipt'
const GENERATION_BASIS = 'generation'

/**
 * The reading a Server-attested Spool Backlog Age carries, or `null` when the
 * Server has none — a reading it never attested is never a zero backlog.
 *
 * A basis this build does not know is kept visible (`basis <value>`) instead of
 * being presented as the receipt-anchored reading: a future basis the Server
 * adds must not silently borrow today's words.
 */
export function spoolBacklogReading(
  reading: SpoolBacklogAgeDiagnostic | null | undefined,
): SpoolBacklogReading | null {
  if (!reading) return null
  return {
    age: formatDuration(reading.age_ms),
    qualifier: spoolBacklogBasis(reading.basis),
  }
}

function spoolBacklogBasis(basis: string | null | undefined): string | null {
  const named = basis?.trim() ?? ''
  if (named === RECEIPT_BASIS) return null
  if (named === GENERATION_BASIS) return 'as of report generation'
  return named === '' ? 'basis not recorded' : 'basis ' + named
}
