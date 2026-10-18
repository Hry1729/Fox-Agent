// Runs the emitted delivery-acceptance component test under node, with the
// real happy-dom global registration and the real testing-library.
//
// The snapshot has no vitest, so this is the runner: it imports the emitted
// test module, which executes its assertions on import and prints a sentinel on
// success. A non-zero exit and no sentinel means the assertions failed.
//
// Two adjustments are made to the emitted file, both of them properties of the
// emit rather than differences in what the test tests:
//
// 1. `tsc` with `moduleResolution: "Bundler"` strips the `.tsx` suffix from the
//    emitted import, which plain node ESM cannot resolve.
// 2. `require` is injected, because the desktop tsconfig has no `@types/node`
//    and a `node:` specifier fails its type-check.

import { readFile, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'

const target = process.argv[2]
if (!target) {
  console.error('usage: node run-component-test.mjs <emitted-test-js>')
  process.exit(2)
}

process.on('unhandledRejection', (reason) => {
  console.error('UNHANDLED_REJECTION', reason)
  process.exit(1)
})

// `@testing-library/react`'s `screen` binds `document.body` at import time, so
// the happy-dom globals have to exist before the test module is imported.
const { GlobalRegistrator } = await import('@happy-dom/global-registrator')
GlobalRegistrator.register()

const fixed = target.replace(/\.js$/, '.run.mjs')
const source = await readFile(target, 'utf8')
const patched = source
  // Idempotent on purpose. The non-greedy capture plus the optional trailing
  // `.js` means an import that already carries the suffix is left exactly as it
  // is, instead of becoming `.js.js` — which is what made the previous version
  // of this runner unreproducible for anyone running the recorded command.
  .replace(/from '\.\/([^']+?)(?:\.js)?'/g, "from './$1.js'")
  // The test reaches node's `assert` through this injected require, because the
  // desktop tsconfig has no `@types/node` and a `node:` specifier fails the
  // contract check.
  .replace(
    /const nodeRequire = globalThis\.__nodeRequire/g,
    'const nodeRequire = globalThis.__nodeRequire',
  )
globalThis.__nodeRequire = createRequire(import.meta.url)
await writeFile(fixed, patched)

try {
  await import(new URL(`file://${fixed.replace(/\\/g, '/')}`))
} catch (error) {
  console.error('TEST_MODULE_FAILED')
  console.error(error)
  process.exit(1)
}
