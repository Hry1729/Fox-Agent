import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/sidebar-cards-20261016/browser')
const port = 1470, cdpPort = 9376
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
async function navigate(params = '', width = 1400) {
  await client.send('Emulation.setDeviceMetricsOverride', { width, height: 950, deviceScaleFactor: 1, mobile: false })
  const previous = await client.evaluate('window.__libraryFixture?.generation')
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/tests/fixtures/library-unification.html?${params}` })
  await waitFor(`window.__libraryFixture?.generation && window.__libraryFixture.generation!==${JSON.stringify(previous ?? null)} && !!document.querySelector('.fox-local-kb-page,.fox-library-manager,.fox-sidebar-list-pane')`, '组件夹具')
  await client.evaluate('document.fonts.ready.then(()=>true)')
  await pause(500)
}
async function screenshot(name) {
  await pause(650)
  const shot = await client.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
  const path = join(output, `${name}.png`)
  writeFileSync(path, Buffer.from(shot.data, 'base64'))
  results.push({ screenshot: name, path, overflow: await client.evaluate('document.documentElement.scrollWidth>innerWidth+1') })
}
const clickText = (text) => client.evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find(item=>item.textContent.trim()===${JSON.stringify(text)});if(!button)throw Error('没有找到按钮 '+${JSON.stringify(text)});button.click()})()`)
const input = (selector, value) => client.evaluate(`(()=>{const element=document.querySelector(${JSON.stringify(selector)});if(!element)throw Error('找不到输入框');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(element,${JSON.stringify(value)});element.dispatchEvent(new Event('input',{bubbles:true}));element.dispatchEvent(new Event('change',{bubbles:true}))})()`)

try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  const profile = mkdtempSync(join(output, 'chrome-profile-'))
  browser = spawn(process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true })
  browser.stderr.on('data', (data) => { diagnostics = (diagnostics + data).slice(-4000) })
  client = await connect()
  await client.send('Page.enable'); await client.send('Runtime.enable')
  let serverReady = false
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/tests/fixtures/library-unification.html`)).ok) { serverReady = true; break } } catch {} await pause(150) }
  if (!serverReady) throw new Error('组件夹具服务未启动')
  const click = async (selector) => {
    const point=await client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});e.scrollIntoView({block:'center'});const r=e.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`)
    await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',...point})
    await client.send('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',clickCount:1})
    await client.send('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',clickCount:1})
  }
  await navigate('folders=1',1000)
  await waitFor(`!!document.querySelector('button[aria-label="项目资料 的更多操作"]')`, '文件夹卡片')
  if(await client.evaluate(`!!document.querySelector('.fox-local-folder-path')`)) throw Error('卡片仍显示路径')
  await click('button[aria-label="项目资料 的更多操作"]')
  await waitFor(`!!document.querySelector('.fox-library-menu-path')`, '菜单文件位置')
  if(!await client.evaluate(`!!document.querySelector('.fox-kb-cover-grid') && !document.querySelector('.fox-library-manager')`)) throw Error('菜单意外导航')
  await screenshot('compact-cards-folder-menu')
  await client.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
  await client.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
  await click('button[aria-label="产品资料库 的更多操作"]')
  await waitFor(`!!document.querySelector('[role="menuitem"]')`, '资料库菜单')
  if(!await client.evaluate(`!!document.querySelector('.fox-kb-cover-grid') && !document.querySelector('.fox-library-manager')`)) throw Error('资料库菜单意外导航')
  await client.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
  await client.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
  await client.evaluate(`document.querySelector('button[aria-label="产品资料库 的更多操作"]').focus()`)
  await pause(200)
  await client.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13})
  await client.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13})
  await waitFor(`[...document.querySelectorAll('[role="menuitem"]')].some(e=>e.textContent.includes('编辑信息'))`, '键盘打开菜单')
  if(!await client.evaluate(`!!document.querySelector('.fox-kb-cover-grid') && !document.querySelector('.fox-library-manager')`)) throw Error('键盘菜单意外导航')
  await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.includes('编辑信息')).click()`)
  await waitFor(`!!document.querySelector('.fox-kb-settings-dialog')`, '菜单编辑仍正常')
  results.push({check:'mouse and keyboard card menus do not navigate; edit opens; location moved into menu',passed:true})
  await navigate('folders=1',1000)
  await click('button[aria-label="打开资料库产品资料库"]')
  await waitFor(`!!document.querySelector('.fox-library-manager')`, '卡片主体打开资料库')
  await client.evaluate(`localStorage.setItem('fox.onboarding.status','complete');localStorage.setItem('fox.layout.sidebarCollapsed','0');localStorage.removeItem('fox.layout.sidebarWidth')`)
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/`})
  await waitFor(`!!document.querySelector('.fox-sidebar-resize-divider')`, 'Workbench 手动调宽')
  const measure=()=>client.evaluate(`({window:innerWidth,sidebar:document.querySelector('.fox-sidebar').getBoundingClientRect().width,collapsed:document.querySelector('.fox-sidebar').classList.contains('is-collapsed'),stored:localStorage.getItem('fox.layout.sidebarWidth')})`)
  const before=await measure()
  if(Math.abs(before.sidebar-288)>1)throw Error('默认侧栏宽度不是 288 '+JSON.stringify(before))
  const handle=await client.evaluate(`(()=>{const r=document.querySelector('.fox-sidebar-resize-divider').getBoundingClientRect();return{x:r.x+r.width/2,y:200}})()`)
  await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',...handle})
  await client.send('Input.dispatchMouseEvent',{type:'mousePressed',...handle,button:'left',clickCount:1})
  await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',x:handle.x+40,y:handle.y,button:'left',buttons:1})
  await client.send('Input.dispatchMouseEvent',{type:'mouseReleased',x:handle.x+40,y:handle.y,button:'left',clickCount:1})
  await pause(300)
  const dragged=await measure()
  if(Math.abs(dragged.sidebar-before.sidebar-40)>1) throw Error('侧栏拖动无效 '+JSON.stringify({before,dragged}))
  const widths=[]
  for(const width of [900,701,700,600,1000]) {
    await client.send('Emulation.setDeviceMetricsOverride',{width,height:950,deviceScaleFactor:1,mobile:false})
    await pause(300)
    widths.push(await measure())
  }
  for(const entry of widths) {
    if(entry.window<=700 ? !entry.collapsed||Math.abs(entry.sidebar-64)>1 : entry.collapsed||Math.abs(entry.sidebar-dragged.sidebar)>1) throw Error('自动折叠或宽度保持失败 '+JSON.stringify(widths))
  }
  results.push({check:'real Workbench dragging, stable expanded width, 700 collapse, 600 minimum view, restore width',before,dragged,widths})
  await screenshot('sidebar-restored-width')
  writeFileSync(join(output,'report.json'),JSON.stringify({passed:true,evidence:'browser fixture cards and production Workbench with mock runtime',results},null,2))
  process.stdout.write(JSON.stringify({passed:true,output,results}))
} catch (error) {
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: false, error: String(error), diagnostics, results }, null, 2))
  process.stderr.write(`${error.stack ?? error}\n`)
  process.exitCode = 1
} finally { client?.close(); browser?.kill(); server?.kill() }
