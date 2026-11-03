/**
 * One repository-root resolver for the e2e harnesses (issue #219 review).
 *
 * Both the disposable-Server harness and the PlatScan replay need the workspace
 * root to find the built binary, the web bundle and the captured fixtures. Two
 * copies of that walk could disagree about where the repository is, so it lives
 * here once.
 */
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'

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

/**
 * The platpulse-server binary the e2e harnesses run. The global setup
 * (e2e/global-setup.ts) builds it once and publishes a private copy through
 * `PLATPULSE_E2E_SERVER_BIN`, so a parallel `cargo build` that rewrites the
 * shared target directory cannot swap the binary under a running suite.
 * Without it, the debug binary in `CARGO_TARGET_DIR` (default `target/`).
 */
export function serverBinary(repoRoot: string = findRepoRoot()): string {
  const prebuilt = process.env.PLATPULSE_E2E_SERVER_BIN
  if (prebuilt !== undefined && prebuilt !== '') return prebuilt
  const targetDir = process.env.CARGO_TARGET_DIR
  return join(
    targetDir === undefined || targetDir === '' ? join(repoRoot, 'target') : resolve(repoRoot, targetDir),
    'debug',
    'platpulse-server',
  )
}
