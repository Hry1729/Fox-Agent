import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { codexHistory, runCodexKernelModel } from '../src/codex-kernel-adapter.mjs'

test('Codex checkpoint conversion retains tool identity and error evidence', () => {
  const result = codexHistory([
    { role: 'user', content: 'original request' },
    { role: 'assistant', content: [{ type: 'toolCall', id: 'call1', name: 'read', arguments: { path: 'proof.txt' } }] },
    { role: 'toolResult', toolCallId: 'call1', toolName: 'read', content: [{ type: 'text', text: 'FOX_EXECUTION_RECEIPT_V1 failed' }], isError: true },
  ])
  assert.equal(result[1].call_id, 'call1')
  assert.equal(result[2].call_id, 'call1')
  assert.equal(JSON.parse(result[2].output).isError, true)
  assert.match(result[2].output, /FOX_EXECUTION_RECEIPT_V1/)
  const reasoning = { type: 'reasoning', id: 'rs_previous', summary: [{ type: 'summary_text', text: 'retained reasoning' }], encrypted_content: 'opaque-original-state' }
  assert.deepEqual(codexHistory([{ role: 'assistant', content: [{ type: 'thinking', thinking: 'retained reasoning', foxCodexReasoning: reasoning }] }]), [reasoning])
  assert.ok(JSON.stringify(codexHistory([{ role: 'assistant', content: [{ type: 'thinking', thinking: 'foreign reasoning data' }] }])).includes('foreign reasoning data'))
})

const binary = process.env.FOX_TEST_CODEX_BINARY
test('native Codex has no environment tools and returns a Fox proposal without executing it', { skip: !binary, timeout: 90000 }, async () => {
  const bodies = []
  const server = createServer(async (request, response) => {
    const chunks = []; for await (const chunk of request) chunks.push(chunk)
    bodies.push(JSON.parse(Buffer.concat(chunks).toString()))
    const item = { type: 'function_call', id: 'fc_native', call_id: 'native-read', name: 'read', arguments: '{"path":"proof.txt"}' }
    response.writeHead(200, { 'content-type': 'text/event-stream' })
    for (const event of [
      { type: 'response.created', response: { id: 'resp_native' } },
      { type: 'response.output_item.added', output_index: 0, item: { ...item, arguments: '' } },
      { type: 'response.output_item.done', output_index: 0, item },
      { type: 'response.completed', response: { id: 'resp_native', status: 'completed', output: [item], usage: { input_tokens: 20, output_tokens: 8, total_tokens: 28 } } },
    ]) response.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
    response.end()
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  try {
    const result = await runCodexKernelModel({ config: {
      nativeAdapter: { command: binary, binaryHash: `sha256:${createHash('sha256').update(await readFile(binary)).digest('hex')}` },
      modelService: { apiType: 'openai-responses', modelId: 'gpt-5.1-codex', baseUrl: `http://127.0.0.1:${server.address().port}/v1`, maxOutputTokens: 512, requestTimeoutMs: 30000 },
      systemPrompt: 'Use only Host tools. Propose one read.', proposalTools: [{ name: 'read', description: 'Read a project file through Host.', parameters: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'], additionalProperties: false } }],
    }, messages: [{ role: 'user', content: 'Read proof.txt' }], apiKey: 'isolated-test' })
    assert.equal(result.stopReason, 'toolUse')
    assert.equal(result.content.find(block => block.type === 'toolCall').id, 'native-read')
    assert.equal(bodies.length, 1)
    assert.equal(bodies[0].max_output_tokens, 512)
    const names = bodies[0].tools.flatMap(tool => tool.type === 'namespace' ? tool.tools.map(t => t.name) : [tool.name ?? tool.type])
    assert.ok(names.includes('read'))
    for (const forbidden of ['exec_command', 'shell', 'apply_patch', 'web_search', 'spawn_agent']) assert.ok(!names.includes(forbidden), forbidden)
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)) }
})
