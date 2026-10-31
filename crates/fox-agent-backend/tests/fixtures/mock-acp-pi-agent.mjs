// Mock "gated pi behind pi-acp" for plan C3 / C5 (tests/fox_mcp_bridge.rs).
// Launched THROUGH the real fixtures/pi/pi-gated.sh (FOX_PI_BIN = a wrapper that
// execs this file), so its argv is what the gate gives pi:
//   --no-extensions -e <fox-approval-gate.ts> [-e <FOX_PI_MCP_EXTENSION>] ...
// Like pi, it loads every `-e` extension except the approval gate (which needs
// pi's own event API) by importing it and calling its default export with a
// tiny pi API ({ registerTool }). That runs the REAL fox-mcp-tools.ts against
// the Host bridge (FOX_MCP_ENDPOINT / FOX_MCP_TOKEN from the environment).
// It speaks ACP v1 on stdio like pi-acp:
//   args              -> text: JSON {extensions, gateLoaded}
//   tools             -> text: JSON names registered by the extensions
//   call <tool> <json> -> the registered tool's execute(); text: JSON result
//                        or "tool-error:<message>" when it throws
//   ask               -> execute ask_user, end_turn WITHOUT any text
import readline from 'node:readline'
import { pathToFileURL } from 'node:url'
import { basename } from 'node:path'

const argv = process.argv.slice(2)
const extensions = []
for (let index = 0; index < argv.length; index++) {
  if (argv[index] === '-e' && argv[index + 1]) extensions.push(argv[++index])
}
const gateLoaded = extensions.some(path => basename(path) === 'fox-approval-gate.ts')
const tools = new Map()
const pi = { registerTool: tool => tools.set(tool.name, tool) }
let loading = null
const loadExtensions = () => {
  loading ??= (async () => {
    for (const path of extensions) {
      if (basename(path) === 'fox-approval-gate.ts') continue
      const module = await import(pathToFileURL(path).href)
      await module.default(pi)
    }
  })()
  return loading
}

const rl = readline.createInterface({ input: process.stdin })
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
const text = (sessionId, value) => send({ method: 'session/update', params: { sessionId, update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: value } } } })
let nextSession = 1

rl.on('line', async line => {
  const msg = JSON.parse(line)
  if (msg.method === 'initialize') {
    send({ id: msg.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: false }, authMethods: [], agentInfo: { name: 'mock-pi', version: '0' } } })
  } else if (msg.method === 'session/new') {
    const sessionId = `pi-sess-${nextSession++}`
    try {
      await loadExtensions()
      send({ id: msg.id, result: { sessionId } })
    } catch (error) {
      // pi does not start without the Fox tools Fox promised.
      send({ id: msg.id, error: { code: -32603, message: `extension failed: ${error.message}` } })
    }
  } else if (msg.method === 'session/prompt') {
    const sessionId = msg.params.sessionId
    const prompt = msg.params.prompt?.[0]?.text ?? ''
    try {
      if (prompt === 'args') {
        text(sessionId, JSON.stringify({ extensions: extensions.map(path => basename(path)), gateLoaded }))
      } else if (prompt === 'tools') {
        text(sessionId, JSON.stringify([...tools.keys()]))
      } else if (prompt.startsWith('call ')) {
        const [, name, ...rest] = prompt.split(' ')
        const tool = tools.get(name)
        if (!tool) throw new Error(`no tool ${name}`)
        try {
          text(sessionId, JSON.stringify(await tool.execute(`call-${Date.now()}`, JSON.parse(rest.join(' ') || '{}'))))
        } catch (error) {
          text(sessionId, `tool-error:${error.message}`)
        }
      } else if (prompt === 'ask') {
        await tools.get('ask_user').execute('call-ask', { questions: [{ question: 'Which format?', options: ['docx', 'pdf'] }] })
      } else {
        text(sessionId, `unknown:${prompt}`)
      }
    } catch (error) {
      text(sessionId, `pi-error:${error.message}`)
    }
    send({ id: msg.id, result: { stopReason: 'end_turn' } })
  } else if (msg.method === 'session/cancel') {
    // nothing long-running
  } else if (msg.id !== undefined && msg.method) {
    send({ id: msg.id, error: { code: -32601, message: `unsupported ${msg.method}` } })
  }
})
