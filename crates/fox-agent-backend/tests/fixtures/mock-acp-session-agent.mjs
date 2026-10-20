// Mock ACP v1 agent for the Host external runtime tests
// (tests/external_runtime.rs). One process, any number of sessions and
// turns; the prompt text picks the scenario:
//   simple      -> "pid:<pid> session:<id>" text, end_turn
//   hello       -> thought, text, tool_call (pending), plan, request_permission;
//                  allow -> tool completed + "permission:allow", end_turn;
//                  reject -> tool failed, end_turn; cancelled -> stop cancelled
//   slow        -> text, then waits; session/cancel -> stop cancelled
//   hang        -> text, then never answers (ignores session/cancel)
//   crash       -> text, then exits with code 3
//   maxtokens   -> text, stop max_tokens
// session/load replays one user and one agent chunk ("replayed-old") before
// answering, as the ACP spec describes.
import readline from 'node:readline'
const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
const update = (sessionId, body) => send({ method: 'session/update', params: { sessionId, update: body } })
const text = (sessionId, value) => update(sessionId, { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: value } })
let nextSession = 1
let nextId = 1000
const waiting = new Map()
const prompts = new Map()
// Answer a session's open prompt once (a cancel and a cancelled permission
// may both try).
const finish = (sessionId, stopReason) => {
  const promptId = prompts.get(sessionId)
  if (promptId === undefined || promptId === 'hang') return
  prompts.delete(sessionId)
  send({ id: promptId, result: { stopReason } })
}
rl.on('line', line => {
  const msg = JSON.parse(line)
  if (msg.method === 'initialize') {
    send({ id: msg.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: true }, authMethods: [], agentInfo: { name: 'mock', version: '0' } } })
  } else if (msg.method === 'session/new') {
    send({ id: msg.id, result: { sessionId: `sess-${process.pid}-${nextSession++}` } })
  } else if (msg.method === 'session/load') {
    const sessionId = msg.params.sessionId
    update(sessionId, { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'old question' } })
    text(sessionId, 'replayed-old')
    send({ id: msg.id, result: {} })
  } else if (msg.method === 'session/prompt') {
    const sessionId = msg.params.sessionId
    const prompt = msg.params.prompt?.[0]?.text ?? ''
    prompts.set(sessionId, msg.id)
    if (prompt === 'simple') {
      text(sessionId, `pid:${process.pid} session:${sessionId}`)
      finish(sessionId, 'end_turn')
    } else if (prompt === 'hello') {
      update(sessionId, { sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: 'thinking' } })
      text(sessionId, 'Running tests')
      update(sessionId, { sessionUpdate: 'tool_call', toolCallId: 't1', title: 'npm test', kind: 'execute', status: 'pending', rawInput: { command: 'npm test' } })
      update(sessionId, { sessionUpdate: 'plan', entries: [{ content: 'run tests', priority: 'high', status: 'in_progress' }] })
      const id = nextId++
      waiting.set(id, sessionId)
      send({ id, method: 'session/request_permission', params: { sessionId, toolCall: { toolCallId: 't1', title: 'npm test', kind: 'execute', rawInput: { command: 'npm test' } }, options: [{ optionId: 'allow', name: 'Allow', kind: 'allow_once' }, { optionId: 'always', name: 'Always', kind: 'allow_always' }, { optionId: 'reject', name: 'Reject', kind: 'reject_once' }] } })
    } else if (prompt === 'slow' || prompt === 'hang') {
      text(sessionId, `working:${prompt}`)
      if (prompt === 'hang') prompts.set(sessionId, 'hang')
    } else if (prompt === 'crash') {
      text(sessionId, 'about to crash')
      setTimeout(() => process.exit(3), 50)
    } else if (prompt === 'maxtokens') {
      text(sessionId, 'partial')
      finish(sessionId, 'max_tokens')
    } else {
      text(sessionId, `echo:${prompt}`)
      finish(sessionId, 'end_turn')
    }
  } else if (msg.method === 'session/cancel') {
    const sessionId = msg.params.sessionId
    // A pending permission was already answered `cancelled` by the client;
    // the turn ends cancelled either way.
    setTimeout(() => finish(sessionId, 'cancelled'), 20)
  } else if (msg.id !== undefined && waiting.has(msg.id)) {
    const sessionId = waiting.get(msg.id)
    waiting.delete(msg.id)
    const outcome = msg.result?.outcome
    if (outcome?.outcome === 'cancelled') {
      text(sessionId, 'permission:cancelled')
      finish(sessionId, 'cancelled')
      return
    }
    const allowed = outcome?.optionId === 'allow' || outcome?.optionId === 'always'
    update(sessionId, { sessionUpdate: 'tool_call_update', toolCallId: 't1', status: allowed ? 'completed' : 'failed', content: [{ type: 'content', content: { type: 'text', text: allowed ? 'tests passed' : 'rejected by user' } }] })
    text(sessionId, ` permission:${outcome?.optionId}`)
    finish(sessionId, 'end_turn')
  }
})
