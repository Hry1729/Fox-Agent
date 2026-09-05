// Drives the REAL Tauri WebView2 over CDP (attach to an already-launched window
// started with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=PORT).
// On the real App page it opens the RealProfilingController, starts collection,
// drives composer input (Workbench commits), stops, and reads the scenario summary.
import { spawn } from 'node:child_process'

const PORT = process.env.CDP_PORT || '9335'
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function cdp(ws) {
  let id = 0
  const pending = new Map()
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data)
    if (m.id && pending.has(m.id)) { const { resolve, reject } = pending.get(m.id); pending.delete(m.id); m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result) }
  })
  const send = (method, params = {}) => new Promise((resolve, reject) => { const mid = ++id; pending.set(mid, { resolve, reject }); ws.send(JSON.stringify({ id: mid, method, params })) })
  return { send }
}

async function main() {
  // Find the WebView2 target already pointing at the app (not a devtools page).
  let target
  for (let i = 0; i < 40; i++) {
    const list = await fetch(`http://127.0.0.1:${PORT}/json`).then((r) => r.json()).catch(() => [])
    target = list.find((t) => t.type === 'page' && t.url.includes('127.0.0.1:1422'))
    if (target) break
    await sleep(500)
  }
  if (!target) throw new Error('WebView2 app target not found on CDP port ' + PORT)

  const ws = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((res, rej) => { ws.addEventListener('open', res, { once: true }); ws.addEventListener('error', () => rej(new Error('ws failed')), { once: true }) })
  const { send } = cdp(ws)
  await send('Runtime.enable'); await send('DOM.enable')
  const evalJs = async (expression) => (await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result?.value

  await sleep(3000) // let App + lazy controller mount
  // Fresh WebView2 may show onboarding (no Workbench composer); skip it.
  if (await evalJs(`localStorage.getItem('fox.onboarding.status') !== 'done'`)) {
    await evalJs(`localStorage.setItem('fox.onboarding.status','done')`)
    await send('Page.enable')
    await send('Page.reload', { ignoreCache: true })
    await sleep(5000)
  }
  const buttons = await evalJs(`[...document.querySelectorAll('button')].map(b=>b.textContent.trim()).filter(t=>/PERF|HARNESS/.test(t)).join(',')`)
  console.log('floating buttons:', buttons || '(none yet)')
  if (!/PERF/.test(buttons)) throw new Error('PERF controller not present in WebView2')

  // Open the real-app controller.
  await evalJs(`[...document.querySelectorAll('button')].find(b=>b.textContent.trim().startsWith('PERF'))?.click()`)
  await sleep(600)
  if (!(await evalJs(`document.body.innerText.includes('Fox 真实采集')`))) throw new Error('controller panel did not open in WebView2')

  // Start collection.
  await evalJs(`[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='开始采集')?.click()`)
  await sleep(300)
  if (!(await evalJs(`document.body.innerText.includes('采集中')`))) throw new Error('recording did not start in WebView2')
  console.log('collection started in WebView2')

  // Drive Workbench: real text input into the composer.
  const focused = await evalJs(`(()=>{const ta=document.querySelector('textarea'); if(!ta) return 'no-textarea'; ta.focus(); return 'focused'})()`)
  console.log('composer:', focused)
  for (let i = 0; i < 12; i++) {
    await send('Input.insertText', { text: `WebView2 真实采集烟测第 ${i} 段文本，触发 Workbench 渲染 ` })
    await sleep(180)
  }
  await evalJs(`window.dispatchEvent(new Event('resize'))`)
  await sleep(700)

  // Stop and end scenario.
  await evalJs(`[...document.querySelectorAll('button')].find(b=>b.textContent.trim().startsWith('停止'))?.click()`)
  await sleep(600)
  const summary = await evalJs(`(()=>{const t=document.body.innerText; const m=t.match(/上次：[^\\n]*/); return m?m[0]:'NO SUMMARY: '+t.slice(-200)})()`)
  console.log('WebView2 real-app scenario:', summary)
  if (/commits=0/.test(summary)) throw new Error('WebView2 scenario captured 0 commits')
  if (!/commits=\d+/.test(summary)) throw new Error('no scenario summary in WebView2')

  console.log('WEBVIEW2 PROFILE SMOKE OK')
  ws.close()
}

main().catch((e) => { console.error('WEBVIEW2 SMOKE FAILED:', e.message); process.exit(1) })
