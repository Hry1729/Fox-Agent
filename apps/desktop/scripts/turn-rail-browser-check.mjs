import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/turn-rail-20261018/browser')
const pause = ms => new Promise(resolve => setTimeout(resolve, ms))
mkdirSync(output, { recursive: true })
let server, browser, socket, serial = 0
const pending = new Map(), errors = [], results = []
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const id = ++serial
  const timer = setTimeout(() => { pending.delete(id); reject(Error(`CDP timeout: ${method}`)) }, 20000)
  pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }))
})
const evaluate = async expression => {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })
  if (result.exceptionDetails) throw Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text)
  return result.result.value
}
async function check(expression, label) {
  for (let i = 0; i < 100; i++) { if (await evaluate(expression)) { results.push({ check: label, passed: true }); return }; await pause(100) }
  throw Error(`${label}: ${await evaluate('document.body.innerText.slice(0,1800)')}`)
}
async function click(selector) {
  const point = await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`)
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point })
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 })
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 }); await pause(200)
}
async function escape() { await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape' }); await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape' }); await pause(200) }
async function screenshot(name) {
  await pause(300)
  const shot = await send('Page.captureScreenshot', { format: 'png' })
  writeFileSync(join(output, `${name}.png`), Buffer.from(shot.data, 'base64'))
}
try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', '1482', '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  browser = spawn('C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--remote-debugging-port=9387', `--user-data-dir=${mkdtempSync(join(output, 'chrome-profile-'))}`, 'about:blank'], { stdio: 'ignore', windowsHide: true })
  let target
  for (let i = 0; i < 100; i++) { try { target = (await fetch('http://127.0.0.1:9387/json').then(r => r.json())).find(t => t.type === 'page'); if (target && (await fetch('http://127.0.0.1:1482/tests/fixtures/turn-rail.html')).ok) break } catch {} await pause(200) }
  if (!target) throw Error('browser unavailable')
  socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }) })
  socket.addEventListener('message', ({ data }) => {
    const response = JSON.parse(data), item = pending.get(response.id)
    if (response.method === 'Runtime.exceptionThrown') errors.push(response.params.exceptionDetails)
    if (item) { clearTimeout(item.timer); pending.delete(response.id); response.error ? item.reject(Error(JSON.stringify(response.error))) : item.resolve(response.result) }
  })
  await send('Runtime.enable'); await send('Page.enable')
  await send('Emulation.setDeviceMetricsOverride', { width: 900, height: 768, deviceScaleFactor: 1, mobile: false })
  await send('Page.navigate', { url: 'http://127.0.0.1:1482/tests/fixtures/turn-rail.html' })
  await check(`document.querySelectorAll('.fox-conversation-marks button').length===12 && document.querySelector('.fox-thread-viewport').scrollHeight>innerHeight`, 'real thread and twelve rail targets rendered')
  await pause(1200)
  console.log(JSON.stringify(await evaluate(`Object.fromEntries(['.fox-conversation-rail','.fox-thread-msg','.fox-chat-stage','.fox-thread-viewport'].map(sel=>{const e=document.querySelector(sel),r=e.getBoundingClientRect();return [sel,{left:r.left,right:r.right,width:r.width,col:getComputedStyle(e).getPropertyValue('--fox-chat-column-width')}] }))`)))
  await check(`(()=>{const rail=document.querySelector('.fox-conversation-rail').getBoundingClientRect(),msg=document.querySelector('.fox-thread-msg').getBoundingClientRect(),stage=document.querySelector('.fox-chat-stage').getBoundingClientRect();return Math.abs((rail.left-stage.left)-(msg.left-rail.right))<2})()`, 'rail equal spacing from stage edge and message column')
  const before = await evaluate(`document.querySelector('.fox-thread-viewport').scrollTop`)
  await click('.fox-conversation-marks button:first-child')
  await check(`(()=>{const v=document.querySelector('.fox-thread-viewport'),t=document.querySelector('[data-turn-message-id="u-0"] > *');return Math.abs(t.getBoundingClientRect().top-v.getBoundingClientRect().top-12)<4})()`, 'click first turn scrolls actual message into view')
  await pause(800)
  await check(`(()=>{const v=document.querySelector('.fox-thread-viewport'),t=document.querySelector('[data-turn-message-id="u-0"] > *');return Math.abs(t.getBoundingClientRect().top-v.getBoundingClientRect().top-12)<4})()`, 'automatic scroll does not pull back to bottom')
  await click('.fox-conversation-marks button:nth-child(6)')
  await check(`(()=>{const v=document.querySelector('.fox-thread-viewport'),t=document.querySelector('[data-turn-message-id="u-5"] > *');return Math.abs(t.getBoundingClientRect().top-v.getBoundingClientRect().top-12)<4})()`, 'middle turn aligns in viewport')
  await pause(300)
  await check(`document.querySelector('.fox-conversation-marks button:nth-child(6)').classList.contains('is-active')`, 'active mark follows visible target')
  await evaluate(`document.querySelector('.fox-conversation-marks button:first-child').focus()`)
  await pause(200)
  await send('Input.dispatchKeyEvent', {type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13,text:'\r'})
  await send('Input.dispatchKeyEvent', {type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13})
  console.log(JSON.stringify(await evaluate(`({focus:document.activeElement?.outerHTML,top:document.querySelector('.fox-thread-viewport').scrollTop,target:document.querySelector('[data-turn-message-id="u-0"] > *').getBoundingClientRect().top})`)))
  await check(`(()=>{const v=document.querySelector('.fox-thread-viewport'),t=document.querySelector('[data-turn-message-id="u-0"] > *');return Math.abs(t.getBoundingClientRect().top-v.getBoundingClientRect().top-12)<4})()`, 'keyboard navigation jumps to first turn')
  const hover = await evaluate(`(()=>{const r=document.querySelector('.fox-conversation-marks button:nth-child(4)').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`)
  await send('Input.dispatchMouseEvent',{type:'mouseMoved',...hover})
  await check(`document.querySelector('.fox-conversation-preview.is-visible strong')?.textContent.includes('第 4 轮') && document.querySelector('.fox-conversation-preview p')?.textContent.includes('已完成第 4 轮分析') && !document.querySelector('.fox-conversation-preview svg') && !document.querySelector('.fox-conversation-preview hr')`, 'hover preview has text hierarchy without icons or dividers')
  await screenshot('turn-preview')
  await send('Emulation.setDeviceMetricsOverride', {width: 700, height: 560, deviceScaleFactor:1,mobile:false})
  await check(`getComputedStyle(document.querySelector('.fox-conversation-rail')).display==='none' && document.documentElement.scrollWidth<=innerWidth`, 'narrow viewport has no overflowing rail')
  await screenshot('narrow-thread')
  results.push({check:'initial scroll position',value:before})
  if (errors.length) throw Error(JSON.stringify(errors))
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: true, results, errors }, null, 2))
  console.log(JSON.stringify({ passed: true, results, errors }))
} catch (error) { console.error(error); if (socket?.readyState === 1) await screenshot('failure'); process.exitCode = 1 }
finally { socket?.close(); browser?.kill(); server?.kill() }
