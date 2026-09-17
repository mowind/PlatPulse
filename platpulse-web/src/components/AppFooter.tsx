import { DataTooltip } from './ui/data-tooltip'

/**
 * Emerald's footer metrics (max-w-[1280px] mx-auto p-4, text-xs
 * text-muted-foreground) with PlatPulse's own copy.
 *
 * Upstream's footer credits the Komari product and the Emerald theme and links
 * to both; that brand and those links are deliberately not carried over. The
 * upstream licence notices live with the code they cover
 * (docs/adr/0002, docs/visual-migration/emerald/README.md) rather than in the
 * product chrome. PlatPulse has no client-version field, so no version is
 * invented here.
 */
export default function AppFooter() {
  return (
    <footer className="mx-auto w-full max-w-[1280px] p-4">
      <div className="flex flex-row items-center justify-between text-xs text-muted-foreground">
        <div className="flex items-center gap-1">
          <DataTooltip as="span" placement="top" content="PlatPulse — Server, Agent and WebUI monitoring for PlatON nodes.">
            <span className="font-medium text-foreground">PlatPulse</span>
          </DataTooltip>
        </div>
        <div className="flex flex-wrap items-center gap-1">
          <span className="opacity-50 text-xs text-muted-foreground">·</span>
          <span>Server-Agent-WebUI monitoring</span>
        </div>
      </div>
    </footer>
  )
}
