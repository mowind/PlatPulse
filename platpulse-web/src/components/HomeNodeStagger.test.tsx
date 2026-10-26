import { describe, expect, it } from 'vitest'
import { act, render } from '@testing-library/react'
import { useState } from 'react'
import { createMemoryRouter, RouterProvider } from 'react-router'
import {
  nodeCardStaggerDelay,
  NODE_CARD_STAGGER_STEP_MS,
  NODE_CARD_MAX_DELAY_MS,
} from './nodeCardStagger'
import HomeDashboard from './HomeDashboard'
import type { PublicNetwork, PublicNode } from '../api/generated'

type NodeProcess = {
  cwd?: () => string
  getBuiltinModule?: (name: string) => {
    readFileSync?: (path: string, encoding: string) => string
    resolve?: (...args: string[]) => string
  }
}

function readEmeraldCss(): string {
  const proc = (globalThis as { process?: NodeProcess }).process
  const fs = proc?.getBuiltinModule?.('node:fs')
  const path = proc?.getBuiltinModule?.('node:path')
  if (!fs?.readFileSync || !path?.resolve || !proc?.cwd) {
    throw new Error('Node filesystem modules unavailable in test environment')
  }
  const cwd = proc.cwd()
  const basePath = cwd.endsWith('platpulse-web') ? cwd : path.resolve(cwd, 'platpulse-web')
  return fs.readFileSync(path.resolve(basePath, 'src/styles/emerald.css'), 'utf8')
}

const emeraldCss = readEmeraldCss()

const mockNodeA: PublicNode = {
  nodeId: 'node-a',
  displayName: 'Alpha',
  networkKey: 'mainnet',
  health: 'healthy',
  healthReason: 'RPC reachable',
  rpcState: 'connected',
  syncState: 'synced',
  consensusState: 'ready',
  processState: 'running',
  resyncState: 'idle',
  currentHead: 100,
  latestBlockTransactionCount: 10,
  historicalHighWatermark: 100,
  networkReferenceHead: 100,
  networkReferenceConfidence: 'high',
  freshness: 'current',
  resyncProgress: null,
  hostCpuPercent: 50,
  hostMemoryPercent: 50,
  processCpuPercent: 10,
  processMemoryPercent: 20,
  hostStoragePercent: 60,
  nodeDataDirectorySizeBytes: 1000,
  nodeDataDirectoryCapacityBytes: 2000,
  hostNetworkRxBytesPerSec: 100,
  hostNetworkTxBytesPerSec: 200,
  peers: { state: 'ok', freshness: 'current', peerCount: 5 },
  peerCountries: { scope: 'unavailable', countries: [] },
  consensus: {
    state: 'ok',
    freshness: 'current',
    observedAt: '2026-10-25T00:00:00Z',
    receivedAt: '2026-10-25T00:00:00Z',
    epoch: 1,
    viewNumber: 1,
    validator: true,
    highestQcBlock: 90,
    highestLockBlock: 89,
    highestCommitBlock: 88,
  },
  validator: null,
}

const mockNodeB: PublicNode = {
  ...mockNodeA,
  nodeId: 'node-b',
  displayName: 'Beta',
  currentHead: 99,
}

const mockNodeC: PublicNode = {
  ...mockNodeA,
  nodeId: 'node-c',
  displayName: 'Gamma',
  health: 'unhealthy',
  currentHead: 98,
}

const mockNodeTestnet: PublicNode = {
  ...mockNodeA,
  nodeId: 'node-testnet-1',
  displayName: 'Delta Testnet',
  networkKey: 'testnet',
  currentHead: 50,
}

const mockNetworkMainnet: PublicNetwork = {
  networkKey: 'mainnet',
  displayName: 'Mainnet',
  geo: { state: 'disabled', scope: 'unavailable' },
  peers: { state: 'unsupported', freshness: 'unknown' },
  validators: [],
  validatorSummary: {
    blocks: { knownSum: '100', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    rewards: { knownSum: '10', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    eligibleValidatorCount: 1,
    linkedNodeCount: 1,
    unlinkedNodeCount: 1,
  },
  nodes: [mockNodeA, mockNodeB, mockNodeC],
}

const mockNetworkTestnet: PublicNetwork = {
  networkKey: 'testnet',
  displayName: 'Testnet',
  geo: { state: 'disabled', scope: 'unavailable' },
  peers: { state: 'unsupported', freshness: 'unknown' },
  validators: [],
  validatorSummary: {
    blocks: { knownSum: '50', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    rewards: { knownSum: '5', expectedCount: 1, valuedCount: 1, staleCount: 0, state: 'complete' },
    eligibleValidatorCount: 1,
    linkedNodeCount: 1,
    unlinkedNodeCount: 1,
  },
  nodes: [mockNodeTestnet],
}

function renderHome(networks: PublicNetwork[], search = '', initiallyLoading = false) {
  let finishLoading = () => {}
  function Harness() {
    const [loading, setLoading] = useState(initiallyLoading)
    finishLoading = () => setLoading(false)
    return (
      <HomeDashboard
        networks={loading ? [] : networks}
        realtimeStatus="connected"
        online
        resetting={false}
        error={null}
        loading={loading}
      />
    )
  }
  const router = createMemoryRouter([{ path: '/', element: <Harness /> }], {
    initialEntries: [{ pathname: '/', search }],
  })
  return { ...render(<RouterProvider router={router} />), router, finishLoading: () => finishLoading() }
}



describe('Node card staggered entrance animation (Issue #278)', () => {
  describe('nodeCardStaggerDelay formula and boundaries', () => {
    it('uses 25ms per item step matching Emerald specification', () => {
      expect(NODE_CARD_STAGGER_STEP_MS).toBe(25)
    })

    it('caps maximum delay within 200~300ms boundary', () => {
      expect(NODE_CARD_MAX_DELAY_MS).toBeGreaterThanOrEqual(200)
      expect(NODE_CARD_MAX_DELAY_MS).toBeLessThanOrEqual(300)
    })

    it('calculates delay accurately for early items', () => {
      expect(nodeCardStaggerDelay(0)).toBe(0)
      expect(nodeCardStaggerDelay(1)).toBe(25)
      expect(nodeCardStaggerDelay(2)).toBe(50)
      expect(nodeCardStaggerDelay(3)).toBe(75)
    })

    it('caps delay for tail items with large indices', () => {
      expect(nodeCardStaggerDelay(10)).toBe(NODE_CARD_MAX_DELAY_MS)
      expect(nodeCardStaggerDelay(50)).toBe(NODE_CARD_MAX_DELAY_MS)
      expect(nodeCardStaggerDelay(100)).toBe(NODE_CARD_MAX_DELAY_MS)
    })

    it('safely handles negative indices', () => {
      expect(nodeCardStaggerDelay(-1)).toBe(0)
    })
  })

  describe('HomeDashboard node-grid card frame staggered styling', () => {
    it('assigns staggered --node-item-delay CSS variable to each card frame in order', () => {
      const { container } = renderHome([mockNetworkMainnet])
      const grid = container.querySelector('[data-slot="node-grid"]') as HTMLElement
      expect(grid).not.toBeNull()

      const frames = grid.querySelectorAll<HTMLElement>('[data-slot="node-card-frame"]')
      expect(frames.length).toBe(3)

      expect(frames[0].style.getPropertyValue('--node-item-delay')).toBe('0ms')
      expect(frames[1].style.getPropertyValue('--node-item-delay')).toBe('25ms')
      expect(frames[2].style.getPropertyValue('--node-item-delay')).toBe('50ms')
    })

    it('recalculates staggered delay starting from 0 when search filter narrows matching cards', () => {
      const { container } = renderHome([mockNetworkMainnet], '?q=Gamma')
      const grid = container.querySelector('[data-slot="node-grid"]') as HTMLElement
      expect(grid).not.toBeNull()

      const frames = grid.querySelectorAll<HTMLElement>('[data-slot="node-card-frame"]')
      expect(frames.length).toBe(1)
      // Filtered to only Gamma, its index in matching is 0, so delay is 0ms
      expect(frames[0].style.getPropertyValue('--node-item-delay')).toBe('0ms')
    })

    it('recalculates staggered delay when health filter narrows matching cards', () => {
      const { container } = renderHome([mockNetworkMainnet], '?health=unhealthy')
      const grid = container.querySelector('[data-slot="node-grid"]') as HTMLElement
      expect(grid).not.toBeNull()

      const frames = grid.querySelectorAll<HTMLElement>('[data-slot="node-card-frame"]')
      expect(frames.length).toBe(1)
      expect(frames[0].style.getPropertyValue('--node-item-delay')).toBe('0ms')
    })

    it('updates node-grid and sets staggered delay when scoped to a specific network tab', () => {
      const { container } = renderHome([mockNetworkMainnet, mockNetworkTestnet], '?network=testnet')
      const grid = container.querySelector('[data-slot="node-grid"]') as HTMLElement
      expect(grid).not.toBeNull()

      const testnetFrames = grid.querySelectorAll<HTMLElement>('[data-slot="node-card-frame"]')
      expect(testnetFrames.length).toBe(1)
      expect(testnetFrames[0].style.getPropertyValue('--node-item-delay')).toBe('0ms')
    })
  })

  describe('entrance trigger (webui.md §15.39: Network/filter change only)', () => {
    const entrance = (container: HTMLElement) =>
      container.querySelector('[data-slot="node-grid"]')?.getAttribute('data-entrance') ?? null

    it('shows the first grid at rest, including after the projection finishes loading', () => {
      expect(entrance(renderHome([mockNetworkMainnet]).container)).toBeNull()

      const loading = renderHome([mockNetworkMainnet], '', true)
      expect(loading.container.querySelector('[data-slot="node-grid"]')).toBeNull()
      act(() => loading.finishLoading())
      expect(loading.container.querySelector('[data-slot="node-grid"]')).not.toBeNull()
      expect(entrance(loading.container)).toBeNull()
    })

    it('staggers the cards when a filter or Network change remounts the grid', async () => {
      const { container, router } = renderHome([mockNetworkMainnet, mockNetworkTestnet])
      expect(entrance(container)).toBeNull()

      await act(() => router.navigate('/?network=testnet'))
      expect(entrance(container)).toBe('stagger')

      await act(() => router.navigate('/?health=unhealthy'))
      expect(entrance(container)).toBe('stagger')
    })
  })

  describe('emerald.css animation keyframes and accessibility compliance', () => {
    it('defines @keyframes node-card-enter with translateY, scale, and opacity', () => {
      expect(emeraldCss).toContain('@keyframes node-card-enter')
      // Must include opacity, translateY, and scale transitions
      expect(emeraldCss).toMatch(/transform:\s*translateY\(\d+px\)\s*scale\([\d.]+\)/)
      expect(emeraldCss).toMatch(/opacity:\s*0/)
      expect(emeraldCss).toMatch(/opacity:\s*1/)
    })

    it('binds animation to [data-slot="node-card-frame"] with var(--node-item-delay) and backwards fill-mode', () => {
      expect(emeraldCss).toContain("[data-slot='node-grid'][data-entrance='stagger'] > [data-slot='node-card-frame']")
      expect(emeraldCss).toContain('animation-name: node-card-enter')
      expect(emeraldCss).toContain('var(--node-item-delay, 0ms)')
      expect(emeraldCss).toContain('animation-fill-mode: backwards')
    })

    it('strictly complies with prefers-reduced-motion to disable animations and transitions', () => {
      const reducedMotionMatch = emeraldCss.match(/@media\s*\(prefers-reduced-motion:\s*reduce\)\s*\{([\s\S]*?)\n\}/)
      expect(reducedMotionMatch).not.toBeNull()

      const reducedBlock = reducedMotionMatch?.[1] ?? ''
      expect(reducedBlock).toContain("[data-slot='node-card-frame']")
      expect(reducedBlock).toMatch(/\[data-slot='node-card-frame'\]\s*\{[\s\S]*?animation:\s*none\s*!important/)
      expect(reducedBlock).toMatch(/\[data-slot='node-card-frame'\]\s*\{[\s\S]*?transform:\s*none\s*!important/)
      expect(reducedBlock).toMatch(/\[data-slot='node-card-frame'\]\s*\{[\s\S]*?opacity:\s*1\s*!important/)
    })
  })
})
