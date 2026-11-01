// 假 ACP v1 后端：prompt 为 spawn-child 时拉起两个长驻子进程，取消时不杀它们（模拟 dsh）。
// spawn-grandchild 时起一个 shell，由它再起长驻的 ping / sleep（孙进程，模拟 Codex 的 pwsh -> PING.EXE）。
// hello 正常结束，用来确认取消清扫之后同一条连接还能用。
import readline from 'node:readline'
import { spawn } from 'node:child_process'

const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
const text = (sessionId, value) => send({
  method: 'session/update',
  params: { sessionId, update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: value } } },
})
const prompts = new Map()
// 故意一直持有子进程，取消时也不调用 kill。
const children = []

const finish = (sessionId, stopReason) => {
  const promptId = prompts.get(sessionId)
  if (promptId === undefined) return
  prompts.delete(sessionId)
  send({ id: promptId, result: { stopReason } })
}

// stdio ignore、windowsHide。第二个带 detached，覆盖 Unix 上离开进程组的情况。
function startChild(detached) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['-e', 'setInterval(()=>{}, 1000)'], {
      stdio: 'ignore',
      windowsHide: true,
      detached,
    })
    children.push(child)
    const fail = error => {
      child.removeListener('spawn', ok)
      reject(error)
    }
    const ok = () => {
      child.removeListener('error', fail)
      if (!child.pid) {
        reject(new Error('child has no pid'))
        return
      }
      if (detached) child.unref()
      resolve(child.pid)
    }
    child.once('error', fail)
    child.once('spawn', ok)
  })
}

// shell 子进程 -> ping/sleep 孙进程。回报 shell 的 pid，孙进程由测试按父进程找。
function startShellWithGrandchild() {
  return new Promise((resolve, reject) => {
    const [command, args] = process.platform === 'win32'
      ? ['cmd.exe', ['/d', '/s', '/c', 'ping -n 600 127.0.0.1 > nul']]
      : ['/bin/sh', ['-c', 'sleep 600; :']]
    const child = spawn(command, args, { stdio: 'ignore', windowsHide: true })
    children.push(child)
    child.once('error', reject)
    child.once('spawn', () => (child.pid ? resolve(child.pid) : reject(new Error('shell has no pid'))))
  })
}

async function spawnPair() {
  const first = await startChild(false)
  let second
  try {
    second = await startChild(true)
  } catch (error) {
    // Windows 的 Job 没有 BREAKAWAY_OK，Node 的 detached 会带 CREATE_BREAKAWAY_FROM_JOB，启动失败。
    // 再起一个留在 Job 里的子进程，保证两个 pid 都能被 Host 清扫。
    process.stderr.write(`mock-acp-child-agent: detached spawn failed (${error.message}); starting a second in-job child\n`)
    second = await startChild(false)
  }
  return [first, second]
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
        agentInfo: { name: 'mock-child', version: '0' },
      },
    })
  } else if (msg.method === 'session/new') {
    send({ id: msg.id, result: { sessionId: `child-${process.pid}` } })
  } else if (msg.method === 'session/load') {
    send({ id: msg.id, result: {} })
  } else if (msg.method === 'session/prompt') {
    const sessionId = msg.params.sessionId
    const prompt = msg.params.prompt?.[0]?.text ?? ''
    prompts.set(sessionId, msg.id)
    if (prompt === 'spawn-child') {
      spawnPair().then(pids => {
        process.stderr.write(`mock-acp-child-agent: children ${pids.join(',')}\n`)
        if (!prompts.has(sessionId)) return
        text(sessionId, `pids:${pids[0]},${pids[1]}`)
        // 故意不结束这一轮，等 session/cancel。
      }, error => {
        if (!prompts.has(sessionId)) return
        text(sessionId, `spawn-failed:${error && error.message ? error.message : error}`)
        finish(sessionId, 'end_turn')
      })
    } else if (prompt === 'spawn-grandchild') {
      startShellWithGrandchild().then(pid => {
        process.stderr.write(`mock-acp-child-agent: shell ${pid}\n`)
        if (!prompts.has(sessionId)) return
        text(sessionId, `shell:${pid};`)
      }, error => {
        if (!prompts.has(sessionId)) return
        text(sessionId, `spawn-failed:${error && error.message ? error.message : error}`)
        finish(sessionId, 'end_turn')
      })
    } else if (prompt === 'hello') {
      text(sessionId, 'hello-ok')
      finish(sessionId, 'end_turn')
    } else {
      text(sessionId, `unknown:${prompt}`)
      finish(sessionId, 'end_turn')
    }
  } else if (msg.method === 'session/cancel') {
    const sessionId = msg.params?.sessionId
    // 以 cancelled 结束本轮，故意不杀子进程。
    if (sessionId) finish(sessionId, 'cancelled')
    else for (const id of [...prompts.keys()]) finish(id, 'cancelled')
  } else if (msg.id !== undefined && msg.method) {
    send({ id: msg.id, error: { code: -32601, message: `method not found: ${msg.method}` } })
  }
})
