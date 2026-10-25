// Mock ACP v1 agent for the Host external runtime tests
// (tests/external_runtime.rs). One process, any number of sessions and
// turns; the prompt text picks the scenario:
//   simple      -> "pid:<pid> session:<id>" text, end_turn
//   hello       -> thought, text, tool_call (pending), plan, request_permission;
//                  allow -> tool completed + "permission:allow", end_turn;
//                  reject -> tool failed, end_turn; cancelled -> stop cancelled
//   slow        -> text, then waits; session/cancel -> stop cancelled
//   reconnect   -> Codex warning + "Reconnecting..." notices, then waits
//   (session/set_mode is accepted unless modeId is "refuse")
//   hang        -> text, then never answers (ignores session/cancel)
//   crash       -> text, then exits with code 3
//   maxtokens   -> text, stop max_tokens
//   stderr-fail -> writes secret-bearing lines (plus a ~2000-byte line and
//                  a CJK line) to stderr, then answers the
//                  prompt with a JSON-RPC error (process stays alive)
//   stderr-crash -> writes secret-bearing lines to stderr, then exits 4
//   data-fail   -> answers the prompt with a JSON-RPC error whose `data`
//                  carries keys and a Bearer token; writes NOTHING to stderr
//   data-stderr-fail -> one stderr line, then the same error with `data`
// session/load replays one user and one agent chunk ("replayed-old") before
// answering, as the ACP spec describes. A session id containing "longreplay"
// replays a long history (text, thoughts, tool calls) and sends more old
// output right after the reply; the prompt "after-load" answers with
// "fresh-after-load" and one tool call ("new-tool").
import readline from 'node:readline'
const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
const update = (sessionId, body) => send({ method: 'session/update', params: { sessionId, update: body } })
const text = (sessionId, value) => update(sessionId, { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: value } })
let nextSession = 1
let nextId = 1000
const waiting = new Map()
const prompts = new Map()
const sessions = new Map()
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
    const sessionId = `sess-${process.pid}-${nextSession++}`
    sessions.set(sessionId, { model: 'stored-old-model' })
    send({ id: msg.id, result: { sessionId } })
  } else if (msg.method === 'session/load') {
    const sessionId = msg.params.sessionId
    sessions.set(sessionId, { model: 'stored-old-model' })
    if (sessionId.includes('longreplay')) {
      // A long history: user / agent / thought chunks and finished tool
      // calls, written in one burst right before the reply, then (like
      // pi-acp's available_commands_update) more output after the reply but
      // before any prompt.
      for (let i = 0; i < 40; i++) {
        update(sessionId, { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: `old question ${i}` } })
        update(sessionId, { sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: `old thought ${i}` } })
        update(sessionId, { sessionUpdate: 'tool_call', toolCallId: `old-tool-${i}`, title: `old tool ${i}`, kind: 'read', status: 'completed', rawInput: { path: `old-${i}.txt` } })
        text(sessionId, `replayed-old-${i} `)
      }
      send({ id: msg.id, result: {} })
      text(sessionId, 'replayed-after-reply')
      update(sessionId, { sessionUpdate: 'tool_call', toolCallId: 'old-tool-late', title: 'late old tool', kind: 'read', status: 'completed' })
      return
    }
    update(sessionId, { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'old question' } })
    text(sessionId, 'replayed-old')
    send({ id: msg.id, result: {} })
  } else if (msg.method === 'session/set_config_option') {
    const session = sessions.get(msg.params.sessionId)
    if (session && msg.params.configId === 'model') session.model = msg.params.value
    send({ id: msg.id, result: { configOptions: [] } })
  } else if (msg.method === 'session/prompt') {
    const sessionId = msg.params.sessionId
    const prompt = msg.params.prompt?.[0]?.text ?? ''
    prompts.set(sessionId, msg.id)
    if (prompt === 'model-state') {
      text(sessionId, sessions.get(sessionId)?.model || 'stored-old-model')
      finish(sessionId, 'end_turn')
    } else if (prompt === 'empty' || prompt === 'status-only' || prompt === 'reasoning-only') {
      if (prompt === 'status-only') update(sessionId, { sessionUpdate: 'agent_message_chunk', _meta: { fox: { kind: 'backend_status' } }, content: { type: 'text', text: 'pi startup information' } })
      if (prompt === 'reasoning-only') update(sessionId, { sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: 'thinking' } })
      update(sessionId, { sessionUpdate: 'usage_update', used: 100, size: 1000 })
      finish(sessionId, 'end_turn')
    } else if (prompt === 'tools-only') {
      update(sessionId, { sessionUpdate: 'tool_call', toolCallId: 'delivery', title: 'write file', kind: 'edit', status: 'completed' })
      finish(sessionId, 'end_turn')
    } else if (prompt === 'partial-model-failure') {
      text(sessionId, 'partial answer')
      prompts.delete(sessionId)
      send({ id: msg.id, error: { code: -32603, message: '模型请求失败', data: { code: 'agent_backend_model_failed', failureKind: 'network_allowlist', redacted: true } } })
    } else if (prompt === 'after-load') {
      text(sessionId, 'fresh-after-load')
      update(sessionId, { sessionUpdate: 'tool_call', toolCallId: 'new-tool', title: 'new tool', kind: 'read', status: 'completed', rawInput: { path: 'new.txt' } })
      finish(sessionId, 'end_turn')
    } else if (prompt === 'simple') {
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
    } else if (prompt === 'reconnect') {
      // Codex's network retry notice, then a model-metadata warning; the
      // turn waits (like slow) until cancelled.
      text(sessionId, 'Model metadata for `deepseek-flash` not found. Defaulting to fallback metadata; this can degrade performance and cause issues.\n')
      text(sessionId, 'Reconnecting... 1/5 (stream disconnected before completion)\n')
    } else if (prompt === 'slow' || prompt === 'hang') {
      text(sessionId, `working:${prompt}`)
      if (prompt === 'hang') prompts.set(sessionId, 'hang')
    } else if (prompt === 'crash') {
      text(sessionId, 'about to crash')
      setTimeout(() => process.exit(3), 50)
    } else if (prompt === 'stderr-fail' || prompt === 'stderr-crash') {
      process.stderr.write('mock: connecting to http://127.0.0.1:3425/v1 with DEEPSEEK_API_KEY=sk-mock-0123456789abcdef\n')
      process.stderr.write('mock: request headers {"authorization":"Bearer mock-bearer-value","x-api-key":"mock-header-key"}\n')
      process.stderr.write('mock: Authorization: Bearer mock-plain-bearer token: mock-token-value\n')
      // A ~2000-byte line with no spaces (renders only if the tail wraps)
      // and a CJK line (byte vs char cutting, wide glyphs).
      process.stderr.write('mock: long ' + Array.from({ length: 150 }, (_, i) => `/segment-${String(i).padStart(4, '0')}`).join('') + ' end-of-long-line\n')
      process.stderr.write('mock: 错误：无法连接本机网关（127.0.0.1:3425），请检查 Magpie 是否在运行；路径 C:\\Users\\张三\\.dsh\n')
      process.stderr.write('mock: Error: Connection error (last line)\n')
      if (prompt === 'stderr-crash') {
        setTimeout(() => process.exit(4), 150)
      } else {
        prompts.delete(sessionId)
        setTimeout(() => send({ id: msg.id, error: { code: -32603, message: 'Internal error: DeepSeek Messages transport failed' } }), 150)
      }
    } else if (prompt === 'data-fail' || prompt === 'data-stderr-fail') {
      if (prompt === 'data-stderr-fail') process.stderr.write('mock: upstream said 401 (stderr line)\n')
      prompts.delete(sessionId)
      const data = {
        request: { url: 'https://api.example.test/v1/messages', headers: { authorization: 'Bearer mock-data-bearer-0001', 'x-api-key': 'mock-data-header-key' } },
        env: 'OPENAI_API_KEY=sk-mockdata-abcdef0123456789',
        apiKey: 'mock-data-apikey-value',
        hint: 'upstream rejected the credential',
      }
      setTimeout(() => send({ id: msg.id, error: { code: -32000, message: 'Authentication required', data } }), 100)
    } else if (prompt === 'maxtokens') {
      text(sessionId, 'partial')
      finish(sessionId, 'max_tokens')
    } else {
      text(sessionId, `echo:${prompt}`)
      finish(sessionId, 'end_turn')
    }
  } else if (msg.method === 'session/set_mode') {
    // Codex permission modes: accepted, except the id "refuse".
    if (msg.params.modeId === 'refuse') send({ id: msg.id, error: { code: -32602, message: 'unknown mode' } })
    else send({ id: msg.id, result: {} })
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
