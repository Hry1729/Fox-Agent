// Mock ACP v1 agent that USES the MCP servers the client hands it in
// session/new (plan C3, tests/fox_mcp_bridge.rs). Like Codex / dsh, it starts
// every stdio server itself (command + args + env), runs MCP initialize and
// tools/list, and calls tools when the prompt asks:
//   servers           -> text: JSON [{name, command, args, envNames}] received
//   tools             -> text: the tool names tools/list returned
//   call <tool> <json> -> tools/call <tool> with <json> arguments; text: the
//                        result JSON ({content, isError}); end_turn
//   ask               -> tools/call ask_user, then end_turn WITHOUT any text
import readline from 'node:readline'
import { spawn } from 'node:child_process'

const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
const text = (sessionId, value) => send({ method: 'session/update', params: { sessionId, update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: value } } } })
const sessions = new Map()
let nextSession = 1

function startMcp(server) {
  const env = { ...process.env }
  for (const variable of server.env || []) env[variable.name] = variable.value
  const child = spawn(server.command, server.args || [], { env, stdio: ['pipe', 'pipe', 'pipe'] })
  const pending = new Map()
  let nextId = 1
  let buffer = ''
  let stderr = ''
  child.stderr.on('data', chunk => { stderr += chunk })
  child.stdout.setEncoding('utf8')
  child.stdout.on('data', chunk => {
    buffer += chunk
    let index
    while ((index = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, index).trim()
      buffer = buffer.slice(index + 1)
      if (!line) continue
      const message = JSON.parse(line)
      const entry = pending.get(message.id)
      if (entry) { pending.delete(message.id); entry(message) }
      else if (message.id === null) for (const waiter of pending.values()) waiter(message)
    }
  })
  child.on('exit', code => { for (const waiter of pending.values()) waiter({ error: { message: `mcp server exited ${code}: ${stderr.trim()}` } }); pending.clear() })
  const request = (method, params) => new Promise(resolve => {
    const id = nextId++
    pending.set(id, resolve)
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n')
  })
  const ready = (async () => {
    const init = await request('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'mock-acp-mcp-agent', version: '0' } })
    if (init.error) throw new Error(init.error.message)
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n')
    const listed = await request('tools/list', {})
    if (listed.error) throw new Error(listed.error.message)
    return listed.result.tools.map(tool => tool.name)
  })()
  return { request, ready, child }
}

rl.on('line', async line => {
  const msg = JSON.parse(line)
  if (msg.method === 'initialize') {
    send({ id: msg.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: false, mcpCapabilities: { http: false, sse: false } }, authMethods: [], agentInfo: { name: 'mock-mcp', version: '0' } } })
  } else if (msg.method === 'session/new') {
    const sessionId = `mcp-sess-${nextSession++}`
    const servers = msg.params.mcpServers || []
    const clients = servers.map(startMcp)
    sessions.set(sessionId, { servers, clients })
    send({ id: msg.id, result: { sessionId } })
  } else if (msg.method === 'session/prompt') {
    const sessionId = msg.params.sessionId
    const prompt = msg.params.prompt?.[0]?.text ?? ''
    const session = sessions.get(sessionId)
    const done = () => send({ id: msg.id, result: { stopReason: 'end_turn' } })
    try {
      if (prompt === 'servers') {
        text(sessionId, JSON.stringify(session.servers.map(server => ({ name: server.name, command: server.command, args: server.args, envNames: (server.env || []).map(variable => variable.name) }))))
      } else if (prompt === 'tools') {
        text(sessionId, JSON.stringify(await session.clients[0].ready))
      } else if (prompt.startsWith('call ')) {
        const [, tool, ...rest] = prompt.split(' ')
        await session.clients[0].ready
        const answer = await session.clients[0].request('tools/call', { name: tool, arguments: JSON.parse(rest.join(' ') || '{}') })
        text(sessionId, JSON.stringify(answer.result ?? { error: answer.error }))
      } else if (prompt === 'ask') {
        await session.clients[0].ready
        await session.clients[0].request('tools/call', { name: 'ask_user', arguments: { questions: [{ question: 'Which format?', options: ['docx', 'pdf'] }] } })
      } else {
        text(sessionId, `unknown:${prompt}`)
      }
    } catch (error) {
      text(sessionId, `mcp-error:${error.message}`)
    }
    done()
  } else if (msg.method === 'session/cancel') {
    // nothing running long
  } else if (msg.id !== undefined && msg.method) {
    send({ id: msg.id, error: { code: -32601, message: `unsupported ${msg.method}` } })
  }
})
