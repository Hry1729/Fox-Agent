import { createServer } from 'node:http'
import { pathToFileURL } from 'node:url'

// Each deployment serves exactly one registered platform application.
export function oauthBroker({ clientId, clientSecret, issuerTokenUrl, redirectUri, method = 'client_secret_post' }, fetchFn = fetch) {
  if (!clientId || !clientSecret || !redirectUri || new URL(issuerTokenUrl).protocol !== 'https:'
    || !['client_secret_basic', 'client_secret_post'].includes(method)) throw new Error('平台授权服务配置无效')
  return createServer(async (req, res) => {
    const reply = (status, body) => {
      res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' })
      res.end(JSON.stringify(body))
    }
    if (req.method !== 'POST' || req.url !== '/oauth/token') return reply(404, { error: 'not_found' })
    try {
      const parts = []
      let bytes = 0
      for await (const part of req) {
        bytes += part.length
        if (bytes > 16384) return reply(413, { error: 'invalid_request' })
        parts.push(part)
      }
      const data = new URLSearchParams(Buffer.concat(parts).toString())
      if (data.get('client_id') !== clientId) return reply(400, { error: 'invalid_client' })
      const grant = data.get('grant_type')
      const allowed = new URLSearchParams({ grant_type: grant, client_id: clientId })
      if (grant === 'authorization_code' && data.get('redirect_uri') === redirectUri && data.get('code')
        && /^[A-Za-z0-9._~-]{43,128}$/.test(data.get('code_verifier') ?? '')) {
        for (const field of ['code', 'code_verifier', 'redirect_uri']) allowed.set(field, data.get(field))
      } else if (grant === 'refresh_token' && data.get('refresh_token')) {
        allowed.set('refresh_token', data.get('refresh_token'))
      } else return reply(400, { error: 'invalid_grant' })
      if (data.has('resource')) allowed.set('resource', data.get('resource'))
      const headers = { 'Content-Type': 'application/x-www-form-urlencoded', Accept: 'application/json' }
      if (method === 'client_secret_basic') {
        headers.Authorization = 'Basic ' + Buffer.from(encodeURIComponent(clientId) + ':' + encodeURIComponent(clientSecret)).toString('base64')
        allowed.delete('client_id')
      } else allowed.set('client_secret', clientSecret)
      const result = await fetchFn(issuerTokenUrl, { method: 'POST', headers, body: allowed, redirect: 'error', signal: AbortSignal.timeout(15000) })
      const token = await result.json()
      if (!result.ok || !token.access_token) return reply(400, { error: 'invalid_grant' })
      return reply(200, { access_token: token.access_token, refresh_token: token.refresh_token,
        token_type: token.token_type ?? 'Bearer', scope: token.scope, expires_in: token.expires_in })
    } catch { return reply(400, { error: 'invalid_request' }) }
  })
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  oauthBroker({ clientId: process.env.OAUTH_CLIENT_ID, clientSecret: process.env.OAUTH_CLIENT_SECRET,
    redirectUri: process.env.OAUTH_REDIRECT_URI, issuerTokenUrl: process.env.OAUTH_ISSUER_TOKEN_URL,
    method: process.env.OAUTH_TOKEN_AUTH_METHOD }).listen(Number(process.env.PORT ?? 8787), process.env.HOST ?? '127.0.0.1')
}
