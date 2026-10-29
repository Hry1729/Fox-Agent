// P4 abort during the prepare phase (inside the 2nd beforeToolCall).
// Run: node services/agent-runtime/.probe-s0/p4-prep-abort.mjs
import { readFileSync, existsSync, mkdirSync, rmSync } from 'node:fs'
import { SessionManager } from '@earendil-works/pi-coding-agent'
import { transformMessages } from '@earendil-works/pi-ai/api/transform-messages'
import { header, check, makeFaux, makeAgent, makeTimeline, makeHostTools, HostScheduler, batchMessage, textMessage, now, summarizeRequest } from './lib/harness.mjs'

const sessionDir = new URL('./tmp/p4-sessions/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\')
rmSync(sessionDir, { recursive: true, force: true })
mkdirSync(sessionDir, { recursive: true })

const timeline = makeTimeline()
const scheduler = new HostScheduler({ readLimit: 4, timeline })
const writeDir = new URL('./tmp/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\')
const { tools, record } = makeHostTools({ scheduler, timeline, readDelayMs: 30, writeDir })

// Real SessionManager, persisted exactly the way AgentSession does it:
// agent-session.js:755-770 persists every message_end message via appendMessage().
const sessionManager = SessionManager.create(writeDir, sessionDir)
const persisted = []
const persistOn = events => {
  const off = agent_subscribe(events)
  return off
}
let agent

const faux = makeFaux()
const IDS = ['call_p4_r1', 'call_p4_r2', 'call_p4_r3']
faux.script([
  batchMessage([
    { id: IDS[0], name: 'read', args: { path: 'one' } },
    { id: IDS[1], name: 'read', args: { path: 'two' } },
    { id: IDS[2], name: 'read', args: { path: 'three' } },
  ]),
  // second prompt of the session: after the aborted batch
  textMessage('auto-continuation of the aborted turn'),
  textMessage('second prompt answer'),
])

let abortDone = false
const beforeToolCall = async ({ toolCall }) => {
  if (toolCall.id === IDS[1] && !abortDone) {
    abortDone = true
    timeline.mark('probe', 'abort() called inside beforeToolCall of call #2', { id: toolCall.id })
    agent.abort()
  }
  return undefined
}

const built = makeAgent({ faux, tools, options: { beforeToolCall } })
agent = built.agent
const events = built.events
agent.subscribe(event => {
  if (event.type === 'message_end') {
    const entryId = sessionManager.appendMessage(event.message)
    persisted.push({ t: now(), role: event.message.role, id: entryId, toolCallId: event.message.toolCallId })
    timeline.mark('persist', `appendMessage ${event.message.role} -> entry ${entryId}`, { toolCallId: event.message.toolCallId })
  }
})

header('P4  abort inside the beforeToolCall of call #2 (of 3)')
await agent.prompt('go')
await agent.waitForIdle()

console.log('\n-- events')
for (const e of events.filter(e => ['message_start', 'message_end', 'tool_execution_start', 'tool_execution_end', 'turn_end', 'agent_end'].includes(e.type))) {
  console.log(`  t+${String(e.t).padStart(9)}ms  ${e.type.padEnd(20)} ${JSON.stringify({ role: e.role, toolCallId: e.toolCallId, stopReason: e.stopReason, toolCallIds: e.toolCallIds })}`)
}

console.log('\n-- execute() actually ran for: ' + JSON.stringify(record.map(r => r.toolCallId)))
const startedIds = events.filter(e => e.type === 'tool_execution_start').map(e => e.toolCallId)
const endedIds = events.filter(e => e.type === 'tool_execution_end').map(e => e.toolCallId)
const resultIds = events.filter(e => e.type === 'message_end' && e.role === 'toolResult').map(e => e.toolCallId)
console.log('  tool_execution_start: ' + JSON.stringify(startedIds))
console.log('  tool_execution_end:   ' + JSON.stringify(endedIds))
console.log('  toolResult messages:  ' + JSON.stringify(resultIds))

const file = sessionManager.getSessionFile()
const jsonl = existsSync(file) ? readFileSync(file, 'utf8').trim().split('\n').map(JSON.parse) : []
console.log(`\n-- session file: ${file}`)
console.log('-- JSONL entries:')
for (const e of jsonl) {
  if (e.type === 'session') { console.log(`   header version=${e.version} cwd=${e.cwd}`); continue }
  const m = e.message
  const tc = m?.content?.filter?.(c => c.type === 'toolCall').map(c => c.id) ?? []
  console.log(`   ${e.type.padEnd(8)} ${String(e.id).padEnd(10)} parent=${String(e.parentId).padEnd(10)} role=${(m?.role ?? '').padEnd(10)} stopReason=${m?.stopReason ?? ''} toolCalls=${JSON.stringify(tc)}${m?.toolCallId ? ' toolCallId=' + m.toolCallId : ''}`)
}

const orphanAssistant = jsonl.find(e => e.type === 'message' && e.message?.role === 'assistant' && e.message.content.some(c => c.type === 'toolCall'))
const orphanIds = orphanAssistant.message.content.filter(c => c.type === 'toolCall').map(c => c.id)
const answered = new Set(jsonl.filter(e => e.type === 'message' && e.message?.role === 'toolResult').map(e => e.message.toolCallId))
const unanswered = orphanIds.filter(id => !answered.has(id))

console.log('\n-- verdict (1) do the later calls get a toolResult?')
check('call #3 never started and has no toolResult', !startedIds.includes(IDS[2]) && !resultIds.includes(IDS[2]), `start=${startedIds.includes(IDS[2])} result=${resultIds.includes(IDS[2])}`)
check('call #2 (the aborting one) does get an immediate error result', resultIds.includes(IDS[1]), JSON.stringify(resultIds))
console.log('\n-- verdict (2) JSONL contains an assistant message with toolCalls lacking toolResults')
check('JSONL has an assistant message with orphaned toolCalls', unanswered.length === 3 || unanswered.length > 0, `orphans=${JSON.stringify(unanswered)} of ${JSON.stringify(orphanIds)}`)

// ---- (3) what does the fake model receive on the next prompt?
header('P4(3)  next prompt: what the provider layer sees')
const summary = summarizeRequest(faux.requests.at(-1).messages)
console.log('  request #' + faux.requests.length + ' message list:')
for (const s of summary) console.log('   ' + JSON.stringify(s))

const raw = faux.requests.at(-1).messages.filter(m => m.role !== 'system')
const transformed = transformMessages(structuredClone(raw), faux.model)
console.log('\n  after transformMessages() (what a real provider adapter would send):')
for (const m of transformed) {
  if (m.role === 'assistant') console.log('   ' + JSON.stringify({ role: 'assistant', stopReason: m.stopReason, toolCalls: m.content.filter(c => c.type === 'toolCall').map(c => c.id) }))
  else if (m.role === 'toolResult') console.log('   ' + JSON.stringify({ role: 'toolResult', toolCallId: m.toolCallId, isError: m.isError, text: m.content.map(c => c.text).join('').slice(0, 50) }))
  else console.log('   ' + JSON.stringify({ role: m.role, content: typeof m.content === 'string' ? m.content.slice(0, 40) : '[...]' }))
}

console.log('\n-- verdict (3)')
console.log(`  raw transcript still carries ALL ${orphanIds.length} toolCalls with ${unanswered.length} unanswered`)
check('transformMessages synthesizes a result for each orphaned toolCall', transformed.filter(m => m.role === 'toolResult' && m.isError && m.content[0]?.text === 'No result provided').length === unanswered.length, 'synthetic text = "No result provided"')

// ---- reopen the session and see what the SDK's own context builder yields
header('P4(4)  SessionManager.open(): SDK-built context for the same file')
const reopened = SessionManager.open(file, sessionDir)
const ctx = reopened.buildSessionContext()
for (const m of ctx.messages) {
  if (m.role === 'assistant') console.log('   ' + JSON.stringify({ role: 'assistant', stopReason: m.stopReason, toolCalls: m.content.filter(c => c.type === 'toolCall').map(c => c.id) }))
  else if (m.role === 'toolResult') console.log('   ' + JSON.stringify({ role: 'toolResult', toolCallId: m.toolCallId, isError: m.isError }))
  else console.log('   ' + JSON.stringify({ role: m.role, content: typeof m.content === 'string' ? m.content.slice(0, 40) : '[...]' }))
}
console.log(`\n  leaf=${reopened.getLeafId()} entries=${reopened.getEntryCount()}`)
console.log(`\n  (artifact kept for P7: ${file})`)

// ---- (3b) a genuine NEW prompt after the abort
header('P4(3b)  a new prompt AFTER the aborted run: what the provider layer sees')
const requestsBefore = faux.requests.length
await agent.prompt('go again')
await agent.waitForIdle()
const newMessages = faux.requests.at(-1).messages
console.log(`  (provider requests during this prompt: ${faux.requests.length - requestsBefore})`)
for (const s of summarizeRequest(newMessages)) console.log('   raw: ' + JSON.stringify(s))

const raw2 = newMessages.filter(m => m.role !== 'system')
const transformed2 = transformMessages(structuredClone(raw2), faux.model)
console.log('\n  after transformMessages() (what a real provider adapter would send):')
for (const m of transformed2) {
  if (m.role === 'assistant') console.log('   ' + JSON.stringify({ role: 'assistant', stopReason: m.stopReason, toolCalls: m.content.filter(c => c.type === 'toolCall').map(c => c.id) }))
  else if (m.role === 'toolResult') console.log('   ' + JSON.stringify({ role: 'toolResult', toolCallId: m.toolCallId, isError: m.isError, text: m.content.map(c => c.text).join('').slice(0, 40) }))
  else console.log('   ' + JSON.stringify({ role: m.role, content: typeof m.content === 'string' ? m.content.slice(0, 40) : '[blocks]' }))
}
const synthetic2 = transformed2.filter(m => m.role === 'toolResult' && m.content[0]?.text === 'No result provided')
const abortedDropped = !transformed2.some(m => m.role === 'assistant' && m.stopReason === 'aborted')
console.log('\n-- verdict (3b)')
check('the raw transcript keeps the orphaned toolCall', raw2.some(m => m.role === 'assistant' && m.content.some(c => c.type === 'toolCall' && unanswered.includes(c.id))), `orphan=${JSON.stringify(unanswered)}`)
check('the aborted assistant message is dropped by the provider transform', abortedDropped)
check('exactly one synthetic result is inserted for the orphan', synthetic2.length === 1, JSON.stringify(synthetic2.map(m => m.toolCallId)))

const jsonl2 = readFileSync(file, 'utf8').trim().split('\n').map(JSON.parse)
console.log(`\n-- JSONL now has ${jsonl2.length - 1} entries; last: ${jsonl2.at(-1).type} role=${jsonl2.at(-1).message?.role}`)

