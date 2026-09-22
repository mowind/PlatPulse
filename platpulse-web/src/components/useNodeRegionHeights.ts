import { useLayoutEffect, useRef } from 'react'

/** Share intrinsic region heights across the whole Node grid, not just one row.
 * Observe inner content only: the shared minima belong to its outer wrappers,
 * so a previous maximum cannot feed back into the next measurement. */
export function useNodeRegionHeights(records: readonly object[], visible: boolean) {
  const ref = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const grid = ref.current
    if (!grid || typeof ResizeObserver === 'undefined') return
    const regions = ['identity', 'resource', 'chain', 'validator'] as const
    const contents = regions.map(region => [...grid.querySelectorAll<HTMLElement>(
      `[data-node-region="${region}"] > [data-node-region-content]`,
    )])
    const measure = () => {
      regions.forEach((region, index) => {
        const height = Math.max(0, ...contents[index].map(el => el.getBoundingClientRect().height))
        grid.style.setProperty(`--node-${region}-h`, `${height}px`)
      })
    }
    measure()
    const observer = new ResizeObserver(measure)
    contents.flat().forEach(el => observer.observe(el))
    return () => observer.disconnect()
  }, [records, visible])
  return ref
}
