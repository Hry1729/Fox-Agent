import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/titlebar-menu-20261017/browser')
const port = 1472, cdpPort = 9378
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
mkdirSync(output, { recursive: true })
let server, browser, client
let diagnostics = ''
const results = []
// node.exe is the direct child, but vite's logger keeps the tree alive; take it down whole.
const stopServer = () => { if (!server) return; if (process.platform === 'win32') spawn('taskkill', ['/pid', String(server.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true }); else server.kill() }
// A failed assertion still has to free the ports for the next run.
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => { stopServer(); process.exit(130) })
async function connect() {
  let target
  for (let index = 0; index < 80; index++) {
    try { target = (await fetch(`http://127.0.0.1:${cdpPort}/json`).then((response) => response.json())).find((item) => item.type === 'page'); if (target) break } catch {}
    await pause(250)
  }
  if (!target) throw new Error(`独立测试浏览器未启动：${diagnostics}`)
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }) })
  let serial = 0
  const pending = new Map(), errors = []
  socket.addEventListener('message', ({ data }) => {
    const response = JSON.parse(data), item = pending.get(response.id)
    if (response.method === 'Runtime.exceptionThrown') errors.push(response.params.exceptionDetails)
    if (item) { clearTimeout(item.timer); pending.delete(response.id); response.error ? item.reject(new Error(JSON.stringify(response.error))) : item.resolve(response.result) }
  })
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++serial
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP 超时：${method}`)) }, 15000)
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }))
  })
  const evaluate = async (expression) => { const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text); return result.result.value }
  return { send, evaluate, errors, close: () => socket.close() }
}
async function waitFor(expression, description) {
  for (let index = 0; index < 200; index++) { if (await client.evaluate(expression)) return; await pause(120) }
  throw new Error(`${description} 未就绪；${JSON.stringify(client.errors)}；${await client.evaluate('document.body.innerText.slice(0,1500)')}；${await client.evaluate('document.body.innerHTML.slice(0,300)')}`)
}
const screenshot = async (name) => {
  await pause(450)
  const shot = await client.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
  writeFileSync(join(output, `${name}.png`), Buffer.from(shot.data, 'base64'))
}
const escape = async () => {
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await pause(200)
}
const click = async (selector) => {
  const point = await client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});e.scrollIntoView({block:'center'});const r=e.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`)
  await client.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point })
  await client.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 })
  await client.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 })
}

try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  const profile = mkdtempSync(join(output, 'chrome-profile-'))
  browser = spawn(process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true })
  browser.stderr.on('data', (data) => { diagnostics = (diagnostics + data).slice(-4000) })
  client = await connect()
  await client.send('Page.enable'); await client.send('Runtime.enable')
  let ready = false
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/`)).ok) { ready = true; break } } catch {} await pause(150) }
  if (!ready) throw new Error('工作台服务未启动')

  await client.send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false })
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/` })
  await waitFor(`document.readyState === 'complete'`, '首次加载')
  await client.evaluate(`(()=>{localStorage.setItem('fox.onboarding.status','complete')})()`)
  await client.send('Page.reload')
  await waitFor(`!!document.querySelector('.fox-window-titlebar .fox-titlebar-menu-trigger')`, '标题栏菜单图标')
  const titlebar = await client.evaluate(`(()=>{const bar=document.querySelector('.fox-window-titlebar');const menu=bar.querySelector('.fox-titlebar-menu-trigger');const toggle=bar.querySelector('.fox-titlebar-sidebar-controls button[aria-pressed]');return{text:bar.innerText.replace(/\\s+/g,' ').trim(),menuX:menu.getBoundingClientRect().x,toggleX:toggle.getBoundingClientRect().x}})()`)
  if (!/^Fox/.test(titlebar.text) || /文件|编辑|视图|帮助/.test(titlebar.text)) throw Error('标题栏仍显示文字菜单 ' + titlebar.text)
  if (!(titlebar.menuX < titlebar.toggleX)) throw Error('菜单图标不在侧栏折叠图标左侧 ' + JSON.stringify(titlebar))
  await screenshot('titlebar-closed')

  await click('.fox-titlebar-menu-trigger')
  await waitFor(`[role="menu"] && [...document.querySelectorAll('[role="menu"] [data-slot="dropdown-menu-label"]')].length===4`, '单图标菜单')
  const menu = await client.evaluate(`[...document.querySelectorAll('[role="menu"] [data-slot="dropdown-menu-label"]')].map((e)=>e.textContent.trim()).join('/')`)
  if (menu !== '文件/编辑/视图/帮助') throw Error('菜单分组不完整 ' + menu)
  await screenshot('titlebar-menu-open')
  await escape()

  const measureRail = () => client.evaluate(`(()=>{const side=document.querySelector('.fox-sidebar');const rail=document.querySelector('.fox-sidebar-collapsed-rail');const toggle=document.querySelector('.fox-titlebar-sidebar-controls button[aria-pressed]').getBoundingClientRect();const first=rail?.querySelector('button');const dividers=rail?rail.querySelectorAll('.fox-sidebar-rail-divider').length:0;return{width:side.getBoundingClientRect().width,collapsed:side.classList.contains('is-collapsed'),railVisible:!!rail&&getComputedStyle(rail).display!=='none',railTop:first?first.getBoundingClientRect().y:null,toggleBottom:toggle.bottom,dividers}})()`)
  const before = await measureRail()
  if (before.collapsed || Math.abs(before.width - Number(await client.evaluate(`getComputedStyle(document.querySelector('.fox-shell')).getPropertyValue('--fox-sidebar-width').trim().replace('px','')`))) > 1) throw Error('展开态宽度与本体验宽度不一致 ' + JSON.stringify(before))

  // Sample inside one paced loop: a CDP round trip between samples takes longer
  // than the 200ms transition, so the browser records 12 frames per 16ms.
  const samples = await client.evaluate(`new Promise((resolve)=>{const out=[],start=performance.now();const tick=()=>{out.push(document.querySelector('.fox-sidebar').getBoundingClientRect().width);if(performance.now()-start<420)requestAnimationFrame(tick);else resolve(out)};document.querySelector('.fox-titlebar-sidebar-controls button[aria-pressed]').click();requestAnimationFrame(tick)})`)
  const monotonic = samples.every((width, index) => index === 0 || width <= samples[index - 1] + 0.5)
  await pause(350)
  const collapsed = await measureRail()
  if (!collapsed.collapsed || Math.abs(collapsed.width - 64) > 1) throw Error('折叠后宽度不是 64 ' + JSON.stringify(collapsed))
  if (!collapsed.railVisible || collapsed.dividers !== 2) throw Error('折叠态缺少文件/对话图标或分割线 ' + JSON.stringify(collapsed))
  if (!(collapsed.railTop < collapsed.toggleBottom + 8)) throw Error('折叠态文件/对话图标没有上移到标题栏附近 ' + JSON.stringify(collapsed))
  if (samples.filter((width) => width > 64.5 && width < before.width - 0.5).length < 4) throw Error('折叠宽度不是连续动画，采样 ' + JSON.stringify(samples))
  if (!monotonic) throw Error('折叠动画中途回弹 ' + JSON.stringify(samples))
  results.push({ check: 'single icon menu left of toggle; collapse animates continuously and shows 文件/对话 above two dividers', before, collapsed, samples })
  await screenshot('titlebar-collapsed')

  // The collapsed conversation tile sits just above the footer: shrink the window
  // so a full list would overrun it, then hover the tile with real pointer moves.
  await client.send('Emulation.setDeviceMetricsOverride', { width: 1000, height: 620, deviceScaleFactor: 1, mobile: false })
  await pause(400)
  await client.evaluate(`document.querySelector('[aria-label^="展开对话列表"]').scrollIntoView({block:'center'})`)
  await client.evaluate('document.querySelector(\'.fox-sidebar-collapsed-entry [aria-label^="展开对话列表"]\').blur()')
  await client.evaluate(`(()=>{const r=document.querySelector('[aria-label^="展开对话列表"]').getBoundingClientRect();window.__tile={x:r.x+r.width/2,y:r.y+r.height/2}})()`)
  const tile = await client.evaluate('window.__tile')
  await client.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: tile.x - 120, y: tile.y })
  await pause(80)
  await client.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: tile.x, y: tile.y })
  await pause(350)
  const popover = await client.evaluate(`(()=>{const box=document.querySelector('.fox-sidebar-collapsed-popover');if(!box)return null;const r=box.getBoundingClientRect();return{top:r.top,bottom:r.bottom,height:r.height,viewport:innerHeight,scrollable:box.scrollHeight>box.clientHeight}})()`)
  if (!popover) throw Error('折叠态对话列表没有打开')
  if (popover.bottom > popover.viewport + 1) throw Error('折叠态对话列表越过窗口底部 ' + JSON.stringify(popover))
  await screenshot('collapsed-conversation-list-short-window')
  results.push({ check: 'collapsed conversation list stays inside a short window', popover })

  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: true, results }, null, 2))
  process.stdout.write(JSON.stringify({ passed: true, output, results }))
} catch (error) {
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: false, error: String(error), diagnostics, results }, null, 2))
  process.stderr.write(`${error.stack ?? error}\n`)
  process.exitCode = 1
} finally { client?.close(); browser?.kill(); stopServer() }
