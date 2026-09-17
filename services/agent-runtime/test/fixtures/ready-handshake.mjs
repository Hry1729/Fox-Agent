// Used by the Rust Host test: capture the actual initialize response, rather
// than constructing a manifest that accidentally omits newly registered tools.
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../../src/protocol.mjs'

const executable = process.env.FOX_RUNTIME_BINARY || process.execPath
const args = process.env.FOX_RUNTIME_BINARY ? [] : [fileURLToPath(new URL('../../src/pi-runtime.mjs', import.meta.url))]
const child = spawn(executable, args, {
  stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true,
})
const lines = createInterface({ input: child.stdout })
const timer = setTimeout(() => finish(1, 'runtime initialize timed out'), 10_000)
let finished = false
function finish(code, error) {
  if (finished) return
  finished = true
  clearTimeout(timer)
  lines.close()
  child.kill()
  if (error) process.stderr.write(`${error}\n`)
  process.exitCode = code
}
child.on('error', error => finish(1, error.message))
child.on('exit', code => { if (!finished) finish(1, `runtime exited before ready (${code})`) })
lines.on('line', line => {
  try {
    const message = JSON.parse(line)
    if (message.type !== 'ready') return finish(1, `unexpected initialize response: ${message.type}`)
    process.stdout.write(`${JSON.stringify(message.payload)}\n`)
    finish(0)
  } catch (error) { finish(1, error.message) }
})
child.stdin.write(`${JSON.stringify(createEnvelope('request', 'initialize', {
  payload: { modelService: { baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 4096, maxOutputTokens: 512 } },
}))}\n`)
