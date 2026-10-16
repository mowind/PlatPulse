import { describe, expect, it, vi } from 'vitest'
import { memoizeLoader } from './echartsCore'

// The deck's loader contract. ECharts itself is never imported here: the module
// builds its dynamic imports inside the loader, so this file exercises the memo
// around them and leaves the charts to nodeMetricChart.test.ts.
describe('on-demand ECharts loader', () => {
  it('imports once and hands the same core to every later caller', async () => {
    const load = vi.fn(() => Promise.resolve('core'))
    const loader = memoizeLoader(load)
    expect(await loader()).toBe('core')
    expect(await loader()).toBe('core')
    expect(load).toHaveBeenCalledTimes(1)
  })

  it('shares one in-flight attempt between concurrent mounts', async () => {
    let settle: ((value: string) => void) | undefined
    const load = vi.fn(() => new Promise<string>((resolve) => { settle = resolve }))
    const loader = memoizeLoader(load)
    const first = loader()
    const second = loader()
    expect(load).toHaveBeenCalledTimes(1)
    settle?.('core')
    expect(await first).toBe('core')
    expect(await second).toBe('core')
    expect(load).toHaveBeenCalledTimes(1)
  })

  it('forgets a rejected import so a later mount retries instead of inheriting it', async () => {
    const load = vi.fn()
      .mockRejectedValueOnce(new Error('Failed to fetch dynamically imported module: echarts'))
      .mockResolvedValue('core')
    const loader = memoizeLoader(load)
    await expect(loader()).rejects.toThrow('Failed to fetch dynamically imported module: echarts')
    // One transient chunk failure must not blank six charts for the session.
    expect(await loader()).toBe('core')
    expect(load).toHaveBeenCalledTimes(2)
  })
})
