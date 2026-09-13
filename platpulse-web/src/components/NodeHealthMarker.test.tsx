import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { NodeHealthMarker, nodeHealthLabel } from './StatusBadge'

afterEach(cleanup)

/**
 * Issue #141: the Home card, Network Overview card, and Node Detail hero drop
 * the Node Health text badge for a two-state marker before the Node name. The
 * marker carries the Server-owned health word as its accessible name, renders
 * Healthy green, and renders every other state grey.
 */
describe('NodeHealthMarker', () => {
  it('renders Healthy as a green marker named Healthy', () => {
    render(<NodeHealthMarker health="healthy" />)

    const marker = screen.getByRole('img', { name: 'Healthy' })
    expect(marker.classList.contains('node-health-marker-healthy')).toBe(true)
    expect(marker.classList.contains('node-health-marker-other')).toBe(false)
  })

  const otherStates: Array<[string | null | undefined, string]> = [
    ['unhealthy', 'Unhealthy'],
    ['UNHEALTHY', 'Unhealthy'],
    ['unknown', 'Unknown'],
    ['', 'Unknown'],
    [null, 'Unknown'],
    [undefined, 'Unknown'],
  ]

  it.each(otherStates)('renders %s as a grey marker named %s', (health, label) => {
    render(<NodeHealthMarker health={health} />)

    const marker = screen.getByRole('img', { name: label })
    expect(marker.classList.contains('node-health-marker-other')).toBe(true)
    expect(marker.classList.contains('node-health-marker-healthy')).toBe(false)
  })

  it('maps Server health values onto the three health words', () => {
    expect(nodeHealthLabel('healthy')).toBe('Healthy')
    expect(nodeHealthLabel('unhealthy')).toBe('Unhealthy')
    expect(nodeHealthLabel('unknown')).toBe('Unknown')
    expect(nodeHealthLabel('degraded')).toBe('Unknown')
    expect(nodeHealthLabel(null)).toBe('Unknown')
  })
})
