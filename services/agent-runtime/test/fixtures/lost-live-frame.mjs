// Fault-injection transport fixture. No model and no resource executor.
import { createInterface } from 'node:readline'
import { writeFileSync } from 'node:fs'
const mode = '__MODE__'
createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line)
  if (request.type === 'kernel.initialize') {
    process.stdout.write(JSON.stringify({ ...request, kind: 'response', type: 'kernel.ready', requestId: request.id,
      payload: { singleUse: true, resourceExecution: false, automaticReplay: false, roundLoop: true, adapterVersion: '__ADAPTER__' } }) + '\n')
  } else if (request.type === 'kernel.start_initial') {
    writeFileSync(new URL('./worker-started', import.meta.url), String(process.pid))
    if (mode === 'exit') process.exit(0)
    setInterval(() => {}, 1000)
  }
})
