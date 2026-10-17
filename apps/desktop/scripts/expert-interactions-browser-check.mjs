import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/expert-interactions-20261015')
const port = 1464, cdpPort = 9370
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
mkdirSync(output, { recursive: true })
let server, browser, client
let diagnostics = ''
const results = []
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
  throw new Error(`${description} 未就绪；${JSON.stringify(client.errors)}；${await client.evaluate('document.body.innerText.slice(0,1500)')}`)
}
async function shot(name) {
  await pause(700)
  const screenshot = await client.send('Page.captureScreenshot', { format: 'png' })
  const path = join(output, `${name}.png`)
  writeFileSync(path, Buffer.from(screenshot.data, 'base64'))
  results.push({ name, path })
}
const state = () => client.evaluate(`(()=>{const b=document.querySelector('.fox-expert-lanyard');const r=b.getBoundingClientRect();return {collapsed:b.dataset.collapsed,dragging:b.dataset.dragging,x:r.x+r.width/2,y:r.bottom-4,bottom:r.bottom,top:r.top}})()`)
async function mouse(type, x, y, extra = {}) { await client.send('Input.dispatchMouseEvent', { type, x, y, ...extra }) }
async function pull(distance = 48) {
  const b = await state()
  await mouse('mouseMoved', b.x, b.y)
  await pause(700)
  const peek = await state()
  await mouse('mouseMoved', peek.x, peek.y)
  await mouse('mousePressed', peek.x, peek.y, { button: 'left', clickCount: 1 })
  await mouse('mouseMoved', peek.x, peek.y + distance, { button: 'left', buttons: 1 })
  await pause(120)
  await mouse('mouseReleased', peek.x, peek.y + distance, { button: 'left', clickCount: 1 })
  await mouse('mouseMoved', 100, 200)
  await pause(750)
}
try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  browser = spawn(process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--no-first-run', '--no-default-browser-check', '--enable-unsafe-swiftshader', `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${mkdtempSync(join(output, 'chrome-profile-'))}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true })
  browser.stderr.on('data', (data) => { diagnostics += data })
  client = await connect()
  await client.send('Runtime.enable')
  await client.send('Emulation.setDeviceMetricsOverride', { width: 1200, height: 800, deviceScaleFactor: 1, mobile: false })
  for (let attempt = 0; attempt < 60; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}`)).ok) break } catch {} await pause(250) }
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/tests/fixtures/expert-interactions.html` })
  await waitFor(`!!document.querySelector('.fox-expert-lanyard')`, '卡片')
  await pause(2000)
  await shot('01-single-strap-expanded')
  const switching = await client.evaluate(`new Promise(resolve=>{const samples=[];const start=performance.now();document.querySelector('#switch-expert').click();const frame=()=>{const e=document.querySelector('.fox-expert-lanyard');samples.push({name:e?.getAttribute('aria-label'),top:e?.getBoundingClientRect().top,count:document.querySelectorAll('.fox-expert-lanyard').length});if(performance.now()-start<1000)requestAnimationFrame(frame);else resolve(samples)};requestAnimationFrame(frame)})`)
  if(!switching.some(s=>s.name?.startsWith('代码专家')&&s.top<20)||!switching.some(s=>s.name?.startsWith('写作专家')&&s.top<20)||switching.some(s=>s.count>1)||!switching.at(-1).name?.startsWith('写作专家'))throw Error('专家没有先上退再下落 '+JSON.stringify(switching))
  await shot('05-switched-expert')
  const previews=await client.evaluate(`(()=>{for(let i=0;i<6;i++)document.querySelector('#cycle-preview').click();return window.__notificationPreviews})()`)
  if(previews.map(p=>p.notification.kind).join(',')!=='completed,failed,progress,approval,question,completed'||new Set(previews.map(p=>p.notification.id)).size!==6)throw Error('测试通知未循环或ID重复')
  results.push({check:'expert-switch-sequence-and-notification-cycle',switching,previewKinds:previews.map(p=>p.notification.kind)})

  await pull(48)
  if ((await state()).collapsed !== 'true') throw Error('向下拉未收起')
  const hidden = await state()
  if (hidden.bottom < 48 || hidden.bottom > 62) throw Error(`收起边缘位置错误 ${JSON.stringify(hidden)}`)
  await shot('02-collapsed-edge')
  await mouse('mouseMoved', hidden.x, hidden.y)
  await pause(750)
  if ((await state()).bottom <= hidden.bottom + 12) throw Error('悬停未探出')
  await shot('03-hover-peek')
  await pull(48)
  if ((await state()).collapsed !== 'false') throw Error('再次下拉未展开')
  await pull(8)
  if ((await state()).collapsed !== 'false') throw Error('轻微移动误触收起')
  await client.evaluate(`document.querySelector('#show-notice').click();document.querySelector('#show-notice').click();document.querySelector('#show-notice').click()`)
  await pause(900)
  const notice = await client.evaluate(`(()=>{const r=document.querySelector('[aria-label="通知更新 (Alt+T)"]').getBoundingClientRect();return {top:r.top,right:r.right,bottom:r.bottom}})()`)
  if (notice.top < 44 || notice.top > 70 || notice.right < 1100) throw Error(`通知未在右上角 ${JSON.stringify(notice)}`)
  await mouse('mouseMoved', 1050, 90)
  await shot('04-top-right-notifications')
  const toastBounds = await client.evaluate(`Array.from(document.querySelectorAll('[aria-label="通知更新 (Alt+T)"] li')).map(e=>({top:e.getBoundingClientRect().top,bottom:e.getBoundingClientRect().bottom}))`)
  if (toastBounds.some(r=>r.top<44||r.bottom>800)) throw Error('通知堆叠越界')
  await client.evaluate(`document.querySelectorAll('button[aria-label=关闭通知]').forEach(e=>e.click());document.querySelector('.fox-expert-lanyard').click()`)
  await mouse('mouseMoved',100,200)
  await pause(800)
  await shot('06-white-print-back')
  await client.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] })
  await pause(200)
  await client.evaluate(`document.querySelector('#switch-expert').click()`)
  await pause(120)
  if(!await client.evaluate(`document.querySelector('.fox-expert-lanyard').getAttribute('aria-label').startsWith('代码专家') && Math.abs(new DOMMatrixReadOnly(getComputedStyle(document.querySelector('.fox-expert-lanyard-transition')).transform).m42)<1`)) throw Error('减少动态效果仍播放切换位移')
  await client.evaluate(`document.querySelector('.fox-expert-lanyard').focus()`)
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowUp', code: 'ArrowUp' })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowUp', code: 'ArrowUp' })
  if ((await state()).collapsed !== 'true') throw Error('键盘收起失败')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowDown', code: 'ArrowDown' })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowDown', code: 'ArrowDown' })
  if ((await state()).collapsed !== 'false') throw Error('键盘展开失败')
  await client.evaluate(`document.querySelector('#send-message').click()`)
  await waitFor(`document.querySelector('.fox-expert-lanyard').dataset.collapsed==='true'`, '发送自动收起')
  await client.evaluate(`document.querySelector('.fox-expert-lanyard').click()`)
  if ((await state()).collapsed !== 'false') throw Error('发送后不能手动展开')
  await client.send('Emulation.setEmulatedMedia', { features: [] })
  await client.evaluate(`document.querySelector('#send-message').click()`)
  await pause(800)
  if ((await state()).collapsed !== 'true' || (await state()).bottom > 62) throw Error('再次发送未向上收起')
  results.push({check:'send-auto-collapse-and-manual-reopen',passed:true})
  await shot('07-send-auto-collapse')
  if (client.errors.length) throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: true, checks: ['pull-collapse', 'hover-peek', 'pull-expand', 'small-drag', 'top-right-toast-stack', 'reduced-motion-keyboard'], results }, null, 2))
  process.stdout.write(JSON.stringify({ passed: true, output, screenshots: results.length }))
} catch (error) {
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: false, error: String(error), diagnostics, results }, null, 2))
  process.stderr.write(String(error)); process.exitCode = 1
} finally { client?.close(); browser?.kill(); server?.kill() }
