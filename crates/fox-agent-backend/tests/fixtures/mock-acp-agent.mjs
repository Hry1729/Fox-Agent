// Minimal ACP v1 agent over stdio for Host-side client tests. One prompt:
// streams a chunk, asks permission, ends the turn according to the answer.
// Also echoes whether a secret-shaped parent variable leaked into its env.
// Prompt text "late": no permission at first; the permission request is sent
// only AFTER session/cancel arrives (a late approval racing a stop), and the
// way it was answered is reported as `late-permission:<outcome>`.
import readline from 'node:readline'
const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
let nextId = 1000
const waiting = new Map()
let promptId = null
let lateMode = false
rl.on('line', line => {
  const msg = JSON.parse(line)
  if (msg.method === 'initialize') {
    send({ id: msg.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: true, sessionCapabilities: { close: {} } }, authMethods: [] } })
  } else if (msg.method === 'session/new') {
    send({ id: msg.id, result: { sessionId: 'mock-session' } })
  } else if (msg.method === 'session/prompt') {
    promptId = msg.id
    const leaked = process.env.FOX_TEST_BACKEND_SECRET_TOKEN ? 'leaked' : 'clean'
    send({ method: 'session/update', params: { sessionId: 'mock-session', update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: `env:${leaked}` } } } })
    lateMode = msg.params?.prompt?.[0]?.text === 'late'
    if (lateMode) return
    const id = nextId++
    waiting.set(id, 'permission')
    send({ id, method: 'session/request_permission', params: { sessionId: 'mock-session', toolCall: { toolCallId: 't1', title: 'run npm test' }, options: [{ optionId: 'allow', name: 'Allow', kind: 'allow_once' }, { optionId: 'reject', name: 'Reject', kind: 'reject_once' }] } })
  } else if (msg.method === 'session/cancel') {
    send({ method: 'session/update', params: { sessionId: 'mock-session', update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'cancel-received' } } } })
    if (lateMode) {
      setTimeout(() => {
        const id = nextId++
        waiting.set(id, 'late')
        send({ id, method: 'session/request_permission', params: { sessionId: 'mock-session', toolCall: { toolCallId: 't-late', title: 'rm -rf build' }, options: [{ optionId: 'allow', name: 'Allow', kind: 'allow_once' }, { optionId: 'reject', name: 'Reject', kind: 'reject_once' }] } })
      }, 100)
    }
  } else if (msg.id !== undefined && waiting.get(msg.id) === 'late') {
    waiting.delete(msg.id)
    const outcome = msg.result?.outcome
    send({ method: 'session/update', params: { sessionId: 'mock-session', update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: `late-permission:${outcome?.optionId ?? outcome?.outcome}` } } } })
    send({ id: promptId, result: { stopReason: 'cancelled' } })
  } else if (msg.id !== undefined && waiting.has(msg.id)) {
    waiting.delete(msg.id)
    const outcome = msg.result?.outcome
    const stopReason = outcome?.outcome === 'cancelled' ? 'cancelled' : 'end_turn'
    send({ method: 'session/update', params: { sessionId: 'mock-session', update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: `permission:${outcome?.optionId ?? outcome?.outcome}` } } } })
    send({ id: promptId, result: { stopReason } })
  }
})
