// Redacted capture of one provider HTTP request, plus a local OpenAI-compatible stub.
//
// This module never reads, writes or prints a credential: the `authorization`
// header value is replaced by a fixed marker before a request is recorded, and
// the stub answers any token with a canned SSE stream, so no real key is needed.
import { createServer } from 'node:http'

export const REDACTED = '<redacted>'

const SENSITIVE_HEADERS = new Set(['authorization', 'proxy-authorization', 'x-api-key', 'api-key', 'cookie', 'set-cookie'])

/** Header bag -> printable object with every credential value replaced. */
export function redactHeaders(headers = {}) {
  const out = {}
  for (const [rawName, rawValue] of Object.entries(headers)) {
    const name = rawName.toLowerCase()
    const value = Array.isArray(rawValue) ? rawValue.join(', ') : String(rawValue ?? '')
    if (!SENSITIVE_HEADERS.has(name)) { out[name] = value; continue }
    // Keep the scheme so a Bearer/Anthropic-style difference stays visible.
    const scheme = value.includes(' ') ? `${value.slice(0, value.indexOf(' '))} ` : ''
    out[name] = `${scheme}${REDACTED}`
  }
  return Object.fromEntries(Object.entries(out).sort(([a], [b]) => a.localeCompare(b)))
}

function sseChunk(delta, finishReason = null, extra = {}) {
  return {
    id: 'chatcmpl-offline-capture',
    object: 'chat.completion.chunk',
    created: 1,
    model: 'offline-capture-model',
    choices: [{ index: 0, delta, finish_reason: finishReason }],
    ...extra,
  }
}

/**
 * Valid OpenAI-compatible SSE answer (two content deltas, a stop chunk and a
 * usage-only chunk because `stream_options.include_usage` is on).
 */
export function sseBody() {
  const frames = [
    sseChunk({ role: 'assistant', content: 'offline ' }),
    sseChunk({ content: 'capture' }),
    sseChunk({}, 'stop'),
    { id: 'chatcmpl-offline-capture', object: 'chat.completion.chunk', created: 1,
      model: 'offline-capture-model', choices: [], usage: { prompt_tokens: 11, completion_tokens: 3, total_tokens: 14 } },
  ]
  return `${frames.map(frame => `data: ${JSON.stringify(frame)}\n\n`).join('')}data: [DONE]\n\n`
}

/**
 * Start the recording stub. `basePath` defaults to `/v1`, so the port it
 * returns is the same shape a provider base URL has: http://127.0.0.1:<port>/v1.
 */
export async function startRecordingServer({ basePath = '/v1', forwardTo = null } = {}) {
  const requests = []
  const server = createServer(async (request, response) => {
    const chunks = []
    let size = 0
    for await (const chunk of request) {
      size += chunk.length
      if (size > 4 * 1024 * 1024) { response.writeHead(413).end(); return }
      chunks.push(chunk)
    }
    const raw = Buffer.concat(chunks).toString('utf8')
    const contentType = String(request.headers['content-type'] ?? '')
    let body = null
    let bodyError = null
    if (raw) {
      if (contentType.includes('json') || raw.trimStart().startsWith('{')) {
        try { body = JSON.parse(raw) } catch (error) { bodyError = String(error?.message ?? error) }
      } else {
        bodyError = `non-JSON body (${contentType || 'no content-type'})`
      }
    }
    const record = {
      at: new Date().toISOString(),
      method: request.method,
      url: request.url,
      path: (() => { try { return new URL(request.url, 'http://127.0.0.1').pathname } catch { return request.url } })(),
      query: (() => { try { const q = new URL(request.url, 'http://127.0.0.1').search; return q || '' } catch { return '' } })(),
      headers: redactHeaders(request.headers),
      httpVersion: request.httpVersion,
      bodyBytes: Buffer.byteLength(raw, 'utf8'),
      body,
      ...(bodyError ? { bodyError } : {}),
    }
    // Optional pass-through: forward the captured request to a real provider so
    // the caller can compare the endpoint's own answer per path. The request
    // already carries the caller-supplied (fake) credential; nothing is added.
    if (forwardTo) {
      const upstreamUrl = new URL(request.url, forwardTo).toString()
      try {
        const upstream = await fetch(upstreamUrl, {
          method: request.method,
          headers: {
            'content-type': request.headers['content-type'] ?? 'application/json',
            authorization: request.headers.authorization ?? '',
            accept: 'application/json',
          },
          body: raw || undefined,
        })
        const upstreamBody = await upstream.text()
        record.upstreamUrl = upstreamUrl
        record.upstreamStatus = upstream.status
        record.upstreamBody = upstreamBody.slice(0, 600)
        requests.push(record)
        response.writeHead(upstream.status, { 'content-type': upstream.headers.get('content-type') ?? 'application/json' })
        response.end(upstreamBody)
      } catch (error) {
        record.upstreamError = String(error?.message ?? error)
        requests.push(record)
        response.writeHead(502, { 'content-type': 'application/json' })
        response.end(JSON.stringify({ diagnostics: 'forward failed' }))
      }
      return
    }
    requests.push(record)
    const wantsStream = body?.stream === true || request.headers.accept?.includes('text/event-stream')
    if (wantsStream) {
      response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-store' })
      response.end(sseBody())
      return
    }
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ object: 'list', data: [{ id: 'offline-capture-model', object: 'model' }] }))
  })
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const origin = `http://127.0.0.1:${server.address().port}`
  return {
    origin,
    baseUrl: `${origin}${basePath}`,
    requests,
    reset() { requests.length = 0 },
    async close() {
      server.closeAllConnections?.()
      await new Promise(resolve => server.close(resolve))
    },
  }
}

/** Stable, comparison-friendly projection of one captured request. */
export function requestShape(record) {
  return {
    method: record.method,
    path: record.path,
    query: record.query,
    contentType: record.headers['content-type'] ?? null,
    userAgent: record.headers['user-agent'] ?? null,
    authorization: record.headers.authorization ?? null,
    bodyKeys: record.body ? Object.keys(record.body).sort() : [],
    model: record.body?.model ?? null,
    stream: record.body?.stream ?? null,
    maxTokens: record.body?.max_tokens ?? record.body?.max_completion_tokens ?? record.body?.max_output_tokens ?? null,
    streamOptions: record.body?.stream_options ?? null,
  }
}
