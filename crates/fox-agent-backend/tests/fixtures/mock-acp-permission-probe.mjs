// 不依赖模型的 ACP v1 权限探针（stdio）。
// session/prompt 的文本是 JSON：
//   {"shape":"pi"|"codex","command":"<字符串>"|"[argv...]","cwd":"..."}
// pi：按 pi-acp 0.0.34 + fox-approval-gate 的 confirm 形状发 session/request_permission
//     （选项 yes=allow_once / no=reject_once，确认消息带 fox:"tool-approval"）。
// codex：按 codex-acp 形状发（rawInput.command 为字符串或字符串数组，rawInput.cwd；
//        选项 allow_once / reject_once）。
// 收到应答后发一条 agent_message_chunk：permission:<optionId> 或 permission:cancelled，
// 再以 end_turn 结束这一轮。
import readline from 'node:readline'

const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
let nextId = 1000
let nextSession = 1
let sessionId = null
let promptId = null
const waiting = new Map()

const chunk = (sid, text) => send({
  method: 'session/update',
  params: { sessionId: sid, update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text } } },
})

const endTurn = () => {
  const id = promptId
  promptId = null
  if (id !== null) send({ id, result: { stopReason: 'end_turn' } })
}

// pi-acp 把 ctx.ui.confirm 映射成的 request_permission（与 external_policy 的 gate 夹具一致）。
const piParams = (sid, command, cwd) => {
  const title = 'Fox approval: bash'
  const message = JSON.stringify({
    fox: 'tool-approval',
    v: 1,
    toolCallId: 'call_probe',
    toolName: 'bash',
    input: { command },
    cwd,
  })
  return {
    sessionId: sid,
    toolCall: {
      toolCallId: 'pi-ui-probe',
      title,
      kind: 'other',
      status: 'pending',
      rawInput: { method: 'confirm', title, message },
    },
    options: [
      { optionId: 'yes', name: 'Yes', kind: 'allow_once' },
      { optionId: 'no', name: 'No', kind: 'reject_once' },
    ],
  }
}

// codex-acp 的命令审批：command 原样放入 rawInput（字符串或 ["bash","-lc",脚本]）。
const codexParams = (sid, command, cwd) => ({
  sessionId: sid,
  toolCall: {
    toolCallId: 'codex-probe',
    title: 'command',
    kind: 'execute',
    rawInput: { command, cwd },
  },
  options: [
    { optionId: 'allow_once', name: 'Allow', kind: 'allow_once' },
    { optionId: 'reject_once', name: 'Reject', kind: 'reject_once' },
  ],
})

const ask = spec => {
  const sid = sessionId
  if (spec.shape === 'pi') {
    if (typeof spec.command !== 'string' || !spec.command.trim()) {
      chunk(sid, 'probe-error:pi command must be a string')
      endTurn()
      return
    }
    const id = nextId++
    waiting.set(id, sid)
    send({ id, method: 'session/request_permission', params: piParams(sid, spec.command, spec.cwd ?? '') })
    return
  }
  if (spec.shape === 'codex') {
    const command = spec.command
    const arrayOk = Array.isArray(command) && command.length > 0 && command.every(part => typeof part === 'string' && part.trim())
    const stringOk = typeof command === 'string' && command.trim()
    if (!arrayOk && !stringOk) {
      chunk(sid, 'probe-error:codex command must be a string or string array')
      endTurn()
      return
    }
    const id = nextId++
    waiting.set(id, sid)
    send({ id, method: 'session/request_permission', params: codexParams(sid, command, spec.cwd ?? '') })
    return
  }
  chunk(sid, `probe-error:unknown shape ${spec?.shape}`)
  endTurn()
}

rl.on('line', line => {
  let msg
  try {
    msg = JSON.parse(line)
  } catch {
    return
  }
  if (msg.method === 'initialize') {
    send({
      id: msg.id,
      result: {
        protocolVersion: 1,
        agentCapabilities: { loadSession: false },
        authMethods: [],
        agentInfo: { name: 'mock-permission-probe', version: '0' },
      },
    })
  } else if (msg.method === 'session/new' || msg.method === 'session/load') {
    sessionId = `probe-${nextSession++}`
    send({ id: msg.id, result: { sessionId } })
  } else if (msg.method === 'session/prompt') {
    promptId = msg.id
    sessionId = msg.params?.sessionId ?? sessionId
    let spec
    try {
      spec = JSON.parse(msg.params?.prompt?.[0]?.text ?? '')
    } catch {
      spec = null
    }
    if (!spec || typeof spec !== 'object') {
      chunk(sessionId, 'probe-error:prompt is not JSON')
      endTurn()
      return
    }
    ask(spec)
  } else if (msg.method === 'session/cancel') {
    // 本探针没有长任务。取消由 Host 直接答 cancelled。
  } else if (msg.id !== undefined && waiting.has(msg.id)) {
    const sid = waiting.get(msg.id)
    waiting.delete(msg.id)
    const outcome = msg.result?.outcome
    const label = outcome?.optionId ?? (outcome?.outcome === 'cancelled' || outcome == null ? 'cancelled' : outcome?.outcome)
    chunk(sid, `permission:${label}`)
    endTurn()
  } else if (msg.id !== undefined && msg.method) {
    send({ id: msg.id, error: { code: -32601, message: `unsupported ${msg.method}` } })
  }
})
