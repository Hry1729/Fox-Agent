// Attach to an isolated, production-asset Tauri WebView2. No cloud requests.
// Start Fox with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=PORT.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { writeFile } from 'node:fs/promises'
const port = process.env.CDP_PORT ?? '9397'
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json()
const target = targets.find(t => t.type === 'page' && /^https?:\/\/tauri\.localhost/.test(t.url))
assert.ok(target, 'requires the real production-asset Tauri page')
const sockets = [], logs = []
async function connect(target) {
const ws = new WebSocket(target.webSocketDebuggerUrl)
sockets.push(ws)
await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }) })
let sequence = 0
const pending = new Map(), contexts = new Map()
ws.addEventListener('message', ({ data }) => {
  const m = JSON.parse(data)
  if (m.id) {
    const p = pending.get(m.id)
    if (p) { pending.delete(m.id); clearTimeout(p.timer); m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result) }
  }
  if (m.method === 'Runtime.executionContextCreated') contexts.set(m.params.context.id, m.params.context)
  if (m.method === 'Runtime.executionContextDestroyed') contexts.delete(m.params.executionContextId)
  if (m.method === 'Log.entryAdded') logs.push(m.params.entry)
})
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const id = ++sequence
  const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)) }, 15000)
  pending.set(id, { resolve, reject, timer }); ws.send(JSON.stringify({ id, method, params }))
})
const evaluate = async (expression, contextId) => {
  const r = await send('Runtime.evaluate', { expression, ...(contextId ? { contextId } : {}), returnByValue: true, awaitPromise: true })
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails))
  return r.result?.value
}
await send('Runtime.enable'); await send('Page.enable'); await send('Log.enable')
return { send, evaluate, contexts }
}
const { send, evaluate } = await connect(target)
let previewId
const requests = []
const server = createServer((req, res) => { requests.push(req.url); res.writeHead(200, { 'Access-Control-Allow-Origin': '*' }); res.end('unexpected egress') })
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
try {
  const app = await evaluate(`({url:location.href, text:document.body.innerText.slice(0,400), ipc:typeof window.__TAURI_INTERNALS__?.invoke, roots:document.querySelector('#root')?.children.length})`)
  assert.ok(app.roots > 0, 'React application rendered')
  assert.equal(app.ipc, 'function')
  const sink = `http://127.0.0.1:${server.address().port}`
  const fixture = `<!doctype html><meta charset=utf-8><button id=legit onclick="this.textContent='交互成功'">运行</button><script>window.previewReady=true</script>`
  previewId = await evaluate(`window.__TAURI_INTERNALS__.invoke('html_preview_create',{content:${JSON.stringify(fixture)}})`)
  assert.match(previewId, /^[0-9a-f-]{36}$/)
  await evaluate(`(()=>{const frame=document.createElement('iframe');frame.id='fox-preview-security-smoke';frame.sandbox='allow-scripts';frame.referrerPolicy='no-referrer';frame.src=window.__TAURI_INTERNALS__.convertFileSrc(${JSON.stringify(previewId)},'fox-preview')+'/frame';document.body.append(frame)})()`)
  let contentContext, frameTree, preview
  for (let i = 0; i < 40; i++) {
    // Cross-origin preview frames run in a separate WebView2 target (OOPIF).
    const frames = await (await fetch(`http://127.0.0.1:${port}/json`)).json()
    const previewTarget = frames.find(t => t.type === 'iframe' && t.url.includes(`/${previewId}/`))
    if (!preview && previewTarget) preview = await connect(previewTarget)
    if (preview) {
      frameTree = (await preview.send('Page.getFrameTree')).frameTree
      const inner = frameTree.frame.url.endsWith('/content') ? frameTree : frameTree.childFrames?.find(f => f.frame.url.endsWith(`/${previewId}/content`))
      contentContext = inner && [...preview.contexts.values()].find(c => c.auxData?.frameId === inner.frame.id && c.auxData?.isDefault)?.id
    }
    if (contentContext) break
    await delay(100)
  }
  assert.ok(contentContext, `preview content frame exists: ${JSON.stringify({ frameTree, logs })}`)
  const interaction = await preview.evaluate(`(()=>{document.querySelector('#legit').click();return {ready:window.previewReady,text:document.querySelector('#legit').textContent}})()`, contentContext)
  assert.deepEqual(interaction, { ready: true, text: '交互成功' })
  const isolation = await preview.evaluate(`(async()=>{
    let parentDenied=false,storageDenied=false,fetchDenied=false;
    try { void top.document.body } catch {parentDenied=true}
    try { localStorage.setItem('preview-canary','x') } catch {storageDenied=true}
    try { await fetch(${JSON.stringify(sink + '/fetch')}) } catch {fetchDenied=true}
    const image=new Image();image.src=${JSON.stringify(sink + '/image')};document.body.append(image);
    const child=document.createElement('iframe');child.src=${JSON.stringify(sink + '/iframe')};document.body.append(child);
    const form=document.createElement('form');form.action=${JSON.stringify(sink + '/form')};document.body.append(form);form.submit();
    let ipcResult='unavailable';
    if (window.__TAURI_INTERNALS__) ipcResult=await Promise.race([
      window.__TAURI_INTERNALS__.invoke('html_preview_create',{content:'smoke IPC canary'}).then(()=> 'ALLOWED',()=> 'denied'),
      new Promise(resolve=>setTimeout(()=>resolve('blocked-timeout'),1000))
    ]);
    return {parentDenied,storageDenied,fetchDenied,ipcType:typeof window.__TAURI_INTERNALS__,ipcResult,origin:window.origin}
  })()`, contentContext)
  assert.equal(isolation.parentDenied, true); assert.equal(isolation.storageDenied, true); assert.equal(isolation.fetchDenied, true)
  assert.notEqual(isolation.ipcResult, 'ALLOWED'); assert.equal(isolation.origin, 'null')
  // Its own content CSP alone does not prevent navigation; the outer frame
  // must enforce this boundary even though Fox's browser panel allows HTTPS.
  await preview.evaluate(`location.href=${JSON.stringify(sink + '/navigation')}`, contentContext)
  await delay(700)
  assert.deepEqual(requests, [], 'untrusted HTML made no external requests')
  const mainUrl = await evaluate('location.href')
  assert.equal(mainUrl, app.url)
  const report = { passed: true, target: target.url, app, interaction, isolation, externalRequests: requests, mainUrl, policyMessages: logs.filter(l => /Content Security Policy|sandbox|frame|script/i.test(l.text)).map(l => l.text) }
  if (process.env.FOX_PREVIEW_SMOKE_REPORT) await writeFile(process.env.FOX_PREVIEW_SMOKE_REPORT, JSON.stringify(report, null, 2))
  console.log(JSON.stringify(report, null, 2))
} catch (error) {
  console.error(JSON.stringify({ error: error.message, requests, policyMessages: logs.map(l => l.text) }, null, 2))
  throw error
} finally {
  if (previewId) await evaluate(`document.querySelector('#fox-preview-security-smoke')?.remove();window.__TAURI_INTERNALS__.invoke('html_preview_release',{id:${JSON.stringify(previewId)}})`).catch(() => {})
  for (const ws of sockets) ws.close()
  server.closeAllConnections()
  await new Promise(resolve => server.close(resolve))
}
