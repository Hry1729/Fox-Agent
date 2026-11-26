import { createHash, randomBytes } from 'node:crypto'
import { createServer } from 'node:http'
import { once } from 'node:events'
import { pathToFileURL } from 'node:url'
import { auth, extractWWWAuthenticateParams } from '@modelcontextprotocol/sdk/client/auth.js'
import { SSEClientTransport } from '@modelcontextprotocol/sdk/client/sse.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'

export async function authorize(request, openBrowser, fetchFn = fetch) {
  const { endpoint, interactive, scope } = request
  const session = { ...request.session }
  const nonce = randomBytes(32).toString('hex')
  let verifier
  let callback
  let finish
  let redirectUrl = session.redirectUrl
  const received = new Promise(resolve => { finish = resolve })
  try {
    if (interactive) {
      callback = createServer((req, res) => {
        const url = new URL(req.url, 'http://127.0.0.1')
        if (req.method !== 'GET' || url.pathname !== '/oauth/callback' || url.searchParams.get('state') !== nonce) {
          res.writeHead(400).end()
          return
        }
        const code = url.searchParams.get('code')
        res.writeHead(200, { 'Content-Type': 'text/plain; charset=utf-8', 'Cache-Control': 'no-store' })
        res.end(code ? '请返回 Fox 查看连接结果。' : '授权已取消，可以返回 Fox。')
        finish(code)
      })
      callback.listen(request.port ?? 0, '127.0.0.1')
      await once(callback, 'listening')
      redirectUrl = 'http://127.0.0.1:' + callback.address().port + '/oauth/callback'
      if (!request.clientId && redirectUrl !== session.redirectUrl) delete session.client
      session.redirectUrl = redirectUrl
    }
    const provider = {
      redirectUrl,
      clientMetadataUrl: request.clientMetadataUrl,
      clientMetadata: {
        client_name: 'Fox', redirect_uris: [redirectUrl], grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'], token_endpoint_auth_method: 'none',
      },
      state: () => nonce,
      clientInformation: () => request.clientId ? { client_id: request.clientId } : session.client,
      saveClientInformation: client => { session.client = { client_id: client.client_id, ...(client.client_secret ? { client_secret: client.client_secret } : {}) } },
      tokens: () => interactive ? undefined : session.tokens,
      saveTokens: tokens => {
        session.tokens = { ...tokens, refresh_token: tokens.refresh_token ?? session.tokens?.refresh_token }
        session.expiresAt = tokens.expires_in ? Math.floor(Date.now() / 1000) + tokens.expires_in : 0
      },
      saveCodeVerifier: value => { verifier = value },
      codeVerifier: () => verifier,
      redirectToAuthorization: url => {
        if (!interactive) throw new Error('请在 Fox 中重新授权')
        openBrowser(url.href)
      },
    }
    if (request.registration === 'FEISHU') {
      const clientId = request.clientId
      const params = { client_id: clientId }
      if (interactive) {
        const proof = randomBytes(32).toString('base64url')
        const url = new URL('https://accounts.feishu.cn/open-apis/authen/v1/authorize')
        url.search = new URLSearchParams({
          client_id: clientId, redirect_uri: redirectUrl, response_type: 'code', state: nonce,
          scope: request.scope ?? 'offline_access search:docs:read docx:document',
          code_challenge_method: 'S256', code_challenge: createHash('sha256').update(proof).digest('base64url'),
        }).toString()
        openBrowser(url.href)
        const code = await received
        if (!code) throw new Error('用户取消授权')
        Object.assign(params, { grant_type: 'authorization_code', code, code_verifier: proof, redirect_uri: redirectUrl })
      } else {
        Object.assign(params, { grant_type: 'refresh_token', refresh_token: session.tokens.refresh_token })
      }
      const res = await fetchFn(request.tokenUrl, {
        method: 'POST', redirect: 'error', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(params),
      })
      if (!res.ok) throw new Error('飞书令牌交换失败')
      const tokens = await res.json()
      if (!tokens.access_token) throw new Error('飞书未返回访问令牌')
      provider.saveTokens({ ...tokens, refresh_token: tokens.refresh_token ?? session.tokens?.refresh_token })
      return session
    }
    const exchange = (input, init) => {
      const url = input instanceof Request ? input.url : String(input)
      if (request.issuerTokenUrl && url === request.issuerTokenUrl && init?.method?.toUpperCase() === 'POST') {
        return fetchFn(request.tokenUrl, { method: 'POST', redirect: 'error',
          headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: init.body })
      }
      return fetchFn(input, init)
    }
    if (interactive) {
      const probe = await fetchFn(endpoint, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(15000),
        headers: { 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream' },
        body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {
          protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'Fox', version: '1' },
        } }) })
      if (probe.status === 401 || probe.status === 403) {
        const challenge = extractWWWAuthenticateParams(probe)
        session.resourceMetadataUrl = challenge.resourceMetadataUrl?.href
        session.scope = challenge.scope
      }
      await probe.body?.cancel()
    }
    const options = { serverUrl: endpoint, resourceMetadataUrl: session.resourceMetadataUrl && new URL(session.resourceMetadataUrl),
      scope: scope ?? session.scope, fetchFn: exchange }
    if (await auth(provider, options) === 'REDIRECT') {
      const code = await received
      if (!code) throw new Error('用户已取消授权')
      await auth(provider, { ...options, authorizationCode: code })
    }
    if (!session.tokens?.access_token) throw new Error('平台未签发访问令牌')
    return session
  } finally {
    callback?.close()
    callback?.closeAllConnections()
  }
}

async function main() {
  const timeout = setTimeout(() => { process.exitCode = 1; process.exit(1) }, 180_000)
  try {
    const input = []
    for await (const chunk of process.stdin) input.push(chunk)
    const request = JSON.parse(Buffer.concat(input).toString('utf8'))
    const write = payload => process.stdout.write(JSON.stringify(payload) + '\n')
    const session = await authorize(request, url => write({ url }))
    write({ session })
  } catch (error) {
    // Vendor response bodies can contain credentials. Only return an OAuth error code.
    const code = /^[a-z_]{2,48}$/.test(error.error ?? '') ? error.error : 'authorization_failed'
    process.stdout.write(JSON.stringify({ error: code }) + '\n')
    process.exitCode = 1
  } finally {
    clearTimeout(timeout)
  }
}

async function bridgeYoudao() {
  const base = new URL('https://open.mail.163.com/api/ynote/mcp/sse')
  const local = new StdioServerTransport()
  const remote = new SSEClientTransport(base, {
    fetch: (input, init) => {
      const url = new URL(input instanceof Request ? input.url : input)
      if (url.origin !== base.origin) throw new Error('有道服务地址发生变化')
      return fetch(input, { ...init, redirect: 'error' })
    },
    requestInit: { headers: { 'x-api-key': process.env.YOUDAO_API_KEY } },
  })
  local.onmessage = message => remote.send(message).catch(() => process.exit(1))
  remote.onmessage = message => local.send(message).catch(() => process.exit(1))
  process.stdin.once('end', () => remote.close())
  await remote.start()
  await local.start()
}

if (process.versions.bun && import.meta.main || process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.includes('--youdao')) {
    try { await bridgeYoudao() } catch { process.exitCode = 1 }
  } else await main()
}
