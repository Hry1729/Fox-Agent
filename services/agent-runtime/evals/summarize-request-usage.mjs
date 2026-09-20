// Usage records from Fox trace exports or NDJSON runtime events. Production
// pricing, cache normalization and completeness stay in D's shared module.
import { readFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { createRequestLedger, aggregateUsage } from '../src/usage-accounting.mjs'

export function summarizeRequestUsage(events) {
  const ledger = createRequestLedger()
  for (const event of events) {
    let body = event?.event ?? event?.payload ?? event
    if (typeof event?.eventJson === 'string') body = JSON.parse(event.eventJson)
    if (typeof event?.event_json === 'string') body = JSON.parse(event.event_json)
    if (body?.type === 'usage.request') ledger.record(body.record)
    else if (event?.type === 'kernel.usage_record') ledger.record(event.payload)
    else if (body?.schemaVersion === 'usage-v1' && body?.requestId) ledger.record(body)
  }
  return aggregateUsage(ledger.records())
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const text = await readFile(process.argv[2], 'utf8')
  let input
  try { input = JSON.parse(text) } catch { input = text.trim().split(/\r?\n/).map(line => JSON.parse(line)) }
  console.log(JSON.stringify(summarizeRequestUsage(Array.isArray(input) ? input : input.events ?? []), null, 2))
}
