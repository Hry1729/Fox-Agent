// Drive the REAL desktop app through its own WebView over CDP.
//
// Why CDP: the app's WebView2 drops posted window messages (PostMessage), so a
// background click/type looks successful but never reaches the page. With
// --remote-debugging-port the same window exposes a devtools target; we set the
// textarea value through the prototype setter (so React sees a real input
// event), read it back, then click the composer's own send button. Nothing is
// force-enabled and no submission path is bypassed.
//
// Usage (app must be running with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS set):
//   node drive.mjs --conv "<conversation title fragment>" --ask "<question>"
//   node drive.mjs --conv "<fragment>" --ask "<question>" --wait 30000 --json
//
// Exit codes: 0 sent and settled, 1 the page could not be driven, 2 no CDP page.
const args = process.argv.slice(2)
const value = (name, fallback) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : fallback }
const conv = value('--conv', null)
const ask = value('--ask', null)
const waitMs = Number(value('--wait', '26000'))
const port = Number(value('--port', '9222'))
const json = args.includes('--json')

const fail = (message, extra = {}) => { console.error(JSON.stringify({ ok: false, message, ...extra }, null, 1)); process.exit(1) }

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json().catch(() => null)
if (!targets) { console.error(`no devtools endpoint on ${port}; start the app with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=${port}`); process.exit(2) }
const page = targets.find((target) => target.type === 'page' && target.url.includes('1421'))
if (!page) { console.error('no app page among devtools targets'); process.exit(2) }

const socket = new WebSocket(page.webSocketDebuggerUrl)
const pending = new Map()
let nextId = 0
const pageErrors = []
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data)
  if (message.id && pending.has(message.id)) { pending.get(message.id)(message); pending.delete(message.id) }
  else if (message.method === 'Runtime.exceptionThrown') pageErrors.push(message.params.exceptionDetails?.exception?.description ?? message.params.exceptionDetails?.text)
})
await new Promise((resolve, reject) => { socket.addEventListener('open', resolve); socket.addEventListener('error', reject) })
const send = (method, params = {}) => new Promise((resolve) => { const id = ++nextId; pending.set(id, resolve); socket.send(JSON.stringify({ id, method, params })) })
const evaluate = async (expression) => {
  const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })
  if (response.result?.exceptionDetails) return { __error: response.result.exceptionDetails.exception?.description ?? 'evaluate failed' }
  return response.result?.result?.value
}
await send('Runtime.enable')

if (conv) {
  const opened = await evaluate(`(() => { const t=[...document.querySelectorAll('button,a,li')].find(e=>(e.textContent||'').includes(${JSON.stringify(conv)})); if(!t) return {ok:false}; t.click(); return {ok:true,text:(t.textContent||'').slice(0,50)} })()`)
  if (!opened?.ok) fail('conversation not found in the sidebar', { conversation: conv })
  await new Promise((resolve) => setTimeout(resolve, 2500))
}
// A resend confirmation banner also carries a 发送 button; dismiss it first so
// the click below can only reach the composer.
await evaluate(`(() => { const b=[...document.querySelectorAll('button')].find(x=>(x.textContent||'').trim()==='取消'); if (b) b.click(); return true })()`)
await new Promise((resolve) => setTimeout(resolve, 700))

const typed = await evaluate(`(() => {
  const areas=[...document.querySelectorAll('textarea')].filter(e=>e.offsetParent!==null); const el=areas.pop()
  if(!el) return {ok:false, reason:'no visible textarea'}
  const set=Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value').set
  el.focus(); set.call(el, ${JSON.stringify(ask ?? '')}); el.dispatchEvent(new Event('input',{bubbles:true}))
  return {ok:true, value: el.value, disabled: el.disabled}
})()`)
if (!typed?.ok) fail('composer not writable', typed)
if (typed.value !== (ask ?? '')) fail('the written text did not survive read-back', { value: typed.value })
await new Promise((resolve) => setTimeout(resolve, 700))

const clicked = await evaluate(`(() => {
  const areas=[...document.querySelectorAll('textarea')].filter(e=>e.offsetParent!==null); const el=areas.pop()
  if(!el) return {ok:false, reason:'no textarea'}
  let box=el.parentElement; for(let i=0;i<4 && box;i++) box=box.parentElement
  const buttons=box?[...box.querySelectorAll('button')]:[]
  const usable=buttons.filter(b=>!b.disabled && !/重新发送/.test(b.parentElement?.textContent||''))
  const target=usable.pop()
  if(!target) return {ok:false, reason:'no usable send button', labels: buttons.map(b=>b.getAttribute('aria-label'))}
  target.click()
  return {ok:true, label: target.getAttribute('aria-label')}
})()`)
if (!clicked?.ok) fail('send button unusable', clicked)

await new Promise((resolve) => setTimeout(resolve, waitMs))
const settled = await evaluate(`(() => {
  const areas=[...document.querySelectorAll('textarea')].filter(e=>e.offsetParent!==null); const el=areas.pop()
  return { inputValue: el?el.value:null, tail: document.body.innerText.slice(-800) }
})()`)

const result = { ok: true, conversation: conv, asked: ask, clicked: clicked.label ?? null, settled, pageErrors }
if (json) console.log(JSON.stringify(result, null, 1))
else console.log(settled?.tail ?? '')
socket.close()
