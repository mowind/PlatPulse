export function formatDuration(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return 'Unknown'
  const totalSeconds = Math.floor(value / 1000)
  const days = Math.floor(totalSeconds / 86_400)
  const hours = Math.floor((totalSeconds % 86_400) / 3_600)
  const minutes = Math.floor((totalSeconds % 3_600) / 60)
  if (days > 0) return days + 'd ' + hours + 'h'
  if (hours > 0) return hours + 'h ' + minutes + 'm'
  if (minutes > 0) return minutes + 'm'
  return totalSeconds + 's'
}
