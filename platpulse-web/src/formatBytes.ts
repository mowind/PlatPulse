export function formatBytes(value: number | null | undefined): string {
  if (value == null) return '—'
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let scaled = value
  let unit = 0
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024
    unit += 1
  }
  const digits = unit === 0 || scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2
  return `${scaled.toFixed(digits)} ${units[unit]}`
}

/** Display host CPU without inventing a value for an absent observation. */
export function formatPercent(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return 'Unknown'
  return `${Number.isInteger(value) ? value : value.toFixed(1)}%`
}

/** Display a byte rate with the same units as memory and storage values. */
export function formatBytesPerSecond(value: number | null | undefined): string {
  if (value == null) return 'Unknown'
  return `${formatBytes(value)}/s`
}

/** Same units as memory and storage, but absent observations stay Unknown
 * instead of the em-dash used where a table cell already implies a value. */
export function formatBytesUnknown(value: number | null | undefined): string {
  if (value == null) return 'Unknown'
  return formatBytes(value)
}

/** Keep stable identifiers readable while preserving the complete value in
 * the DOM and in the title attribute. */
export function formatIdentifier(value: string): string {
  if (value.length <= 13) return value
  return `${value.slice(0, 8)}…${value.slice(-4)}`
}
