// P2b negative control: beforeToolCall that waits for the batch to SETTLE.
// Run: node services/agent-runtime/.probe-s0/p2b-settle-deadlock.mjs   (expected: prints HUNG, exit 0)
import { header, makeFaux, makeAgent, makeHostTools, HostScheduler, batchMessage, textMessage, sleep } from './lib/harness.mjs'

header('P2b  negative control: beforeToolCall waits for batch settlement')
const writeDir = new URL('./tmp/', import.meta.url).pathname.replace(/^\//, '').replace(/\//g, '\\')
const scheduler = new HostScheduler({ readLimit: 4 })
const { tools, record } = makeHostTools({ scheduler, readDelayMs: 20, writeDir })

const settled = new Set()
const waiters = []
const settle = id => { settled.add(id); waiters.splice(0).forEach(fn => fn()) }
const waitAllSettled = ids => ids.every(id => settled.has(id))
  ? Promise.resolve()
  : new Promise(resolve => { const check = () => (ids.every(id => settled.has(id)) ? resolve() : waiters.push(check)); waiters.push(check) })

const faux = makeFaux()
faux.script([
  batchMessage([
    { id: 'call_n1', name: 'read', args: { path: 'n1' } },
    { id: 'call_n2', name: 'read', args: { path: 'n2' } },
  ]),
  textMessage('done'),
])

let batchIds = []
const { agent } = makeAgent({
  faux, tools,
  options: {
    beforeToolCall: async ({ assistantMessage }) => {
      batchIds = assistantMessage.content.filter(c => c.type === 'toolCall').map(c => c.id)
      await waitAllSettled(batchIds)   // the dangerous variant
      return undefined
    },
  },
})
agent.subscribe(e => { if (e.type === 'tool_execution_end') settle(e.toolCallId) })

const outcome = await Promise.race([agent.prompt('go').then(() => 'completed'), sleep(1500).then(() => 'hung')])
console.log(`  outcome after 1500ms: ${outcome}`)
console.log(`  tool execute() calls that ever ran: ${record.length} of ${batchIds.length} registered toolCalls`)
agent.abort()
await sleep(300)
console.log(`  after agent.abort() + 300ms: tool execute() calls that ran = ${record.length}`)
console.log(`  agent.signal.aborted = ${agent.signal?.aborted}`)
console.log('\n-- verdict')
console.log(`${outcome === 'hung' ? 'PASS' : 'FAIL'}  awaiting batch settlement inside beforeToolCall deadlocks the run, and abort() does not break it`)
console.log('  (the prepare phase cannot advance, so no tool can ever reach execute() and settle the wait)')
// The run can never settle; exit explicitly rather than leave an unsettled top-level await.
process.exit(0)
