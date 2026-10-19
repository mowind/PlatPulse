import { describe, expect, it } from 'vitest'
import { readBackRecovered } from './readBack'

describe('readBackRecovered', () => {
  // The read-back failed at t=2000 as the query's second error.
  const failure = { errorUpdateCount: 2, errorUpdatedAt: 2000 }

  it('is not recovered by the detail held from before the write', () => {
    // A render whose props have not caught up with the failure yet.
    expect(readBackRecovered(failure, { isError: false, errorUpdateCount: 1, dataUpdatedAt: 1000 })).toBe(false)
    // Same millisecond as the failure is still not after it.
    expect(readBackRecovered(failure, { isError: false, errorUpdateCount: 1, dataUpdatedAt: 2000 })).toBe(false)
  })

  it('is not recovered while the detail query is still in error', () => {
    expect(readBackRecovered(failure, { isError: true, errorUpdateCount: 2, dataUpdatedAt: 1000 })).toBe(false)
    expect(readBackRecovered(failure, { isError: true, errorUpdateCount: 3, dataUpdatedAt: 1000 })).toBe(false)
  })

  it('is recovered by a successful read after the failure', () => {
    expect(readBackRecovered(failure, { isError: false, errorUpdateCount: 2, dataUpdatedAt: 2000 })).toBe(true)
    // A fresh query (for example a new access generation) read after it.
    expect(readBackRecovered(failure, { isError: false, errorUpdateCount: 0, dataUpdatedAt: 2001 })).toBe(true)
  })
})
