// Minimal in-process runner for harness-security tests.
// `node --test` spawns a subprocess per file (piped stdio), which the sandbox
// blocks with EPERM. The programmatic `run()` API executes in the current
// process instead, so the same test file is exercised without spawning.
import { run } from 'node:test'
import { spec } from 'node:test/reporters'
import { fileURLToPath } from 'node:url'

const file = fileURLToPath(new URL('./harness-security.test.mjs', import.meta.url))
const stream = run({ files: [file], concurrency: 1 })
stream.on('test:fail', () => { process.exitCode = 1 })
stream.compose(spec).pipe(process.stdout)
