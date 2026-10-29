// Produces a session file whose assistant message has toolCalls but no toolResults:
// the process exits from a raw Agent listener right after the assistant message was persisted
// (AgentSession persists on message_end before later Agent listeners run: agent-session.js:755-770).
// Run: node services/agent-runtime/.probe-s0/p7-artifact.mjs
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { fauxAssistantMessage, fauxToolCall, Type } from '@earendil-works/pi-ai'
import { header, sleep } from './lib/harness.mjs'
import { createProbeSession } from './lib/session.mjs'

const root = '.probe-s0/tmp/p7'
rmSync(root, { recursive: true, force: true })
mkdirSync(root + '/sessions', { recursive: true })

const read = {
  name: 'read', label: 'read', description: 'read tool', parameters: Type.Object({ path: Type.String() }), executionMode: 'parallel',
  async execute(id) { return { content: [{ type: 'text', text: 'should never run ' + id }], details: {} } },
}

header('P7 artifact  assistant message with toolCalls, no toolResults')
const { session, sessionManager, faux } = await createProbeSession({
  cwd: root, sessionDir: root + '/sessions', customTools: [read],
})
const file = sessionManager.getSessionFile()

const IDS = ['call_orphan_1', 'call_orphan_2']
faux.script([
  fauxAssistantMessage([
    fauxToolCall('read', { path: 'one' }, { id: IDS[0] }),
    fauxToolCall('read', { path: 'two' }, { id: IDS[1] }),
  ], { stopReason: 'toolUse' }),
])

let exiting = false
session.agent.subscribe(async event => {
  if (exiting) return
  if (event.type === 'message_end' && event.message.role === 'assistant' && event.message.content.some(c => c.type === 'toolCall')) {
    exiting = true
    await sleep(20)   // let AgentSession's own listener finish appending
    writeFileSync(root + '/artifact-path.txt', file)
    console.log('  assistant message persisted; exiting before any toolResult is written')
    console.log('  session file: ' + file)
    process.exit(0)
  }
})

await session.prompt('go')
await sleep(2000)
console.log('  ERROR: the run completed without exiting early')
process.exit(1)
