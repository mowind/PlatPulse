import { Eclipse, Sun, Moon, Settings } from 'lucide-react'

// One bundled outline family for actions and business icons across both shells.
const icons = { 'dark-mode': Eclipse, 'sun-one': Sun, moon: Moon, setting: Settings }
export function EmeraldActionIcon({ name }: { name: keyof typeof icons }) {
  const Icon = icons[name]
  return <Icon className="shrink-0" size={18} strokeWidth={2} data-icon={name} aria-hidden="true" focusable="false" />
}
