import { Wallet } from 'lucide-react'
import type { PublicRewardWallet } from '../api/generated'
import { formatAmountTwoDecimals } from '../lib/amount'
import { isNonnegativeDecimal } from '../lib/decimal'
import { formatDuration } from '../formatDuration'
import { formatUtcDateTime } from './StatusBadge'
import { SummaryMetricCard } from './SummaryMetricCard'

/** What Home holds of the Reward Wallet Balance query (#246). It is its own
 *  request, so neither the Network projection's loading or failure nor any
 *  Home selection reaches it. */
export type RewardWalletReading =
  | { status: 'loading' }
  | { status: 'error' }
  | { status: 'ready'; wallet: PublicRewardWallet }

const UNAVAILABLE = '—'

/** `lat1zyctwgvw…j5h` reads `lat1zy…j5h` on the face; the whole address stays
 *  in the title and the accessible name. */
export function shortAddress(address: string): string {
  return address.length > 12 ? address.slice(0, 6) + '…' + address.slice(-3) : address
}

function upstreamProblem(wallet: PublicRewardWallet): string | null {
  if (wallet.errorCategory === 'upstream_unavailable') return 'PlatScan unavailable'
  if (wallet.errorCategory === 'invalid_response') return 'PlatScan returned an invalid response'
  return null
}

function sinceLastGood(lastGoodAt: string | null | undefined, now: Date): string {
  const at = lastGoodAt ? new Date(lastGoodAt).getTime() : Number.NaN
  return Number.isNaN(at) ? 'Unknown' : formatDuration(now.getTime() - at)
}

/**
 * The Balance tile: the Reward Wallet Balance as PlatScan reports it — the fixed
 * Reward Wallet's freely available LAT, not Validator Lifetime Rewards and
 * without restricted, staked, delegated or unclaimed amounts. It is one
 * Network-independent figure, so it reads the same under every Network
 * selection and list filter, including a list that matches no Node. A balance
 * the Server never read is `—` and `Unavailable`, never a zero; a retained
 * last-good balance keeps its value and says how old it is.
 */
export function RewardWalletCard({ reading, now = new Date() }: { reading: RewardWalletReading; now?: Date }) {
  const label = 'Balance'
  if (reading.status !== 'ready') {
    const loading = reading.status === 'loading'
    return <SummaryMetricCard label={label} icon={Wallet} value={UNAVAILABLE}
      caption={loading ? 'Loading…' : 'Unavailable'}
      hint={loading ? undefined : 'Unable to load the Reward Wallet Balance'}
      accessibleName={label + ': ' + (loading ? 'loading' : 'unavailable, unable to load the Reward Wallet Balance')} />
  }
  const { wallet } = reading
  const unit = wallet.unit
  const balance = isNonnegativeDecimal(wallet.balance) ? wallet.balance : null
  const where = 'Reward Wallet ' + wallet.address + ' on PlatON Mainnet'
  const problem = upstreamProblem(wallet)
  if (wallet.state === 'unavailable' || balance === null) {
    const reason = problem ?? 'No PlatScan reading yet'
    return <SummaryMetricCard label={label} icon={Wallet} value={UNAVAILABLE} caption="Unavailable"
      hint={reason + ' · ' + where}
      accessibleName={label + ': unavailable, ' + reason + '. ' + where} />
  }
  const exact = balance + ' ' + unit
  const lastGood = 'last updated ' + formatUtcDateTime(wallet.lastGoodAt)
  const stale = wallet.state === 'stale'
  const facts = stale
    ? [problem ?? 'PlatScan reading is stale', exact, where, lastGood]
    : [exact, where, lastGood]
  return <SummaryMetricCard label={label} icon={Wallet}
    value={formatAmountTwoDecimals(balance)}
    unit={unit}
    caption={stale ? 'stale · ' + sinceLastGood(wallet.lastGoodAt, now) : 'Mainnet · ' + shortAddress(wallet.address)}
    hint={facts.join(' · ')}
    accessibleName={label + ': ' + formatAmountTwoDecimals(balance) + ' ' + unit + (stale ? ', stale' : '') + '. ' + facts.join('. ')} />
}
