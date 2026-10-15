import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { NodeHealthMarker, nodeHealthLabel } from './StatusBadge'

afterEach(cleanup)

/**
 * Issue #141, round 2: the Home card, Network Overview card, and Node Detail
 * hero drop the Node Health text badge for a marker before the Node name. The
 * marker carries the Server-owned health word as its accessible name and shows
 * three distinct tones - Healthy green, Unhealthy in the error tone, and
 * Unknown in the warning tone. Missing, stale, or never-observed evidence is a
 * warning, never a confirmed failure, and never Offline.
 */
describe('NodeHealthMarker', () => {
  it('renders Healthy as a green marker named Healthy', () => {
    render(<NodeHealthMarker health="healthy" />)

    const marker = screen.getByRole('img', { name: 'Healthy' })
    expect(marker.classList.contains('node-health-marker-healthy')).toBe(true)
    expect(marker.getAttribute('data-tone')).toBe('healthy')
    expect(marker.classList.contains('bg-emerald-600')).toBe(true)
  })

  const unhealthyStates: Array<[string | null | undefined, string]> = [
    ['unhealthy', 'Unhealthy'],
    ['UNHEALTHY', 'Unhealthy'],
  ]

  it.each(unhealthyStates)('renders %s as an error-tone marker named %s', (health, label) => {
    render(<NodeHealthMarker health={health} />)

    const marker = screen.getByRole('img', { name: label })
    expect(marker.classList.contains('node-health-marker-unhealthy')).toBe(true)
    expect(marker.getAttribute('data-tone')).toBe('unhealthy')
    expect(marker.classList.contains('bg-destructive')).toBe(true)
  })

  const unknownStates: Array<[string | null | undefined, string]> = [
    ['unknown', 'Unknown'],
    ['degraded', 'Unknown'],
    ['', 'Unknown'],
    [null, 'Unknown'],
    [undefined, 'Unknown'],
  ]

  it.each(unknownStates)('renders %s as a warning-tone marker named %s', (health, label) => {
    render(<NodeHealthMarker health={health} />)

    const marker = screen.getByRole('img', { name: label })
    expect(marker.classList.contains('node-health-marker-unknown')).toBe(true)
    expect(marker.getAttribute('data-tone')).toBe('unknown')
    expect(marker.classList.contains('bg-warning')).toBe(true)
    expect(marker.classList.contains('bg-destructive')).toBe(false)
  })

  it('leaves the retired two-state grey class on no tone', () => {
    for (const health of ['healthy', 'unhealthy', 'unknown', null]) {
      render(<NodeHealthMarker health={health} />)
      expect(document.querySelector('.node-health-marker-other')).toBeNull()
    }
  })

  it('maps Server health values onto the three health words', () => {
    expect(nodeHealthLabel('healthy')).toBe('Healthy')
    expect(nodeHealthLabel('unhealthy')).toBe('Unhealthy')
    expect(nodeHealthLabel('unknown')).toBe('Unknown')
    expect(nodeHealthLabel('degraded')).toBe('Unknown')
    expect(nodeHealthLabel(null)).toBe('Unknown')
  })
})
