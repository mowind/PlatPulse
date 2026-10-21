import { Link } from 'react-router'
import type { PublicNetwork, PublicNode } from '../api/generated'
import { NodeHealthMarker } from './StatusBadge'
import { homeNodeLabel } from '../homeFilters'
import type { HomeReturnState } from '../homeReturn'
import { formatNumber, healthLabel, peerRetentionCue, formatPeerCount } from './HomeNodeCard'
import { formatPercent } from './HomeResourceMetrics'

export type NodeRecord = { network: PublicNetwork; node: PublicNode }
export type HomeReturnLinkState = (nodeId: string, index: number, count: number) => HomeReturnState
export type NodeRowRefs = (nodeId: string, element: HTMLElement | null) => void

export type HomeNodeListProps = {
  records: readonly NodeRecord[]
  linkState: HomeReturnLinkState
  registerRow: NodeRowRefs
}

/** The compact list view (stories 72, 73, 79): one row per matching Node with
 * the seven public columns this ticket names, inside one local horizontal
 * scroller whose Node name and health word stay pinned (emerald.css owns those
 * two columns and their offsets). Every value comes from the same public field
 * and the same formatter the card uses, so a metric the Server never attested
 * still reads Unknown here and a real zero still reads 0. QC, Locked and
 * Committed stay card-only columns, exactly as the design says. */
export function HomeNodeList({ records, linkState, registerRow }: HomeNodeListProps) {
  return (
    <div data-slot="node-list-scroll" className="overflow-x-auto rounded-md bg-background">
      <table data-slot="node-list" aria-label="Active Nodes" className="w-full text-sm">
        <thead>
          <tr>
            <th scope="col" data-column="name" className="text-left">
              Node
            </th>
            <th scope="col" data-column="network" className="text-left">
              Network
            </th>
            <th scope="col" data-column="health" className="text-left">
              Health
            </th>
            <th scope="col" data-column="head" className="text-right">
              Current Head
            </th>
            <th scope="col" data-column="peers" className="text-right">
              Peers
            </th>
            <th scope="col" data-column="process-cpu" className="text-right">
              Process CPU
            </th>
            <th scope="col" data-column="process-memory" className="text-right">
              Process memory
            </th>
          </tr>
        </thead>
        <tbody>
          {records.map(({ network, node }, index) => (
            <tr key={node.nodeId} data-slot="node-list-row" ref={(element) => registerRow(node.nodeId, element)}>
              <td data-column="name">
                <Link
                  to={'/nodes/' + node.nodeId}
                  state={linkState(node.nodeId, index, records.length)}
                  data-slot="node-link"
                  aria-label={homeNodeLabel(node)}
                  title={homeNodeLabel(node)}
                  className="inline-flex min-h-11 min-w-11 max-w-full items-center rounded-sm focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                >
                  <span className="truncate">{homeNodeLabel(node)}</span>
                </Link>
              </td>
              <td data-column="network" className="text-muted-foreground">
                <span className="block truncate" title={network.displayName}>
                  {network.displayName}
                </span>
              </td>
              <td data-column="health">
                <span className="inline-flex items-center gap-1.5 whitespace-nowrap">
                  <NodeHealthMarker health={node.health} />
                  {healthLabel(node.health)}
                </span>
              </td>
              <td data-column="head" className="text-right tabular-nums">
                {formatNumber(node.currentHead)}
              </td>
              <td data-column="peers" className="text-right tabular-nums">
                {formatPeerCount(node)}
                {peerRetentionCue(node) && (
                  <small data-slot="node-list-peers-cue" className="block text-[11px] text-muted-foreground">
                    {peerRetentionCue(node)}
                  </small>
                )}
              </td>
              <td data-column="process-cpu" className="text-right tabular-nums">
                {formatPercent(node.processCpuPercent)}
              </td>
              <td data-column="process-memory" className="text-right tabular-nums">
                {formatPercent(node.processMemoryPercent)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
