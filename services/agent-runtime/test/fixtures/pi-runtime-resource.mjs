const write = process.stdout.write.bind(process.stdout)
const runs = new Set()
process.stdout.write = (chunk, encoding, callback) => {
  const result = write(chunk, encoding, callback)
  try {
    const message = JSON.parse(String(chunk))
    if (message.kind === 'request' && message.type === 'tool_call.execute') {
      runs.add(message.runId)
      process.stderr.write(`FOX_RESOURCE_SAMPLE:${JSON.stringify({ activeRuns: runs.size, ...process.memoryUsage() })}\n`)
    }
  } catch {}
  return result
}
await import('../../src/pi-runtime.mjs')
