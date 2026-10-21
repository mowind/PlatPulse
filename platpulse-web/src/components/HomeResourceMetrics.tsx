import { ChevronDown, ChevronUp } from 'lucide-react'
import type { PublicNode } from '../api/generated'
import { nodeDataProgress } from '../nodeData'
import { MetricRow } from './MetricRow'
import { ProgressThin, type ProgressStatus } from './ui/progress-thin'
import { cn } from '../lib/utils'

/** PlatPulse owns no occupancy threshold rule for process CPU/Memory or the
 *  Node data directory, so none is invented here: the ordinary fill uses
 *  Emerald's normal-state success token, the same green family as the online
 *  dot and the theme accent, instead of the informational blue or the default
 *  primary (white in the dark theme). A Server-owned warning/error rule would
 *  replace this, and it would still have to map onto the existing tokens: the
 *  ordinary resource bar keeps the ticket's own success green (#237). */
export const RESOURCE_BAR_STATUS: ProgressStatus = 'success'

/** What the Disk bar measures (#237). The Node data directory's size over the
 *  capacity of the filesystem that holds it is not whole-filesystem fullness —
 *  the Host's own storage percentage is a different observation — so the bar's
 *  accessible name names the source it reads, and the identity explanation
 *  repeats the sentence beside the used / capacity bytes. */
export const DISK_BAR_NAME =
  'Disk: Node data directory size as a share of the filesystem capacity holding it, not whole-filesystem fullness'

/** What the Speed line reads (#246): the rate is the Agent Host's own interface
 *  throughput, collected once per Host and drawn on every Node card that Host
 *  reports for, so it is a shared Host observation and never this Node's own
 *  traffic — one Node's card cannot separate its share of it, and the label says
 *  so rather than letting the word "Speed" imply a per-Node meter. The card keeps
 *  its height: the same sentence is the label's tooltip, and the identity entry
 *  repeats it beside the CPU, Memory and Disk definitions. */
export const HOST_NETWORK_SPEED_NAME =
  "Host network speed: this Agent Host's own interface throughput, shared with every Node the Host reports for — not a rate attributable to this Node alone"

export const RATE_DIRECTION_CLASSES = {
  upload: 'text-emerald-600 dark:text-emerald-400',
  download: 'text-sky-600 dark:text-sky-400',
} as const

type ResourceMetric = {
  metric: 'process-cpu' | 'process-memory' | 'node-data'
  /** The short name the three-column row can hold; the full source name stays
   *  the bar's accessible name and the label's title. */
  label: string
  name: string
  value: string
  progress: number | null
}

export function formatPercent(value: number | null | undefined): string {
  return value == null ? 'Unknown' : `${value.toFixed(1)}%`
}

export function formatRate(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0) return 'Unknown'
  const units = ['bps', 'Kbps', 'Mbps', 'Gbps', 'Tbps']
  let scaled = value * 8
  let unit = 0
  while (scaled >= 1000 && unit < units.length - 1) {
    scaled /= 1000
    unit += 1
  }
  let amount = Number(scaled.toPrecision(3))
  // Rounding at a unit boundary should show 1Mbps, not 1000Kbps.
  if (amount >= 1000 && unit < units.length - 1) {
    amount /= 1000
    unit += 1
  }
  return `${amount}${units[unit]}`
}

/** Home's three resource columns (#237): CPU and Memory are the monitored
 *  PlatON process's own percentages and Disk is the Node data directory's share
 *  of the filesystem capacity that holds it. Every column keeps the Emerald
 *  anatomy — muted label at the left, tabular value flush right on the same
 *  line, one thin success bar underneath — and the three bars share the row
 *  below the values, so a value that cannot share its label's line (an
 *  unattested one, or a very long ratio) drops to its own right-aligned line
 *  instead of overflowing, being clipped, or becoming a zero it never was. */
export function HomeResourceMetrics({ node }: { node: PublicNode }) {
  const nodeDataRatio = nodeDataProgress(node.nodeDataDirectorySizeBytes, node.nodeDataDirectoryCapacityBytes)
  const metrics: ResourceMetric[] = [
    {
      metric: 'process-cpu',
      label: 'CPU',
      name: 'Process CPU',
      value: formatPercent(node.processCpuPercent),
      progress: node.processCpuPercent ?? null,
    },
    // 'Mem' is the shortest honest form of the longest of the three labels: the
    // card's own minimum track must hold the label and its right-aligned
    // percentage on one line, and the full 'Process memory' name stays the
    // label's title and the bar's accessible name (the short-label form the
    // Linked Validator parameters already use).
    {
      metric: 'process-memory',
      label: 'Mem',
      name: 'Process memory',
      value: formatPercent(node.processMemoryPercent),
      progress: node.processMemoryPercent ?? null,
    },
    {
      metric: 'node-data',
      label: 'Disk',
      name: DISK_BAR_NAME,
      value: formatPercent(nodeDataRatio),
      progress: nodeDataRatio,
    },
  ]

  return (
    <div
      data-slot="node-resource-region"
      className="grid grid-cols-3 gap-x-2.5 gap-y-1"
      aria-label="Node process and host network resources"
    >
      {metrics.map(({ metric, label, name, value }) => (
        <div
          key={metric}
          data-slot="resource-cell"
          data-metric={metric}
          className="flex min-w-0 flex-wrap items-baseline gap-x-1 self-end"
        >
          <span data-slot="resource-label" title={name} className="text-[11px] leading-4 text-muted-foreground">
            {label}
          </span>
          <strong data-slot="resource-value" className="ml-auto whitespace-nowrap text-[13px] font-medium tabular-nums">
            {value}
          </strong>
        </div>
      ))}
      {metrics.map(({ metric, name, progress }) => (
        <ProgressThin key={metric} percentage={progress} status={RESOURCE_BAR_STATUS} label={name} />
      ))}
      <div className="col-span-3" data-slot="host-network-speed" role="group" aria-label="Host network speed">
        <MetricRow
          layout="compact"
          label="Speed"
          labelTitle={HOST_NETWORK_SPEED_NAME}
          value={
            <span className="flex items-center gap-2 whitespace-nowrap">
              <span
                aria-label={`Upload ${formatRate(node.hostNetworkTxBytesPerSec)}`}
                className={cn('inline-flex items-center gap-0.5', RATE_DIRECTION_CLASSES.upload)}
              >
                <ChevronUp className="size-3 shrink-0" aria-hidden="true" />
                {formatRate(node.hostNetworkTxBytesPerSec)}
              </span>
              <span
                aria-label={`Download ${formatRate(node.hostNetworkRxBytesPerSec)}`}
                className={cn('inline-flex items-center gap-0.5', RATE_DIRECTION_CLASSES.download)}
              >
                <ChevronDown className="size-3 shrink-0" aria-hidden="true" />
                {formatRate(node.hostNetworkRxBytesPerSec)}
              </span>
            </span>
          }
        />
      </div>
    </div>
  )
}
