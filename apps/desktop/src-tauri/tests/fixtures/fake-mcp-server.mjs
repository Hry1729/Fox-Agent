import readline from 'node:readline'

const mode = process.argv[2] ?? 'valid'
const input = readline.createInterface({ input: process.stdin, terminal: false })

function send(id, result) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id, result })}\n`)
}

input.on('line', (line) => {
  const request = JSON.parse(line)
  if (request.method === 'initialize') {
    if (mode === 'timeout') return
    send(request.id, { protocolVersion: '2025-03-26', capabilities: { tools: {} }, serverInfo: { name: 'fake', version: '1.0.0' } })
    return
  }
  if (request.method === 'notifications/initialized') return
  if (request.method === 'tools/list') {
    send(request.id, { tools: [{ name: 'echo', description: 'Echo text', annotations: mode === 'readonly' ? { readOnlyHint: true, destructiveHint: false } : undefined, inputSchema: mode === 'invalid-schema' ? { type: 'string' } : { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] })
    return
  }
  if (request.method === 'tools/call') {
    if (mode === 'valid' && request.params.arguments.text === 'must-not-execute') process.exit(77)
    send(request.id, { content: [{ type: 'text', text: request.params.arguments.text }] })
  }
})
