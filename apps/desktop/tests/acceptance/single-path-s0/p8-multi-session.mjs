// P8 two AgentSessions in one process: isolation and memory.
// Run: node services/agent-runtime/.probe-s0/p8-multi-session.mjs
import { spawnSync } from 'node:child_process'
import { mkdirSync, rmSync } from 'node:fs'
import { fauxAssistantMessage, fauxToolCall, Type } from '@earendil-works/pi-ai'
import { header, check, now, sleep } from './lib/harness.mjs'
import { createProbeSession } from './lib/session.mjs'

const rssMiB = () => +(process.memoryUsage().rss / 1048576).toFixed(1)
const root = '.probe-s0/tmp/p8'
rmSync(root, { recursive: true, force: true })
mkdirSync(root + '/a/sessions', { recursive: true })
mkdirSync(root + '/b/sessions', { recursive: true })

header('P8  two AgentSessions in one process')
const rss0 = rssMiB()
console.log('  RSS at script start (SDK already imported) : ' + rss0 + ' MiB')
const executions = []
const abortedAtEnd = []

const mkTool = ({ tag, name, readMs, honourSignal }) => ({
  name, label: name, description: name + ' tool', parameters: Type.Object({ path: Type.String() }), executionMode: 'parallel',
  async execute(id, params, signal) {
    executions.push({ tag, id, name, at: now(), abortedAtEntry: signal?.aborted === true })
    if (readMs) {
      await new Promise(resolve => {
        const t = setTimeout(resolve, readMs)
        signal?.addEventListener('abort', () => { if (honourSignal) { clearTimeout(t); resolve() } }, { once: true })
      })
    }
    abortedAtEnd.push({ tag, id, aborted: signal?.aborted === true })
    return { content: [{ type: 'text', text: tag + ' done ' + params.path }], details: {} }
  },
})

const A = await createProbeSession({
  cwd: root + '/a', sessionDir: root + '/a/sessions',
  customTools: [mkTool({ tag: 'a', name: 'a_read', readMs: 400, honourSignal: false })],
})
const rssA = rssMiB()
const B = await createProbeSession({
  cwd: root + '/b', sessionDir: root + '/b/sessions',
  customTools: [mkTool({ tag: 'b', name: 'b_read', readMs: 30, honourSignal: true })],
})
const rssB = rssMiB()
console.log('  RSS after session A                        : ' + rssA + ' MiB  (delta ' + (rssA - rss0).toFixed(1) + ')')
console.log('  RSS after session B                        : ' + rssB + ' MiB  (delta ' + (rssB - rssA).toFixed(1) + ')')
console.log('  tools of A: ' + JSON.stringify(A.session.agent.state.tools.map(t => ({ n: t.name, m: t.executionMode }))))
console.log('  tools of B: ' + JSON.stringify(B.session.agent.state.tools.map(t => ({ n: t.name, m: t.executionMode }))))

const eventsA = []
const eventsB = []
A.session.subscribe(e => { if (e.toolCallId) eventsA.push(e.toolCallId) })
B.session.subscribe(e => { if (e.toolCallId) eventsB.push(e.toolCallId) })

A.faux.script([
  fauxAssistantMessage([fauxToolCall('a_read', { path: 'A1' }, { id: 'a-call-1' })], { stopReason: 'toolUse' }),
  fauxAssistantMessage('A continued', { stopReason: 'stop' }),
])
B.faux.script([
  fauxAssistantMessage([fauxToolCall('b_read', { path: 'B1' }, { id: 'b-call-1' })], { stopReason: 'toolUse' }),
  fauxAssistantMessage('B continued', { stopReason: 'stop' }),
])

A.session.subscribe(e => {
  if (e.type === 'tool_execution_start' && e.toolCallId === 'a-call-1') {
    setTimeout(() => { console.log('  -> aborting session A at t+' + now() + 'ms'); A.session.abort() }, 80)
  }
})

const runA = A.session.prompt('A go')
const runB = B.session.prompt('B go')
await Promise.allSettled([runA, runB])
await sleep(150)

const summarize = s => s.agent.state.messages.filter(m => m.role !== 'system')
  .map(m => m.role + (m.toolCallId ? ':' + m.toolCallId + (m.isError ? '(err)' : '') : '') + (m.role === 'assistant' ? ':' + m.stopReason : ''))

console.log('\n  tool executions: ' + JSON.stringify(executions))
console.log('  signal state when each tool returned: ' + JSON.stringify(abortedAtEnd))
console.log('  A tool events: ' + JSON.stringify(eventsA))
console.log('  B tool events: ' + JSON.stringify(eventsB))
console.log('  A messages: ' + JSON.stringify(summarize(A.session)))
console.log('  B messages: ' + JSON.stringify(summarize(B.session)))
const aLast = A.session.agent.state.messages.at(-1)
console.log('  A terminal assistant: stopReason=' + aLast.stopReason + ' errorMessage=' + JSON.stringify(aLast.errorMessage))
console.log('  A provider requests: ' + A.faux.requests.length + ', B provider requests: ' + B.faux.requests.length)
console.log('  A session file: ' + A.sessionManager.getSessionFile())
console.log('  B session file: ' + B.sessionManager.getSessionFile())

console.log('\n-- verdict')
check('each session only ever saw its own toolCallIds',
  eventsA.every(id => id.startsWith('a-')) && eventsB.every(id => id.startsWith('b-')),
  'A=' + JSON.stringify(eventsA) + ' B=' + JSON.stringify(eventsB))
check('each session executed only its own tool',
  executions.filter(e => e.tag === 'a').every(e => e.id.startsWith('a-')) &&
  executions.filter(e => e.tag === 'b').every(e => e.id.startsWith('b-')),
  JSON.stringify(executions.map(e => e.tag + ':' + e.id)))
check('aborting A did not abort B: B produced its toolResult and a final assistant turn',
  B.session.agent.state.messages.some(m => m.role === 'toolResult' && m.toolCallId === 'b-call-1') &&
  B.session.agent.state.messages.some(m => m.role === 'assistant' && m.stopReason === 'stop'))
check('A saw the abort signal, B did not',
  abortedAtEnd.some(s => s.tag === 'a' && s.aborted) && abortedAtEnd.some(s => s.tag === 'b' && !s.aborted),
  JSON.stringify(abortedAtEnd))
check('different cwd and different session file per session',
  A.sessionManager.getCwd() !== B.sessionManager.getCwd() &&
  A.sessionManager.getSessionFile() !== B.sessionManager.getSessionFile())

header('P8b  memory')
console.log('  RSS after both runs                        : ' + rssMiB() + ' MiB')
const bare = spawnSync(process.execPath, ['-e', 'console.log((process.memoryUsage().rss/1048576).toFixed(1))'], { encoding: 'utf8' })
console.log('  RSS of a bare `node -e 0` process          : ' + bare.stdout.trim() + ' MiB')
const cold = spawnSync(process.execPath, ['.probe-s0/lib/cold-start.mjs'], { encoding: 'utf8' })
console.log('  separate process: import Pi + 1 session    : ' + (cold.stdout.trim() || cold.stderr.trim()))

