import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
// harness-security -> fixtures -> test -> agent-runtime -> services -> worktree root
const root = join(here, '..', '..', '..', '..', '..')
const files = [
  'services/agent-runtime/test/harness-security.test.mjs',
  'services/agent-runtime/test/fixtures/harness-security/make-security-fixtures.mjs',
  'services/agent-runtime/test/fixtures/harness-security/run-in-process.mjs',
  'services/agent-runtime/test/fixtures/harness-security/probe-junction.mjs',
  'apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/harness_security_tests.rs',
]
let missing = 0
for (const file of files) {
  const exists = existsSync(join(root, file))
  if (!exists) missing += 1
  console.log(`${exists ? 'present' : 'MISSING'}  ${file}`)
}
console.log(`\nmissing=${missing}`)
if (missing > 0) process.exitCode = 1
