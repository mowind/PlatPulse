/**
 * One repository-root resolver for the e2e harnesses (issue #219 review).
 *
 * Both the disposable-Server harness and the PlatScan replay need the workspace
 * root to find the built binary, the web bundle and the captured fixtures. Two
 * copies of that walk could disagree about where the repository is, so it lives
 * here once.
 */
import { existsSync } from 'node:fs'
import { dirname, join } from 'node:path'

/** Walk up from the invocation directory so a harness does not depend on the
 *  Playwright cwd happening to be `platpulse-web`. */
export function findRepoRoot(from: string = process.cwd()): string {
  let directory = from
  for (;;) {
    if (existsSync(join(directory, 'Cargo.toml')) && existsSync(join(directory, 'platpulse-web'))) {
      return directory
    }
    const parent = dirname(directory)
    if (parent === directory) break
    directory = parent
  }
  throw new Error(`could not locate the PlatPulse repository root from ${from}`)
}
