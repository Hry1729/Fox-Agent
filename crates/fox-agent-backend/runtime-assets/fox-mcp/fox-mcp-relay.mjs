// Fox tool MCP bridge: stdio relay (plan C3). The external Agent (Codex, dsh)
// starts this as the stdio MCP server Fox handed it in ACP `session/new`
// `mcpServers`. It only moves bytes: stdin -> Host loopback socket, socket ->
// stdout. All MCP handling happens in the Fox Host (fox_mcp.rs).
//
// FOX_MCP_ENDPOINT  127.0.0.1:<port> of the Host bridge
// FOX_MCP_TOKEN     token issued for this Agent launch (first line on the socket)
//
// Fails loudly, never silently: a missing variable, a refused connection or a
// closed socket ends the process with a non-zero exit and a stderr line the
// Agent shows in its MCP diagnostics.
import net from 'node:net'

const endpoint = process.env.FOX_MCP_ENDPOINT || ''
const token = process.env.FOX_MCP_TOKEN || ''
const fail = (message, code = 2) => {
  process.stderr.write(`fox-mcp-relay: ${message}\n`)
  process.exit(code)
}
const match = /^127\.0\.0\.1:(\d{1,5})$/.exec(endpoint)
if (!match) fail('FOX_MCP_ENDPOINT must be 127.0.0.1:<port> (set by Fox)')
if (!/^[0-9a-f]{64}$/.test(token)) fail('FOX_MCP_TOKEN missing or malformed (set by Fox)')

let stdinEnded = false
const socket = net.connect({ host: '127.0.0.1', port: Number(match[1]) })
socket.setNoDelay(true)
socket.on('connect', () => {
  socket.write(JSON.stringify({ foxMcpToken: token }) + '\n')
  process.stdin.pipe(socket)
})
socket.pipe(process.stdout)
socket.on('error', error => fail(`cannot reach the Fox Host bridge at ${endpoint}: ${error.message}`, 3))
socket.on('close', () => {
  // The Host closed the bridge (token revoked: the Agent process it belongs
  // to is stopping) or the Host exited.
  process.stdout.write('', () => process.exit(stdinEnded ? 0 : 4))
})
process.stdin.on('end', () => {
  stdinEnded = true
  socket.end()
})
