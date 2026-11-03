/**
 * Where opt-in evidence captures land.
 *
 * Evidence (screenshots, measured geometry, frozen DTOs for a before/after pair)
 * belongs in the pull request description or a GitHub Actions artifact, never in
 * git: `docs/visual-migration/` keeps its existing images frozen and
 * scripts/check-no-new-screenshots.py rejects new ones. Captures therefore go to
 * the ignored `platpulse-web/evidence/` directory. It is not Playwright's
 * outputDir because Playwright empties that at the start of every run, which
 * would delete the "before" captures when the "after" phase runs.
 */
import { join } from 'node:path'

import { findRepoRoot } from './repo-root'

/** Absolute path under the ignored evidence directory. */
export function evidenceDir(...segments: string[]): string {
  return join(findRepoRoot(), 'platpulse-web', 'evidence', ...segments)
}
