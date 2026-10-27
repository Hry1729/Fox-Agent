// What the configured provider actually answers, per route — no credential used.
//
// Purpose: decide whether the recorded SDK failure body
//   {"message":"model route not found","type":"not_found_error","code":"5"}
// can be produced by the configured base URL at all. Every request here is
// public, carries either no credential or an obviously fake one, and contains
// only the fixed probe text.
//
// Usage: node probe-vendor-routes.mjs [origin]   (default https://token.sensenova.cn)
const origin = (process.argv[2] ?? 'https://token.sensenova.cn').replace(/\/$/, '')
const RECORDED_BODY = '{"message":"model route not found","type":"not_found_error","code":"5"}'
const BODY = JSON.stringify({ model: 'sensenova-6.8-flash-lite', messages: [{ role: 'user', content: 'route probe' }], stream: false, max_tokens: 8 })

const cases = [
  { label: 'documented chat route, no credential', url: `${origin}/v1/chat/completions`, method: 'POST' },
  { label: 'documented chat route, fake credential', url: `${origin}/v1/chat/completions`, method: 'POST', auth: 'Bearer diagnostics-not-a-credential' },
  { label: 'models list, fake credential', url: `${origin}/v1/models`, method: 'GET', auth: 'Bearer diagnostics-not-a-credential' },
  { label: 'responses route, no credential', url: `${origin}/v1/responses`, method: 'POST' },
  { label: 'path without /v1', url: `${origin}/chat/completions`, method: 'POST' },
  { label: 'double-appended /v1', url: `${origin}/v1/v1/chat/completions`, method: 'POST' },
  { label: 'version suffix /v4', url: `${origin}/v4/chat/completions`, method: 'POST' },
  { label: 'trailing slash after /v1', url: `${origin}/v1/chat/completions/`, method: 'POST' },
  { label: 'second SenseNova host, /v1/chat/completions', url: 'https://api.sensenova.cn/v1/chat/completions', method: 'POST' },
  { label: 'second SenseNova host, documented llm route', url: 'https://api.sensenova.cn/v1/llm/chat-completions', method: 'POST' },
]

const rows = []
for (const item of cases) {
  const init = { method: item.method, signal: AbortSignal.timeout(20_000) }
  if (item.method === 'POST') {
    init.headers = { 'content-type': 'application/json' }
    init.body = BODY
  }
  if (item.auth) init.headers = { ...(init.headers ?? {}), authorization: item.auth }
  try {
    const response = await fetch(item.url, init)
    const text = (await response.text()).replace(/\s+/g, ' ').trim()
    rows.push({ label: item.label, status: response.status, body: text.length > 200 ? `${text.slice(0, 200)}…` : text })
  } catch (error) {
    rows.push({ label: item.label, status: 'network_error', body: String(error?.message ?? error) })
  }
}

for (const row of rows) console.log(`${String(row.status).padEnd(5)} ${row.label.padEnd(44)} ${row.body}`)
const offline = rows.every(row => row.status === 'network_error')
if (offline) {
  console.log('SKIP  no network reachability; the route corpus was not collected')
  process.exitCode = 0
} else {
  const recordedSeen = rows.some(row => row.body.includes('model route not found'))
  console.log('')
  console.log(`recorded SDK failure body: ${RECORDED_BODY}`)
  console.log(recordedSeen
    ? 'MATCH  a probe reproduced the recorded body above; see which route did'
    : 'MISMATCH  none of the probed routes answer with the recorded body; the failing request was not this route/host')
}
