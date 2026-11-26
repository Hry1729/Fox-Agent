import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { once } from 'node:events'
import { createServer } from 'node:http'
import test from 'node:test'
import { authorize } from '../src/connector-auth.mjs'
import { feishuBroker } from '../../connector-auth/feishu-broker.mjs'
import { oauthBroker } from '../../connector-auth/oauth-broker.mjs'

async function serve(t, server) {
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  t.after(() => { server.closeAllConnections(); server.close() })
  return 'http://127.0.0.1:' + server.address().port
}

async function callback(url, code, state) {
  const target = new URL(url.searchParams.get('redirect_uri'))
  target.search = new URLSearchParams({ state, code })
  return fetch(target)
}

test('MCP SDK discovers the issuer, registers Fox, exchanges PKCE and refreshes the grant', { timeout: 10000 }, async t => {
  let challenge
  let registration
  let verification
  const issuer = await serve(t, createServer(async (req, res) => {
    const url = new URL(req.url, issuer)
    const answer = data => { res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify(data)) }
    if (url.pathname.includes('oauth-protected-resource')) return answer({ resource: issuer + '/mcp', authorization_servers: [issuer] })
    if (url.pathname.includes('oauth-authorization-server')) return answer({ issuer, authorization_endpoint: issuer + '/authorize',
      token_endpoint: issuer + '/token', registration_endpoint: issuer + '/register', response_types_supported: ['code'],
      code_challenge_methods_supported: ['S256'], token_endpoint_auth_methods_supported: ['none'] })
    const parts = []
    for await (const part of req) parts.push(part)
    const body = Buffer.concat(parts).toString()
    if (url.pathname === '/register') {
      registration = JSON.parse(body)
      return answer({ ...registration, client_id: 'fox-test' })
    }
    if (url.pathname === '/token') {
      const values = new URLSearchParams(body)
      if (values.get('grant_type') === 'refresh_token' && values.get('refresh_token') === 'first-refresh')
        return answer({ access_token: 'second', token_type: 'Bearer', expires_in: 3600 })
      verification = createHash('sha256').update(values.get('code_verifier') ?? '').digest('base64url') === challenge
        && values.get('code') === 'good-code' && registration.redirect_uris.includes(values.get('redirect_uri'))
      return answer({ access_token: 'first', refresh_token: 'first-refresh', token_type: 'Bearer', expires_in: 300 })
    }
    res.writeHead(404).end()
  }))
  let delivery
  const request = { endpoint: issuer + '/mcp', interactive: true }
  const session = await authorize(request, target => {
    const url = new URL(target)
    challenge = url.searchParams.get('code_challenge')
    delivery = (async () => {
      const rejected = await callback(url, 'wrong', 'invalid')
      assert.equal(rejected.status, 400)
      assert.equal((await callback(url, 'good-code', url.searchParams.get('state'))).status, 200)
    })()
  })
  await delivery
  assert.ok(verification)
  assert.equal(session.tokens.access_token, 'first')
  const renewed = await authorize({ ...request, session, interactive: false }, assert.fail)
  assert.equal(renewed.tokens.access_token, 'second')
  assert.equal(renewed.tokens.refresh_token, 'first-refresh')
})

test('Feishu broker keeps the app secret on the server and rejects a foreign redirect', async t => {
  let exchanged
  const redirect = 'http://127.0.0.1:42181/oauth/callback'
  const base = await serve(t, feishuBroker({ clientId: 'fox-app', clientSecret: 'fixture-only', redirectUri: redirect },
    async (url, init) => {
      assert.equal(url, 'https://open.feishu.cn/open-apis/authen/v2/oauth/token')
      exchanged = JSON.parse(init.body)
      return Response.json({ access_token: 'test-access', refresh_token: 'test-refresh', expires_in: 3600, debug: 'private' })
    }))
  const post = body => fetch(base + '/oauth/feishu/token', { method: 'POST', body: JSON.stringify(body) })
  const body = { grant_type: 'authorization_code', client_id: 'fox-app', code: 'test', code_verifier: 'A'.repeat(43) }
  assert.equal((await post({ ...body, redirect_uri: 'https://other.example' })).status, 400)
  assert.equal(exchanged, undefined)
  const tokens = await (await post({ ...body, redirect_uri: redirect })).json()
  assert.equal(exchanged.client_secret, 'fixture-only')
  assert.equal(tokens.access_token, 'test-access')
  assert.equal(tokens.debug, undefined)
  const renewed = await authorize({ interactive: false, registration: 'FEISHU', tokenUrl: base + '/oauth/feishu/token',
    clientId: 'fox-app', session: { tokens } }, assert.fail)
  assert.equal(exchanged.grant_type, 'refresh_token')
  assert.equal(renewed.tokens.refresh_token, 'test-refresh')
})

test('registered OAuth client refreshes through its broker at the configured token endpoint', async t => {
  const endpoint = 'https://platform.example/mcp'
  const issuer = 'https://issuer.example'
  const redirectUri = 'http://127.0.0.1:42181/oauth/callback'
  let upstream
  const broker = await serve(t, oauthBroker({ clientId: 'fox-app', clientSecret: 'fixture', redirectUri,
    issuerTokenUrl: issuer + '/token', method: 'client_secret_basic' }, async (url, init) => {
    upstream = { url, headers: init.headers, body: init.body }
    return Response.json({ access_token: 'new-token', token_type: 'Bearer' })
  }))
  const session = await authorize({ endpoint, clientId: 'fox-app', interactive: false,
    issuerTokenUrl: issuer + '/token', tokenUrl: broker + '/oauth/token',
    session: { redirectUrl: redirectUri, tokens: { refresh_token: 'old-refresh' } } }, assert.fail, (url, init) => {
    if (String(url).includes('oauth-protected-resource')) return Response.json({ resource: endpoint, authorization_servers: [issuer] })
    if (String(url).includes('oauth-authorization-server')) return Response.json({ issuer, authorization_endpoint: issuer + '/authorize',
      token_endpoint: issuer + '/token', response_types_supported: ['code'], token_endpoint_auth_methods_supported: ['client_secret_basic'] })
    assert.equal(String(url), broker + '/oauth/token')
    return fetch(url, init)
  })
  assert.equal(session.tokens.access_token, 'new-token')
  assert.equal(upstream.body.get('refresh_token'), 'old-refresh')
  assert.equal(upstream.headers.Authorization, 'Basic ' + Buffer.from('fox-app:fixture').toString('base64'))
})
