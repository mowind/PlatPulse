import { useState } from 'react'
import { act, render } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { BrowserRouter, MemoryRouter } from 'react-router'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { homeReturnState, type HomeReturnState } from '../homeReturn'
import { HomeNodeCard, type HomeNodeCardProps } from './HomeNodeCard'
import { HomeNodeList, HomeNodeListRow, type HomeNodeListRowProps } from './HomeNodeList'
import HomeDashboard from './HomeDashboard'

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

const mockNetwork: PublicNetwork = {
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
  nodes: [mockNodeA, mockNodeB],
}

describe('HomeNodeCard & HomeNodeList memoization (Issue #276)', () => {
  describe('React.memo wrapper verification', () => {
    it('HomeNodeCard is wrapped in React.memo with shallow comparison', () => {
      expect((HomeNodeCard as unknown as { $$typeof: symbol }).$$typeof).toBe(Symbol.for('react.memo'))
      expect(typeof (HomeNodeCard as unknown as { compare: unknown }).compare).toBe('function')
      expect(HomeNodeCard.displayName).toBe('HomeNodeCard')
    })

    it('HomeNodeListRow is wrapped in React.memo with shallow comparison', () => {
      expect((HomeNodeListRow as unknown as { $$typeof: symbol }).$$typeof).toBe(Symbol.for('react.memo'))
      expect(typeof (HomeNodeListRow as unknown as { compare: unknown }).compare).toBe('function')
      expect(HomeNodeListRow.displayName).toBe('HomeNodeListRow')
    })

    it('HomeNodeList is wrapped in React.memo with shallow comparison', () => {
      expect((HomeNodeList as unknown as { $$typeof: symbol }).$$typeof).toBe(Symbol.for('react.memo'))
      expect((HomeNodeList as unknown as { compare: unknown }).compare).toBeNull()
      expect(HomeNodeList.displayName).toBe('HomeNodeList')
    })
  })

  describe('HomeNodeCard unit memoization behavior', () => {
    it('does not re-render HomeNodeCard when props are unchanged', () => {
      let renderCount = 0
      const OriginalType = (HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type
      ;(HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type = (props) => {
        renderCount++
        return OriginalType(props)
      }

      try {
        const linkState = homeReturnState('node-a', new URLSearchParams(), 0, 1, 'tab-1')
        const { rerender } = render(
          <MemoryRouter>
            <HomeNodeCard network={mockNetwork} node={mockNodeA} linkState={linkState} />
          </MemoryRouter>,
        )

        expect(renderCount).toBe(1)

        // Rerender with the identical prop references
        rerender(
          <MemoryRouter>
            <HomeNodeCard network={mockNetwork} node={mockNodeA} linkState={linkState} />
          </MemoryRouter>,
        )

        // Must not re-render
        expect(renderCount).toBe(1)

        // Rerender with updated node reference
        const updatedNode = { ...mockNodeA, currentHead: 101 }
        rerender(
          <MemoryRouter>
            <HomeNodeCard network={mockNetwork} node={updatedNode} linkState={linkState} />
          </MemoryRouter>,
        )

        // Must re-render once
        expect(renderCount).toBe(2)
      } finally {
        ;(HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type = OriginalType
      }
    })
  })

  describe('HomeNodeListRow unit memoization behavior', () => {
    it('does not re-render HomeNodeListRow when props are unchanged', () => {
      let renderCount = 0
      const OriginalType = (HomeNodeListRow as unknown as { type: (props: HomeNodeListRowProps) => React.ReactNode }).type
      ;(HomeNodeListRow as unknown as { type: (props: HomeNodeListRowProps) => React.ReactNode }).type = (props) => {
        renderCount++
        return OriginalType(props)
      }

      try {
        const linkState = homeReturnState('node-a', new URLSearchParams(), 0, 1, 'tab-1')
        const registerRow = vi.fn()
        const { rerender } = render(
          <MemoryRouter>
            <table>
              <tbody>
                <HomeNodeListRow
                  network={mockNetwork}
                  node={mockNodeA}
                  linkState={linkState}
                  registerRow={registerRow}
                />
              </tbody>
            </table>
          </MemoryRouter>,
        )

        expect(renderCount).toBe(1)

        // Rerender with identical props
        rerender(
          <MemoryRouter>
            <table>
              <tbody>
                <HomeNodeListRow
                  network={mockNetwork}
                  node={mockNodeA}
                  linkState={linkState}
                  registerRow={registerRow}
                />
              </tbody>
            </table>
          </MemoryRouter>,
        )

        expect(renderCount).toBe(1)

        // Rerender with updated node
        const updatedNode = { ...mockNodeA, currentHead: 101 }
        rerender(
          <MemoryRouter>
            <table>
              <tbody>
                <HomeNodeListRow
                  network={mockNetwork}
                  node={updatedNode}
                  linkState={linkState}
                  registerRow={registerRow}
                />
              </tbody>
            </table>
          </MemoryRouter>,
        )

        expect(renderCount).toBe(2)
      } finally {
        ;(HomeNodeListRow as unknown as { type: (props: HomeNodeListRowProps) => React.ReactNode }).type = OriginalType
      }
    })
  })

  describe('HomeDashboard SSE & partial node updates', () => {
    it('preserves HomeNodeCard instances and skips re-renders for unchanged nodes when SSE updates another node', async () => {
      const renders = { 'node-a': 0, 'node-b': 0 }

      const OriginalCardType = (HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type
      ;(HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type = (props) => {
        if (props.node.nodeId === 'node-a' || props.node.nodeId === 'node-b') {
          renders[props.node.nodeId as 'node-a' | 'node-b']++
        }
        return OriginalCardType(props)
      }

      try {
        let setNetworksFn!: (networks: PublicNetwork[]) => void
        let setRealtimeStatusFn!: (status: 'connecting' | 'connected' | 'disconnected') => void

        function TestContainer() {
          const [networks, setNetworks] = useState<PublicNetwork[]>([mockNetwork])
          const [realtimeStatus, setRealtimeStatus] = useState<'connecting' | 'connected' | 'disconnected'>('connected')
          setNetworksFn = setNetworks
          setRealtimeStatusFn = setRealtimeStatus

          return (
            <HomeDashboard
              networks={networks}
              realtimeStatus={realtimeStatus}
              online
              resetting={false}
              error={null}
              loading={false}
            />
          )
        }

        render(
          <BrowserRouter>
            <TestContainer />
          </BrowserRouter>,
        )

        // First render mounts both cards
        expect(renders['node-a']).toBe(1)
        expect(renders['node-b']).toBe(1)

        // 1. Parent re-renders due to realtime status (heartbeat / sse status), data unchanged
        await act(async () => {
          setRealtimeStatusFn('connecting')
        })

        // Unchanged nodes must NOT re-render
        expect(renders['node-a']).toBe(1)
        expect(renders['node-b']).toBe(1)

        // 2. SSE pushes new projection: Node B changes, Node A is unchanged (structural sharing)
        const updatedNodeB: PublicNode = {
          ...mockNodeB,
          currentHead: 105,
        }
        const updatedNetwork: PublicNetwork = {
          ...mockNetwork,
          nodes: [mockNodeA, updatedNodeB], // mockNodeA reference is identical
        }

        await act(async () => {
          setNetworksFn([updatedNetwork])
        })

        // Node A was unchanged -> 0 additional renders
        expect(renders['node-a']).toBe(1)
        // Node B had updated metrics -> exactly 1 re-render
        expect(renders['node-b']).toBe(2)
      } finally {
        ;(HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type = OriginalCardType
      }
    })

    it('stabilizes linkState object references across renders when position and query are unchanged', async () => {
      const capturedLinkStates: HomeReturnState[] = []

      const OriginalCardType = (HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type
      ;(HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type = (props) => {
        if (props.node.nodeId === 'node-a') {
          capturedLinkStates.push(props.linkState)
        }
        return OriginalCardType(props)
      }

      try {
        let setRealtimeStatusFn!: (status: 'connecting' | 'connected' | 'disconnected') => void

        function TestContainer() {
          const [realtimeStatus, setRealtimeStatus] = useState<'connecting' | 'connected' | 'disconnected'>('connected')
          setRealtimeStatusFn = setRealtimeStatus
          return (
            <HomeDashboard
              networks={[mockNetwork]}
              realtimeStatus={realtimeStatus}
              online
              resetting={false}
              error={null}
              loading={false}
            />
          )
        }

        render(
          <BrowserRouter>
            <TestContainer />
          </BrowserRouter>,
        )

        expect(capturedLinkStates.length).toBe(1)

        await act(async () => {
          setRealtimeStatusFn('connecting')
        })

        // Because HomeNodeCard was memoized and linkState reference was identical,
        // HomeNodeCard was not re-rendered.
        expect(capturedLinkStates.length).toBe(1)
      } finally {
        ;(HomeNodeCard as unknown as { type: (props: HomeNodeCardProps) => React.ReactNode }).type = OriginalCardType
      }
    })

    it('skips re-renders for unchanged rows in list view when SSE updates another node', async () => {
      const renders = { 'node-a': 0, 'node-b': 0 }

      const OriginalRowType = (HomeNodeListRow as unknown as { type: (props: HomeNodeListRowProps) => React.ReactNode }).type
      ;(HomeNodeListRow as unknown as { type: (props: HomeNodeListRowProps) => React.ReactNode }).type = (props) => {
        if (props.node.nodeId === 'node-a' || props.node.nodeId === 'node-b') {
          renders[props.node.nodeId as 'node-a' | 'node-b']++
        }
        return OriginalRowType(props)
      }

      try {
        let setNetworksFn!: (networks: PublicNetwork[]) => void

        function TestContainer() {
          const [networks, setNetworks] = useState<PublicNetwork[]>([mockNetwork])
          setNetworksFn = setNetworks
          return (
            <HomeDashboard
              networks={networks}
              realtimeStatus="connected"
              online
              resetting={false}
              error={null}
              loading={false}
            />
          )
        }

        render(
          <MemoryRouter initialEntries={['/?view=list']}>
            <TestContainer />
          </MemoryRouter>,
        )

        expect(renders['node-a']).toBe(1)
        expect(renders['node-b']).toBe(1)

        // SSE update: Node B updated, Node A unchanged
        const updatedNodeB: PublicNode = {
          ...mockNodeB,
          currentHead: 120,
        }
        const updatedNetwork: PublicNetwork = {
          ...mockNetwork,
          nodes: [mockNodeA, updatedNodeB],
        }

        await act(async () => {
          setNetworksFn([updatedNetwork])
        })

        expect(renders['node-a']).toBe(1)
        expect(renders['node-b']).toBe(2)
      } finally {
        ;(HomeNodeListRow as unknown as { type: (props: HomeNodeListRowProps) => React.ReactNode }).type = OriginalRowType
      }
    })
  })
})
