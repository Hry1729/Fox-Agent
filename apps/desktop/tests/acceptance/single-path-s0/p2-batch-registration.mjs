// P2 whole-batch registration inside the first beforeToolCall.
// Run: node services/agent-runtime/.probe-s0/p2-batch-registration.mjs
import { header, check, makeFaux, makeAgent, makeTimeline, makeHostTools, HostScheduler, batchMessage, textMessage, now, sleep } from './lib/harness.mjs'

const timeline = makeTimeline()
const scheduler = new HostScheduler({ readLimit: 4, timeline })
const writeDir = new URL('./tmp/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\')
const { tools, record } = makeHostTools({ scheduler, timeline, readDelayMs: 30, writeDir })

const ledger = { registrations: [], beforeCallEntries: [], byMessage: new Map() }
const REGISTER_ACK_MS = 50

async function registerBatch(assistantMessage) {
  const existing = ledger.byMessage.get(assistantMessage)
  if (existing) return existing
  const toolCalls = assistantMessage.content.filter(c => c.type === 'toolCall')
  const registration = (async () => {
    const startedAt = now()
    await sleep(REGISTER_ACK_MS)             // simulated durable acknowledgement
    const done = { t: now(), ids: toolCalls.map(c => c.id), startedAt, ackMs: REGISTER_ACK_MS }
    ledger.registrations.push(done)
    timeline.mark('ledger', `batch registered (${toolCalls.length} calls)`, { ids: done.ids })
    return done
  })()
  ledger.byMessage.set(assistantMessage, registration)
  return registration
}

let hookArgs = null
const beforeToolCall = async ({ assistantMessage, toolCall, args, context }) => {
  hookArgs = { keys: Object.keys({ assistantMessage, toolCall, args, context }), hasAssistantMessage: !!assistantMessage, hasContext: !!context, argsType: typeof args }
  const alreadyKnown = ledger.byMessage.has(assistantMessage)
  ledger.beforeCallEntries.push({ t: now(), toolCallId: toolCall.id, alreadyKnown })
  // Await the batch registration only. Never await tool execution.
  await registerBatch(assistantMessage)
  return undefined
}

const faux = makeFaux()
faux.script([
  batchMessage([
    { id: 'call_b1_r1', name: 'read', args: { path: '1' } },
    { id: 'call_b1_r2', name: 'read', args: { path: '2' } },
    { id: 'call_b1_r3', name: 'read', args: { path: '3' } },
  ]),
  batchMessage([
    { id: 'call_b2_r1', name: 'read', args: { path: '4' } },
    { id: 'call_b2_r2', name: 'read', args: { path: '5' } },
  ]),
  textMessage('done'),
])

const { agent } = makeAgent({ faux, tools, options: { beforeToolCall } })

header('P2  whole-batch registration in the first beforeToolCall')
const started = now()
await agent.prompt('go')
await agent.waitForIdle()
const elapsed = now() - started
timeline.dump()

console.log('\n-- beforeToolCall invocations (arrival order = source order, serial prepare phase)')
for (const e of ledger.beforeCallEntries) console.log(`  t+${String(e.t).padStart(9)}ms  ${e.toolCallId.padEnd(12)} batch_already_known=${e.alreadyKnown}`)

console.log('\n-- registrations')
for (const r of ledger.registrations) console.log(`  t+${String(r.t).padStart(9)}ms  ids=${JSON.stringify(r.ids)}`)

const batch1FirstToolStart = Math.min(...record.filter(r => r.toolCallId.startsWith('call_b1')).map(r => r.startAt))
const batch2FirstToolStart = Math.min(...record.filter(r => r.toolCallId.startsWith('call_b2')).map(r => r.startAt))

// per-batch grouping: [0..2] belongs to batch 1, [3..4] to batch 2
const b1 = ledger.beforeCallEntries.slice(0, 3)
const b2 = ledger.beforeCallEntries.slice(3, 5)

console.log('\n-- verdict')
console.log('  beforeToolCall argument keys: ' + JSON.stringify(hookArgs))
check('beforeToolCall receives exactly { assistantMessage, toolCall, args, context }', hookArgs && JSON.stringify(hookArgs.keys) === JSON.stringify(['assistantMessage', 'toolCall', 'args', 'context']))
check('no deadlock: both batches and the final turn completed',
  ledger.registrations.length === 2 && record.length === 5,
  `registrations=${ledger.registrations.length} toolExecutions=${record.length} elapsed=${elapsed.toFixed(1)}ms`)
check('one registration per assistant batch (3+2 calls -> 2 registrations)',
  ledger.registrations.length === 2 && ledger.registrations[0].ids.length === 3 && ledger.registrations[1].ids.length === 2,
  JSON.stringify(ledger.registrations.map(r => r.ids.length)))
check('beforeToolCall ran for every toolCall (3+2)',
  ledger.beforeCallEntries.length === 5, `entries=${ledger.beforeCallEntries.length}`)
check('re-entry within a batch never registers a second time',
  b1[0].alreadyKnown === false && b1.slice(1).every(e => e.alreadyKnown) &&
  b2[0].alreadyKnown === false && b2.slice(1).every(e => e.alreadyKnown),
  `batch1=${JSON.stringify(b1.map(e => e.alreadyKnown))} batch2=${JSON.stringify(b2.map(e => e.alreadyKnown))}`)
check('no tool execute starts before its batch registration is acknowledged',
  batch1FirstToolStart >= ledger.registrations[0].t && batch2FirstToolStart >= ledger.registrations[1].t,
  `batch1 firstStart=${batch1FirstToolStart} ack=${ledger.registrations[0].t}`)
console.log(`\nregistration latency sits on the batch critical path: first tool ran ${(batch1FirstToolStart - ledger.registrations[0].startedAt).toFixed(1)}ms after registration began (ack=${REGISTER_ACK_MS}ms)`)

