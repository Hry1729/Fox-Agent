import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { openNativeModelProxy } from '../src/native-model-proxy.mjs'

for (const mode of ['complete', 'truncated', 'provider-error']) {
  test(`native transport ${mode}: one request, bounded completed batch only`, async () => {
    let calls = 0
    const output = ['first', 'second'].map(call_id => ({ type: 'function_call', call_id, name: 'read', arguments: '{"path":"中文.txt"}' }))
    const server = createServer(async (req, res) => {
      calls++
      let input = ''; for await (const chunk of req) input += chunk
      const body = JSON.parse(input)
      assert.equal(body.store, false)
      assert.equal(body.max_output_tokens, 256)
      assert.deepEqual(body.tools.map(tool => tool.name), ['read'])
      if (mode === 'provider-error') { res.writeHead(429); res.end('secret provider diagnostic'); return }
      res.writeHead(200, { 'content-type': 'text/event-stream' })
      res.end(`data: ${JSON.stringify(mode === 'complete'
        ? { type: 'response.completed', response: { status: 'completed', output } }
        : { type: 'response.output_item.done', item: output[0] })}\n\n`)
    })
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
    const proxy = await openNativeModelProxy({ modelId: 'test', baseUrl: `http://127.0.0.1:${server.address().port}`, maxOutputTokens: 256 },
      'test-key', undefined, [{ name: 'read', description: 'read', parameters: { type: 'object' } }])
    const invoke = () => fetch(`${proxy.baseUrl}/responses`, { method: 'POST', headers: { authorization: `Bearer ${proxy.secret}` },
      body: JSON.stringify({ model: 'test', stream: true, tools: [{ name: 'shell' }] }) })
    try {
      const response = await invoke()
      assert.equal(response.status, mode === 'complete' ? 200 : 502)
      assert.ok(!(await response.text()).includes('secret provider diagnostic'))
      assert.deepEqual(proxy.modelOutput, mode === 'complete' ? output : null)
      assert.equal((await invoke()).status, 403)
      assert.equal(calls, 1)
    } finally { await proxy.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve)) }
  })
}
