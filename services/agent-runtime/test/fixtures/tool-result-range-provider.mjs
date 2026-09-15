// Deterministic HTTP provider: choose the next call only from the tool message
// received over HTTP. It has no access to Host details or the source contents.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { createHash } from 'node:crypto'

const marker = 'FOX_RESULT_CURSOR_V1 '
const keys = ['reference', 'offset', 'returnedBytes', 'nextOffset', 'complete', 'originalBytes', 'retrievable', 'truncated']
const hash = text => createHash('sha256').update(text).digest('hex')

export async function startRangeProvider(reference, limit = 4096, onComplete = () => {}) {
  const fragments = []
  const pages = []
  let report = null
  let failure = null
  let requests = 0
  const server = createServer(async (req, res) => {
    try {
      // HTTP chunks may end inside a Chinese character or emoji.
      req.setEncoding('utf8')
      let raw = ''
      for await (const chunk of req) raw += chunk
      const body = JSON.parse(raw)
      requests++
      assert.ok(requests < 100, 'range walk must terminate')
      const tool = body.messages.findLast(message => message.role === 'tool')
      let offset = 0
      if (tool) {
        const at = tool.content.lastIndexOf(`\n${marker}`)
        assert.ok(at >= 0, 'actual HTTP tool input must expose navigation')
        const nav = JSON.parse(tool.content.slice(at + 1 + marker.length))
        assert.deepEqual(Object.keys(nav).sort(), [...keys].sort(), 'only public navigation fields reach the cursor')
        assert.equal(nav.reference, reference)
        assert.equal(nav.offset, Buffer.byteLength(fragments.join('')))
        const fragment = Buffer.from(tool.content).subarray(0, nav.returnedBytes).toString('utf8')
        assert.equal(Buffer.byteLength(fragment), nav.returnedBytes, 'range ends at a code point boundary')
        fragments.push(fragment)
        pages.push({ ...nav, fragmentHash: hash(fragment), toolCallId: tool.tool_call_id,
          fragmentPreview: fragment.slice(0, 100),
          providerCursor: tool.content.slice(at + 1) })
        if (nav.complete) {
          assert.equal(nav.nextOffset, null)
          if (nav.retrievable) assert.equal(Buffer.byteLength(fragments.join('')), nav.originalBytes)
          report = { requests, pages, returnedBytes: Buffer.byteLength(fragments.join('')), sha256: hash(fragments.join('')) }
          onComplete(report)
        } else {
          assert.equal(nav.nextOffset, nav.offset + nav.returnedBytes)
          assert.ok(nav.nextOffset > nav.offset)
          offset = nav.nextOffset
        }
      }
      res.writeHead(200, { 'content-type': 'text/event-stream' })
      const delta = report ? { role: 'assistant', content: 'Ranges verified from received cursor.\n<fox-final/>' }
        : { role: 'assistant', tool_calls: [{ index: 0, id: `range-read-${pages.length + 1}`, type: 'function',
            function: { name: 'read_tool_result', arguments: JSON.stringify({ reference, offset, limit }) } }] }
      for (const choice of [{ index: 0, delta, finish_reason: null },
        { index: 0, delta: {}, finish_reason: report ? 'stop' : 'tool_calls' }]) {
        res.write(`data: ${JSON.stringify({ id: `range-${requests}`, object: 'chat.completion.chunk', created: 1,
          model: 'range-provider', choices: [choice] })}\n\n`)
      }
      res.end('data: [DONE]\n\n')
    } catch (error) {
      failure = error
      res.writeHead(500, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ error: { message: error.message } }))
    }
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  return { baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    get report() { if (failure) throw failure; return report },
    close() { server.closeAllConnections(); server.close() } }
}

if (process.argv[2] === '--serve') {
  const provider = await startRangeProvider(process.argv[3], Number(process.argv[4]),
    report => process.stdout.write(`${JSON.stringify({ report })}\n`))
  process.stdout.write(`${JSON.stringify({ baseUrl: provider.baseUrl })}\n`)
}
