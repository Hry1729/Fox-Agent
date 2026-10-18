import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/navigation-repair-20261017/browser')
const port = 1477, cdpPort = 9383
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
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP 超时：${method}`)) }, 60000)
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }))
  })
  const evaluate = async (expression) => { const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text); return result.result.value }
  return { send, evaluate, errors, close: () => socket.close() }
}
async function waitFor(expression, description) {
  for (let index = 0; index < 200; index++) { if (await client.evaluate(expression)) return; await pause(120) }
  throw new Error(`${description} 未就绪；${JSON.stringify(client.errors)}；${await client.evaluate('document.body.innerText.slice(0,1500)')}`)
}
async function screenshot(name) {
  await pause(650)
  const shot = await client.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
  const path = join(output, `${name}.png`)
  writeFileSync(path, Buffer.from(shot.data, 'base64'))
  results.push({ screenshot: name, path, overflow: await client.evaluate('document.documentElement.scrollWidth>innerWidth+1') })
}
try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  const profile = mkdtempSync(join(output, 'chrome-profile-'))
  browser = spawn(process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true })
  browser.stderr.on('data', (data) => { diagnostics = (diagnostics + data).slice(-4000) })
  client = await connect()
  await client.send('Page.enable'); await client.send('Runtime.enable')
  let serverReady = false
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/tests/fixtures/navigation-repair.html`)).ok) { serverReady = true; break } } catch {} await pause(150) }
  if (!serverReady) throw new Error('组件夹具服务未启动')



  const hover = async (selector) => { const point=await client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',...point});await pause(350) }
  const key = async key => { await client.send('Input.dispatchKeyEvent',{type:'keyDown',key,code:key});await client.send('Input.dispatchKeyEvent',{type:'keyUp',key,code:key});await pause(180) }
  const bounds = async selector => client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)}),r=e.getBoundingClientRect();return {left:r.left,top:r.top,right:r.right,bottom:r.bottom,width:r.width,height:r.height,client:e.clientHeight,scroll:e.scrollHeight,innerWidth,innerHeight}})()`)
  const assertBounds = value => {if(value.left<0||value.top<0||value.right>value.innerWidth+1||value.bottom>value.innerHeight+1)throw Error(JSON.stringify(value))}
  await client.send('Emulation.setDeviceMetricsOverride',{width:1200,height:800,deviceScaleFactor:1,mobile:false})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/navigation-repair.html`})
  await waitFor(`!!window.navigationFixture && !!document.querySelector('.fox-sidebar')`, '导航夹具')
  await hover('[aria-label="应用菜单"]')
  const menuPoint=await client.evaluate(`(()=>{const r=document.querySelector('[aria-label="应用菜单"]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`)
  await client.send('Input.dispatchMouseEvent',{type:'mousePressed',...menuPoint,button:'left',clickCount:1})
  await client.send('Input.dispatchMouseEvent',{type:'mouseReleased',...menuPoint,button:'left',clickCount:1})
  await waitFor(`document.querySelectorAll('.fox-app-menu [role="menuitem"]').length===4`, '四个一级菜单')
  const names=await client.evaluate(`[...document.querySelectorAll('.fox-app-menu [role="menuitem"]')].map(x=>x.textContent)`)
  if(JSON.stringify(names)!==JSON.stringify(['文件','编辑','视图','帮助']))throw Error(JSON.stringify(names))
  await hover('.fox-app-menu [role="menuitem"]')
  await waitFor(`!!document.querySelector('[data-slot="dropdown-menu-sub-content"]')`, '悬浮子菜单')
  await screenshot('application-menu')
  await key('Escape');await key('Escape')
  const order=await client.evaluate(`(()=>{const plugin=document.querySelector('.fox-sidebar-primary [aria-label="插件"]');const project=document.querySelector('[aria-label^="展开项目列表"]');const convo=document.querySelector('[aria-label^="展开对话列表"]');return {plugin:plugin.getBoundingClientRect().bottom,project:project.getBoundingClientRect().top,convo:convo.getBoundingClientRect().top,border:getComputedStyle(project.closest('.fox-sidebar-collapsed-sections')).borderTopWidth}})()`)
  if(!(order.plugin<order.project&&order.project<order.convo&&order.border==='1px'))throw Error(JSON.stringify(order))
  results.push({check:'four application submenus and rail order',names,...order})
  await hover('[aria-label^="展开项目列表"]')
  await waitFor(`!!document.querySelector('[aria-label="项目列表"][role="menu"]')`, '项目列表')
  const projects=await bounds('[aria-label="项目列表"][role="menu"]');assertBounds(projects);if(projects.scroll<=projects.client)throw Error('项目列表没有内部滚动')
  await hover('[aria-label="项目列表"] [data-slot="dropdown-menu-sub-trigger"]')
  await waitFor(`!!document.querySelector('[aria-label="项目-01 对话"][role="menu"]')`, '项目内对话')
  const conversations=await bounds('[aria-label="项目-01 对话"][role="menu"]');assertBounds(conversations);if(conversations.scroll<=conversations.client)throw Error('对话列表没有内部滚动')
  await screenshot('project-and-conversations')
  results.push({check:'large project and conversation menus stay within viewport',projects,conversations})
  await client.evaluate(`(()=>{const e=document.querySelector('[aria-label="项目-01 对话"]');e.scrollTop=e.scrollHeight})()`)
  await pause(100)
  await client.evaluate(`document.querySelector('[aria-label="项目-01 对话"] [role="menuitem"]:last-child').click()`)
  if(!await client.evaluate(`window.navigationFixture.action?.startsWith('c-')`))throw Error('滚动后无法选择对话')
  await key('Escape');await key('Escape')
  await hover('[aria-label^="展开对话列表"]')
  await waitFor(`!!document.querySelector('[aria-label="对话列表"][role="menu"]')`, '独立对话列表')
  const regular=await bounds('[aria-label="对话列表"][role="menu"]');assertBounds(regular)
  await key('Escape')
  await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',x:850,y:500})
  const animation=await client.evaluate(`new Promise(resolve=>{const samples=[];const start=performance.now();window.navigationFixture.toggle();function next(t){samples.push({t:t-start,width:document.querySelector('.fox-sidebar').getBoundingClientRect().width});if(t-start<350)requestAnimationFrame(next);else resolve(samples)}requestAnimationFrame(next)})`)
  if(Math.abs(animation.at(-1).width-288)>1)throw Error(JSON.stringify(animation))
  results.push({check:'expand animation',frames:animation.length,samples:animation})
  await client.evaluate(`window.navigationFixture.toggle()`);await pause(300)
  await client.send('Emulation.setDeviceMetricsOverride',{width:800,height:520,deviceScaleFactor:1,mobile:false})
  await hover('[aria-label^="展开项目列表"]')
  await waitFor(`!!document.querySelector('[aria-label="项目列表"][role="menu"]')`, '小窗口项目列表')
  assertBounds(await bounds('[aria-label="项目列表"][role="menu"]'))
  await hover('[aria-label="项目列表"] [data-slot="dropdown-menu-sub-trigger"]')
  await waitFor(`!!document.querySelector('[aria-label="项目-01 对话"][role="menu"]')`, '小窗口子菜单')
  assertBounds(await bounds('[aria-label="项目-01 对话"][role="menu"]'))
  await screenshot('small-window-menus')
  if(client.errors.length)throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,'report.json'),JSON.stringify({passed:true,results,errors:client.errors},null,2))
  console.log(JSON.stringify({passed:true,results,errors:client.errors}))
} catch(error){console.error(error);if(client){console.error(await client.evaluate(`JSON.stringify({menus:[...document.querySelectorAll('[role=menu]')].map(e=>e.outerHTML.slice(0,500)),buttons:[...document.querySelectorAll('.fox-sidebar-collapsed-entry button')].map(e=>({html:e.outerHTML,hover:e.matches(':hover')})),bodyStyle:document.body.getAttribute('style')})`));await screenshot('failure')}process.exitCode=1} finally {client?.close();browser?.kill();server?.kill()}
