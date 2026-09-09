// Owned local HTTP provider for Rust-to-worker-to-native-engine acceptance.
import { createServer } from 'node:http'
import { appendFileSync } from 'node:fs'
const [engine, log] = process.argv.slice(2)
let count = 0
const server = createServer(async (req, res) => {
  let input = ''; for await (const chunk of req) input += chunk
  appendFileSync(log, `${input}\n`)
  count++
  res.writeHead(200, { 'content-type': 'text/event-stream' })
  const calls = ['read-a','read-b'].map(id => ({ type: 'function_call', id: `fc_${id}`, call_id: id, name: 'read', arguments: '{"path":"proof.txt"}' }))
  if (engine === 'codex') {
    const items = count === 1 ? calls : [{ type: 'message', id: 'msg-native-final', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: 'Native engine resumed confirmed results 中文 😀', annotations: [] }] }]
    const events = [{ type: 'response.created', response: { id: `resp_${count}` } },
      ...items.flatMap((item, output_index) => [{ type: 'response.output_item.added', output_index, item }, { type: 'response.output_item.done', output_index, item }]),
      { type: 'response.completed', response: { id: `resp_${count}`, status: 'completed', output: items, usage: { input_tokens: 20, output_tokens: 8, total_tokens: 28 } } }]
    for (const event of events) res.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
    res.end()
  } else {
    const delta = count === 1 ? { tool_calls: calls.map((call, index) => ({ index, id: call.call_id, type: 'function', function: { name: call.name, arguments: call.arguments } })) } : { content: 'Native engine resumed confirmed results 中文 😀' }
    for (const choice of [{ index: 0, delta: { role: 'assistant', ...delta }, finish_reason: null }, { index: 0, delta: {}, finish_reason: count === 1 ? 'tool_calls' : 'stop' }]) {
      res.write(`data: ${JSON.stringify({ id: `resp_${count}`, object: 'chat.completion.chunk', model: 'deepseek-v4-flash', created: 1, choices: [choice] })}\n\n`)
    }
    res.end('data: [DONE]\n\n')
  }
})
server.listen(0, '127.0.0.1', () => console.log(`http://127.0.0.1:${server.address().port}/v1`))
process.stdin.resume()
process.stdin.on('end', () => { server.closeAllConnections(); server.close() })
