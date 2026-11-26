import { createServer } from 'node:http'
import { pathToFileURL } from 'node:url'

const TOKEN_ENDPOINT = 'https://open.feishu.cn/open-apis/authen/v2/oauth/token'

export function feishuBroker({ clientId, clientSecret, redirectUri }, fetchFn = fetch) {
  if (!clientId || !clientSecret || !redirectUri) throw new Error('缺少飞书应用 ID、密钥或回调地址')
  return createServer(async (req, res) => {
    const reply = (status, value) => {
      res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' })
      res.end(JSON.stringify(value))
    }
    if (req.method !== 'POST' || req.url !== '/oauth/feishu/token') return reply(404, { error: 'not_found' })
    try {
      const chunks = []
      let size = 0
      for await (const chunk of req) {
        size += chunk.length
        if (size > 16384) return reply(413, { error: 'invalid_request' })
        chunks.push(chunk)
      }
      const input = JSON.parse(Buffer.concat(chunks).toString('utf8'))
      if (input?.client_id !== clientId) return reply(400, { error: 'invalid_client' })
      const data = { client_id: clientId, client_secret: clientSecret, grant_type: input.grant_type }
      if (data.grant_type === 'authorization_code' && input.redirect_uri === redirectUri && typeof input.code === 'string'
        && typeof input.code_verifier === 'string' && /^[A-Za-z0-9._~-]{43,128}$/.test(input.code_verifier)) {
        Object.assign(data, { code: input.code, redirect_uri: redirectUri, code_verifier: input.code_verifier })
      } else if (data.grant_type === 'refresh_token' && typeof input.refresh_token === 'string' && input.refresh_token) {
        data.refresh_token = input.refresh_token
      } else return reply(400, { error: 'invalid_grant' })
      const response = await fetchFn(TOKEN_ENDPOINT, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(15000),
        headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(data) })
      const token = await response.json()
      if (!response.ok || !token.access_token) return reply(400, { error: 'invalid_grant' })
      return reply(200, { access_token: token.access_token, refresh_token: token.refresh_token,
        expires_in: token.expires_in, token_type: token.token_type ?? 'Bearer' })
    } catch {
      return reply(400, { error: 'token_exchange_failed' })
    }
  })
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  feishuBroker({ clientId: process.env.FEISHU_APP_ID, clientSecret: process.env.FEISHU_APP_SECRET,
    redirectUri: process.env.FEISHU_REDIRECT_URI }).listen(Number(process.env.PORT ?? 8787), process.env.HOST ?? '127.0.0.1')
}
