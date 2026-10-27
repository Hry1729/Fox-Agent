// Test-only process fault injector, imported into the runtime child through
// `NODE_OPTIONS=--import=...` by `test/runtime-fatal.test.mjs`.
//
// It is inert unless `FOX_TEST_FATAL_KIND` names a fault. The fault is armed
// when the first `prompt` frame arrives on stdin, so it always fires *inside* a
// live Run — the exact situation that used to kill the process with a closed
// stdout and no terminal event. Nothing here is used by the production runtime.
const kind = process.env.FOX_TEST_FATAL_KIND
const delayMs = Number(process.env.FOX_TEST_FATAL_DELAY_MS ?? 60)

if (kind === 'rejection' || kind === 'exception') {
  let seen = ''
  let armed = false
  process.stdin.on('data', (chunk) => {
    if (armed) return
    seen += chunk.toString('utf8')
    if (!seen.includes('"type":"prompt"')) {
      if (seen.length > 65536) seen = seen.slice(-4096)
      return
    }
    armed = true
    setTimeout(() => {
      if (kind === 'rejection') {
        // A detached rejection: nothing in the Run's own promise chain awaits it.
        Promise.reject(new Error('injected fatal fault: detached promise rejection'))
        return
      }
      throw new Error('injected fatal fault: uncaught exception in a timer')
    }, delayMs)
  })
}
