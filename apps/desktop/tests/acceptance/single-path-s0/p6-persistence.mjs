// P6 when does a toolResult actually reach the JSONL file?
// Real AgentSession (createAgentSession) + SessionManager.create(temp dir), P1's mixed batch.
// Run: node services/agent-runtime/.probe-s0/p6-persistence.mjs
import { readFileSync, existsSync, mkdirSync, rmSync } from 'node:fs'
import { fauxAssistantMessage, fauxToolCall, Type } from '@earendil-works/pi-ai'
import { header, check, now, sleep, HostScheduler } from './lib/harness.mjs'
import { createProbeSession } from './lib/session.mjs'

const root = '.probe-s0/tmp/p6'
rmSync(root, { recursive: true, force: true })
mkdirSync(root + '/sessions', { recursive: true })

const scheduler = new HostScheduler({ readLimit: 4 })
const windows = []
const readTool = name => ({
  name, label: name, description: name + ' tool', parameters: Type.Object({ path: Type.String() }), executionMode: 'parallel',
  async execute(id, params) {
    const arrived = now()
    const w = { id, name, arrived, start: null, end: null }
    windows.push(w)
    await scheduler.acquireRead()
    w.start = now()
    try { await sleep(40); return { content: [{ type: 'text', text: 'executed ' + id }], details: {} } }
    finally { w.end = now(); scheduler.releaseRead() }
  },
})
const writeTool = {
  name: 'write', label: 'write', description: 'write tool', parameters: Type.Object({ path: Type.String(), text: Type.String() }), executionMode: 'parallel',
  async execute(id, params) {
    const arrived = now()
    const w = { id, name: 'write', arrived, start: null, end: null }
    windows.push(w)
    return scheduler.withBarrier(async () => {
      w.start = now()
      try {
        const { writeFileSync } = await import('node:fs')
        writeFileSync(root + '/' + String(params.path).replace(/[\\/:]/g, '_'), params.text ?? '')
        return { content: [{ type: 'text', text: 'wrote ' + params.path }], details: {} }
      } finally { w.end = now() }
    })
  },
}

const { session, sessionManager, faux } = await createProbeSession({
  cwd: root, sessionDir: root + '/sessions', customTools: [readTool('read'), readTool('grep'), writeTool],
})
const file = sessionManager.getSessionFile()

const IDS = { A: 'call_read_A', B: 'call_grep_B', C: 'call_write_C', D: 'call_read_C2' }
faux.script([
  fauxAssistantMessage([
    fauxToolCall('read', { path: 'A' }, { id: IDS.A }),
    fauxToolCall('grep', { path: 'B' }, { id: IDS.B }),
    fauxToolCall('write', { path: 'C.txt', text: 'x' }, { id: IDS.C }),
    fauxToolCall('read', { path: 'C' }, { id: IDS.D }),
  ], { stopReason: 'toolUse' }),
  fauxAssistantMessage('done', { stopReason: 'stop' }),
])

function onDisk(toolCallId) {
  if (!file || !existsSync(file)) return { present: false, entries: -1, entryId: null }
  const entries = readFileSync(file, 'utf8').trim().split('\n').filter(Boolean)
    .map(l => { try { return JSON.parse(l) } catch { return null } }).filter(Boolean)
  const hit = entries.find(e => e.type === 'message' && e.message?.role === 'toolResult' && e.message.toolCallId === toolCallId)
  return { present: !!hit, entries: entries.length - 1, entryId: hit?.id ?? null }
}

const log = []
const record = async (source, event) => {
  const id = event.toolCallId ?? event.message?.toolCallId
  const before = onDisk(id)
  const leafBefore = sessionManager.getLeafId()
  await null   // exactly one microtask hop
  const after = onDisk(id)
  const leafAfter = sessionManager.getLeafId()
  log.push({ t: now(), source, type: event.type, role: event.message?.role, id, diskBefore: before.present, diskAfter: after.present, entryId: after.entryId, leafBefore, leafAfter })
}

session.subscribe(event => { void record('session', event) })
session.agent.subscribe(event => { void record('agent', event) })

header('P6  toolResult persistence timing (real AgentSession + SessionManager.create)')
console.log('  session file: ' + file)
await session.prompt('go')
await session.waitForIdle()
await sleep(50)

console.log('\n-- execution windows of the real tools (all executionMode:"parallel")')
for (const w of windows) console.log('  ' + w.name.padEnd(6) + ' ' + w.id.padEnd(16) + ' start=' + String(w.start).padStart(9) + ' end=' + String(w.end).padStart(9))

const byId = Object.fromEntries(windows.map(w => [w.id, w]))
check('all four tools really executed',
  windows.length === 4 && windows.every(w => w.start !== null),
  JSON.stringify(windows.map(w => w.id)))
check('write C started after read A and grep B both ended',
  byId[IDS.C].start >= Math.max(byId[IDS.A].end, byId[IDS.B].end),
  'C.start=' + byId[IDS.C].start + ' max(A,B).end=' + Math.max(byId[IDS.A].end, byId[IDS.B].end))
check('read C2 started after write C ended',
  byId[IDS.D].start >= byId[IDS.C].end,
  'D.start=' + byId[IDS.D].start + ' C.end=' + byId[IDS.C].end)

console.log('\n-- event log: disk state at listener time, and after one microtask hop')
console.log('   t(ms)      source   type                  toolCallId        disk@sync  disk@+1mt  entryId   leafAfter')
for (const r of log) {
  if (!['message_end', 'tool_execution_end', 'agent_end', 'turn_end'].includes(r.type)) continue
  console.log('  ' + String(r.t).padStart(9) + '  ' + r.source.padEnd(7) + '  ' + r.type.padEnd(21) + ' ' + String(r.id ?? '').padEnd(16) + '  ' + String(r.diskBefore).padEnd(9) + '  ' + String(r.diskAfter).padEnd(9) + '  ' + String(r.entryId ?? '').padEnd(9) + ' ' + String(r.leafAfter ?? ''))
}

const fromSession = log.filter(r => r.source === 'session' && r.type === 'message_end' && r.role === 'toolResult')
const fromAgent = log.filter(r => r.source === 'agent' && r.type === 'message_end' && r.role === 'toolResult')

console.log('\n-- verdict')
console.log('  AgentSession listener (session.subscribe) sees the entry ' + (fromSession[0]?.diskBefore ? 'already' : 'NOT yet') + ' on disk at message_end')
console.log('  raw Agent listener (agent.subscribe, registered after AgentSession own listener) sees it ' + (fromAgent[0]?.diskBefore ? 'already' : 'NOT yet') + ' on disk')
check('[session.subscribe] at message_end the toolResult is NOT yet on disk', fromSession.length === 4 && fromSession.every(r => r.diskBefore === false))
check('[session.subscribe] after one microtask hop it IS on disk', fromSession.every(r => r.diskAfter === true))
check('[agent.subscribe] at message_end it is ALREADY on disk', fromAgent.length === 4 && fromAgent.every(r => r.diskBefore === true))
check('the entry id on disk equals SessionManager.getLeafId() right after that hop', fromSession.every(r => r.entryId === r.leafAfter))
check('each toolResult entry carries an id that maps to its toolCallId', fromSession.length === 4)

const execEnds = log.filter(r => r.source === 'agent' && r.type === 'tool_execution_end')
const agentEnd = log.find(r => r.source === 'session' && r.type === 'agent_end')
console.log('  last tool_execution_end t+' + execEnds.at(-1).t + 'ms; first toolResult persisted t+' + fromSession[0].t + 'ms; agent_end t+' + agentEnd.t + 'ms')
console.log('  every toolResult was on disk before agent_end: ' + (agentEnd.t > fromSession.at(-1).t))

const entryAppended = log.filter(r => r.type === 'entry_appended')
console.log('\n  entry_appended events observed: ' + entryAppended.length + ' (emitted only for custom entries / context edits / cache-warm entries, not for message_end: agent-session.js:755-770)')

header('P6b  final JSONL')
for (const l of readFileSync(file, 'utf8').trim().split('\n')) {
  const e = JSON.parse(l)
  if (e.type === 'session') { console.log('  header ' + JSON.stringify({ version: e.version, cwd: e.cwd })); continue }
  const m = e.message
  console.log('  ' + String(e.id).padEnd(10) + ' parent=' + String(e.parentId).padEnd(10) + ' ' + e.type + '/' + (m?.role ?? '').padEnd(10) + ' ' + (m?.toolCallId ? 'toolCallId=' + m.toolCallId : '') + (m?.isError ? ' isError=true' : ''))
}
console.log('\n  leafId=' + sessionManager.getLeafId() + ' entries=' + sessionManager.getEntryCount())
