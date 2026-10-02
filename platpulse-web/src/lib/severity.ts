function capitalize(value: string): string {
  return value.length === 0 ? value : value.charAt(0).toUpperCase() + value.slice(1)
}

/**
 * Severity wording and tone, shared by the Incident pages and the Rule pages
 * (issue #204 review, Standards #2) so the two surfaces cannot drift on how a
 * Server severity is displayed.
 */
export function severityLabel(severity: string): string {
  if (severity === 'critical') return 'Critical'
  if (severity === 'warning') return 'Warning'
  if (severity === 'info') return 'Info'
  return capitalize(severity)
}

export function severityTone(severity: string): 'error' | 'warning' | 'neutral' {
  if (severity === 'critical') return 'error'
  if (severity === 'warning') return 'warning'
  return 'neutral'
}
