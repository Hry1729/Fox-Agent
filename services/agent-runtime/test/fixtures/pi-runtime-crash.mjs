const point = process.env.FOX_TEST_CRASH_POINT
const write = process.stdout.write.bind(process.stdout)
let admissions = 0
process.stdout.write = (chunk, encoding, callback) => {
  let crash = false
  try {
    const message = JSON.parse(String(chunk))
    if (message.kind === 'request') {
      if (message.type === 'model_request.admit') admissions += 1
      crash = (point === 'register' && message.type === 'tool_batch.register')
        || (point === 'execute' && message.type === 'tool_call.execute')
        || (point === 'after_result' && message.type === 'model_request.admit' && admissions === 2)
    } else if (point === 'after_cancel' && message.kind === 'event' && message.type === 'run_released') crash = true
  } catch {}
  if (!crash) return write(chunk, encoding, callback)
  return write(chunk, typeof encoding === 'string' ? encoding : undefined, () => process.exit(73))
}
await import('../../src/pi-runtime.mjs')
