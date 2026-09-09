// A private, single-request model transport. Native engines never receive the
// provider credential, choose another model, or spend a second request on retry.
import { createServer } from 'node:http'
import { randomUUID } from 'node:crypto'

export async function openNativeModelProxy(modelService, apiKey, signal, proposalTools = []) {
  const secret = randomUUID()
  const controller = new AbortController()
  const abort = () => controller.abort()
  signal?.addEventListener('abort', abort, { once: true })
  if (signal?.aborted) abort()
  let used = false
  let upstreamRequests = 0
  let modelOutput = null
  let modelUsage = null
  const server = createServer(async (request, response) => {
    const fail = status => { if (!response.headersSent) response.writeHead(status); response.end() }
    if (request.headers.authorization !== `Bearer ${secret}` || request.method !== 'POST'
        || request.url !== '/v1/responses' || used || controller.signal.aborted) return fail(403)
    used = true
    try {
      const chunks = []; let size = 0
      for await (const chunk of request) {
        size += chunk.length
        if (size > 2_097_152) throw new Error('native model request too large')
        chunks.push(chunk)
      }
      const body = JSON.parse(Buffer.concat(chunks).toString('utf8'))
      if (body.model !== modelService.modelId || body.stream !== true) throw new Error('native model route mismatch')
      body.max_output_tokens = Math.min(modelService.maxOutputTokens ?? 8192, 131072)
      body.store = false
      body.tools = proposalTools.map(tool => ({ type: 'function', name: tool.name, description: tool.description, parameters: tool.parameters }))
      const endpoint = `${modelService.baseUrl.replace(/\/$/, '')}/responses`
      upstreamRequests++
      const upstream = await fetch(endpoint, { method: 'POST', headers: { 'content-type': 'application/json', authorization: `Bearer ${apiKey}` },
        body: JSON.stringify(body), signal: controller.signal })
      // Never give a provider diagnostic to a native retry/error log.
      if (!upstream.ok || !upstream.body) { await upstream.body?.cancel(); return fail(502) }
      let received = 0
      const stream = []
      for await (const chunk of upstream.body) {
        received += chunk.length
        if (received > 8_388_608 || controller.signal.aborted) throw new Error('native model response exceeded bounds')
        stream.push(chunk)
      }
      // Finish the bounded model turn before exposing it to Codex. Dynamic
      // tools are dispatched serially by some native versions, so interrupting
      // the first request must not discard the rest of the model's proposal batch.
      const bytes = Buffer.concat(stream)
      const events = bytes.toString('utf8').split(/\r?\n\r?\n/).flatMap(frame => {
        const data = frame.split(/\r?\n/).filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n')
        return data && data !== '[DONE]' ? [JSON.parse(data)] : []
      })
      const terminal = events.filter(event => event.type === 'response.completed')
      if (terminal.length !== 1 || terminal[0].response?.status !== 'completed') throw new Error('native model turn incomplete')
      modelOutput = terminal[0].response.output ?? events.filter(event => event.type === 'response.output_item.done').map(event => event.item)
      if (!Array.isArray(modelOutput)) throw new Error('native model output invalid')
      modelUsage = terminal[0].response.usage ?? null
      response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-store' })
      response.write(bytes)
      response.end()
    } catch { controller.abort(); fail(502) }
  })
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
  return { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, secret,
    get upstreamRequests() { return upstreamRequests },
    get modelOutput() { return modelOutput },
    get modelUsage() { return modelUsage },
    async close() { signal?.removeEventListener('abort', abort); controller.abort(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve)) },
  }
}
