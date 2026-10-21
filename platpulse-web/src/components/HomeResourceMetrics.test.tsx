import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { HomeResourceMetrics, formatPercent, formatRate } from './HomeResourceMetrics'
import type { PublicNode } from '../api/generated'

afterEach(cleanup)

const baseNode = {
  nodeId: 'node-1',
  displayName: 'Node 1',
  networkKey: 'mainnet',
  health: 'healthy',
  processCpuPercent: 12.34,
  processMemoryPercent: 45.67,
  nodeDataDirectorySizeBytes: 100,
  nodeDataDirectoryCapacityBytes: 400,
  hostNetworkTxBytesPerSec: 1024,
  hostNetworkRxBytesPerSec: 2048,
  currentHead: 1000,
} as unknown as PublicNode

describe('HomeResourceMetrics', () => {
  it('formats percentages with one decimal place or Unknown', () => {
    expect(formatPercent(12.34)).toBe('12.3%')
    expect(formatPercent(null)).toBe('Unknown')
    expect(formatPercent(undefined)).toBe('Unknown')
  })

  it('formats network rates correctly across unit boundaries', () => {
    expect(formatRate(null)).toBe('Unknown')
    expect(formatRate(-10)).toBe('Unknown')
    expect(formatRate(100)).toBe('800bps')
    expect(formatRate(125000)).toBe('1Mbps')
  })

  it('renders CPU, Mem, Disk progress cells and host network speed', () => {
    const { container } = render(<HomeResourceMetrics node={baseNode} />)

    expect(screen.getByText('CPU')).toBeDefined()
    expect(screen.getByText('Mem')).toBeDefined()
    expect(screen.getByText('Disk')).toBeDefined()
    expect(screen.getByText('Speed')).toBeDefined()

    expect(screen.getByText('12.3%')).toBeDefined()
    expect(screen.getByText('45.7%')).toBeDefined()
    expect(screen.getByText('25.0%')).toBeDefined()

    const region = container.querySelector('[data-slot="node-resource-region"]')
    expect(region).not.toBeNull()
    const speed = container.querySelector('[data-slot="host-network-speed"]')
    expect(speed).not.toBeNull()
  })
})
