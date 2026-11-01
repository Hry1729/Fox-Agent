// 假 ACP v1 后端，供 tests/e2e_full_turn.rs 在同一会话里走完一整轮。
// prompt：
//   e2e-tool   流式文字，然后请求一次工具审批。收到 allow_once 后真的执行
//              node -e "console.log('tool-ok')"，把标准输出当作工具结果回传。
//   e2e-plain  普通回复，end_turn。
//   e2e-resume 取消之后的普通回复，end_turn。
//   e2e-hold   拉起一个长命子进程（node -e "setInterval(()=>{},1000)"），
//              用文字回报子进程和本进程的 pid。session/cancel 正常以 cancelled 结束，
//              但故意不杀这个子进程（模拟后端把工具子进程留下）。
import readline from 'node:readline'
import { spawn } from 'node:child_process'

const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
const update = (sessionId, body) => send({ method: 'session/update', params: { sessionId, update: body } })
const text = (sessionId, value) => update(sessionId, { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: value } })

let nextSession = 1
let nextId = 1000
const waiting = new Map()
const prompts = new Map()
// 故意一直持有长命子进程。取消时不调用 kill，留给 Host 清扫。
const retained = []

const finish = (sessionId, stopReason) => {
  const promptId = prompts.get(sessionId)
  if (promptId === undefined) return
  prompts.delete(sessionId)
  send({ id: promptId, result: { stopReason } })
}

// stdin 用 ignore：管道上的 stdin 会让 node -e 在脚本结束后仍不退出。
function runTool() {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['-e', "console.log('tool-ok')"], {
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    })
    let out = ''
    let err = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', chunk => { out += chunk })
    child.stderr.on('data', chunk => { err += chunk })
    child.once('error', reject)
    child.once('close', code => {
      if (code === 0) resolve(out)
      else reject(new Error(`exit ${code} stdout=${JSON.stringify(out)} stderr=${JSON.stringify(err)}`))
    })
  })
}

function startLongChild() {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], {
      stdio: 'ignore',
      windowsHide: true,
    })
    retained.push(child)
    child.once('error', reject)
    child.once('spawn', () => (child.pid ? resolve(child.pid) : reject(new Error('child has no pid'))))
  })
}

function toolUpdate(sessionId, status, output) {
  update(sessionId, {
    sessionUpdate: 'tool_call_update',
    toolCallId: 't1',
    status,
    content: [{ type: 'content', content: { type: 'text', text: output } }],
  })
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
        agentCapabilities: { loadSession: true },
        authMethods: [],
        agentInfo: { name: 'mock-e2e', version: '0' },
      },
    })
  } else if (msg.method === 'session/new') {
    send({ id: msg.id, result: { sessionId: `e2e-${process.pid}-${nextSession++}` } })
  } else if (msg.method === 'session/load') {
    send({ id: msg.id, result: {} })
  } else if (msg.method === 'session/set_mode') {
    send({ id: msg.id, result: {} })
  } else if (msg.method === 'session/prompt') {
    const sessionId = msg.params.sessionId
    const prompt = msg.params.prompt?.[0]?.text ?? ''
    prompts.set(sessionId, msg.id)
    if (prompt === 'e2e-tool') {
      text(sessionId, '先看工作区，')
      text(sessionId, '再执行命令。')
      update(sessionId, {
        sessionUpdate: 'tool_call',
        toolCallId: 't1',
        title: '运行检查',
        kind: 'execute',
        status: 'pending',
        rawInput: { command: 'node -e "console.log(\'tool-ok\')"' },
      })
      const id = nextId++
      waiting.set(id, sessionId)
      send({
        id,
        method: 'session/request_permission',
        params: {
          sessionId,
          toolCall: {
            toolCallId: 't1',
            title: '运行检查',
            kind: 'execute',
            rawInput: { command: 'node -e "console.log(\'tool-ok\')"' },
          },
          options: [
            { optionId: 'allow', name: '允许一次', kind: 'allow_once' },
            { optionId: 'always', name: '总是允许', kind: 'allow_always' },
            { optionId: 'reject', name: '拒绝', kind: 'reject_once' },
          ],
        },
      })
    } else if (prompt === 'e2e-plain') {
      text(sessionId, '同一会话的第二轮。')
      finish(sessionId, 'end_turn')
    } else if (prompt === 'e2e-resume') {
      text(sessionId, '取消后可以继续。')
      finish(sessionId, 'end_turn')
    } else if (prompt === 'e2e-hold') {
      startLongChild().then(pid => {
        process.stderr.write(`mock-acp-e2e-agent: long child ${pid} backend ${process.pid}\n`)
        if (!prompts.has(sessionId)) return
        text(sessionId, `已拉起子进程 child-pid:${pid};backend-pid:${process.pid}`)
      }, error => {
        process.stderr.write(`mock-acp-e2e-agent: long child failed ${error && error.message ? error.message : error}\n`)
        if (!prompts.has(sessionId)) return
        text(sessionId, `spawn-failed:${error && error.message ? error.message : error}`)
        finish(sessionId, 'end_turn')
      })
    } else {
      text(sessionId, `未知请求：${prompt}`)
      finish(sessionId, 'end_turn')
    }
  } else if (msg.method === 'session/cancel') {
    const sessionId = msg.params?.sessionId
    // 以 cancelled 结束本轮，故意不杀 retained 里的子进程。
    if (sessionId) finish(sessionId, 'cancelled')
    else for (const id of [...prompts.keys()]) finish(id, 'cancelled')
  } else if (msg.id !== undefined && waiting.has(msg.id)) {
    const sessionId = waiting.get(msg.id)
    waiting.delete(msg.id)
    const optionId = msg.result?.outcome?.optionId
    if (optionId !== 'allow') {
      toolUpdate(sessionId, 'failed', '未允许')
      text(sessionId, '未允许执行。')
      finish(sessionId, 'end_turn')
      return
    }
    runTool().then(output => {
      if (!prompts.has(sessionId)) return
      toolUpdate(sessionId, 'completed', output)
      text(sessionId, '命令已完成。')
      finish(sessionId, 'end_turn')
    }, error => {
      const detail = error && error.message ? error.message : String(error)
      process.stderr.write(`mock-acp-e2e-agent: tool failed ${detail}\n`)
      if (!prompts.has(sessionId)) return
      toolUpdate(sessionId, 'failed', detail)
      text(sessionId, '命令没有跑成。')
      finish(sessionId, 'end_turn')
    })
  } else if (msg.id !== undefined && msg.method) {
    send({ id: msg.id, error: { code: -32601, message: `method not found: ${msg.method}` } })
  }
})
