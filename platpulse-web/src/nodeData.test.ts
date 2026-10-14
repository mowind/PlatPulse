import { describe, expect, it } from 'vitest'
import { nodeDataProgress } from './nodeData'

/** The one Node data ratio the Home card, the Node Detail hero and the Home Disk
 *  sort all read (issues #140 and #242): a share needs two measured byte counts,
 *  and anything else is Unknown rather than a number no reader can stand behind. */
describe('Node data ratio', () => {
  it('reads a measured directory against a measured capacity, a zero included', () => {
    expect(nodeDataProgress(0, 200)).toBe(0)
    expect(nodeDataProgress(100, 200)).toBe(50)
    expect(nodeDataProgress(268_435_456_000, 536_870_912_000)).toBe(50)
  })

  it('is Unknown for every pair a capacity cannot divide', () => {
    // Absent on either side, or a capacity that holds nothing.
    expect(nodeDataProgress(null, 200)).toBeNull()
    expect(nodeDataProgress(undefined, 200)).toBeNull()
    expect(nodeDataProgress(100, null)).toBeNull()
    expect(nodeDataProgress(100, 0)).toBeNull()
    expect(nodeDataProgress(100, -200)).toBeNull()
    // Byte counts no Server can attest: a negative directory, or either side
    // carrying something other than a finite measurement.
    expect(nodeDataProgress(-100, 200)).toBeNull()
    expect(nodeDataProgress(Number.NaN, 200)).toBeNull()
    expect(nodeDataProgress(100, Number.NaN)).toBeNull()
    expect(nodeDataProgress(Number.POSITIVE_INFINITY, 200)).toBeNull()
    expect(nodeDataProgress(100, Number.POSITIVE_INFINITY)).toBeNull()
  })
})
