import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

/**
 * Class composition, the same helper upstream Emerald uses
 * (src/lib/utils.ts at c2c5e88). Variant classes from cva() and caller
 * overrides are merged so the last conflicting utility wins.
 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
