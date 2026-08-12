// Deterministic browser API client generation (design §13.4).
//
// 1. Reads the committed full OpenAPI 3 document produced by
//    `platpulse-server --print-openapi` (`docs/openapi/openapi.json`).
// 2. Drops `agent` operations and prunes every schema that becomes
//    unreachable, so Agent wire DTOs never reach the browser client.
// 3. Generates the TypeScript client into `src/api/generated/` with
//    @hey-api/openapi-ts (bundled native-fetch client, no runtime deps).
//
// CI reruns this script and fails on any diff, which also proves the
// committed spec and client stay in sync with the Server routes.

import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createClient } from '@hey-api/openapi-ts'

const WEB_ROOT = dirname(dirname(fileURLToPath(import.meta.url)))
const SPEC_PATH = join(WEB_ROOT, '..', 'docs', 'openapi', 'openapi.json')
const OUT_DIR = join(WEB_ROOT, 'src', 'api', 'generated')

/**
 * Build the browser spec: drop every operation tagged `agent` and every
 * schema that is no longer reachable from the remaining operations.
 * Returns a new object; the input spec is never mutated.
 */
export function filterBrowserSpec(spec) {
  const paths = {}
  for (const [path, item] of Object.entries(spec.paths ?? {})) {
    const pathItem = {}
    // Path-level parameters apply to every operation on the path; keep them
    // only if at least one operation survives.
    if (item.parameters) pathItem.parameters = item.parameters
    for (const [method, operation] of Object.entries(item)) {
      if (method === 'parameters') continue
      if (operation?.tags?.includes('agent')) continue
      pathItem[method] = operation
    }
    // Build the item from scratch: spreading the original item would keep
    // rejected agent operations that share the path with browser ones.
    if (Object.keys(pathItem).length > 0) paths[path] = pathItem
  }

  const reachable = new Set()
  const visit = (node) => {
    if (!node || typeof node !== 'object') return
    if (typeof node.$ref === 'string') {
      const name = node.$ref.replace('#/components/schemas/', '')
      if (name !== node.$ref && !reachable.has(name)) {
        reachable.add(name)
        visit(spec.components?.schemas?.[name])
      }
      return
    }
    for (const value of Object.values(node)) visit(value)
  }
  for (const pathItem of Object.values(paths)) {
    for (const operation of Object.values(pathItem)) {
      if (!operation || typeof operation !== 'object') continue
      visit(operation.parameters)
      visit(operation.requestBody)
      visit(operation.responses)
    }
  }

  const schemas = {}
  for (const [name, schema] of Object.entries(spec.components?.schemas ?? {})) {
    if (reachable.has(name)) schemas[name] = schema
  }

  return { ...spec, paths, components: { ...spec.components, schemas } }
}

async function main() {
  const spec = JSON.parse(await readFile(SPEC_PATH, 'utf8'))
  const browserSpec = filterBrowserSpec(spec)

  const tempDir = await mkdtemp(join(tmpdir(), 'platpulse-openapi-'))
  const filteredPath = join(tempDir, 'openapi-browser.json')
  await writeFile(filteredPath, JSON.stringify(browserSpec))
  try {
    await createClient({
      input: filteredPath,
      output: OUT_DIR,
      plugins: ['@hey-api/client-fetch'],
      outputOptions: { clean: true },
    })
  } finally {
    await rm(tempDir, { recursive: true, force: true })
  }
  console.log(`generated browser client in ${OUT_DIR} from ${SPEC_PATH}`)
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error)
    process.exit(1)
  })
}
