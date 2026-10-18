import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/navigation-history-20261018/browser')
const port = 1479, cdpPort = 9385
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
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/tests/fixtures/navigation-history.html`)).ok) { serverReady = true; break } } catch {} await pause(150) }
  if (!serverReady) throw new Error('组件夹具服务未启动')



  const point = selector => client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`)
  const click = async selector => {const p=await point(selector);await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',...p});await pause(180);await client.send('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',clickCount:1});await client.send('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',clickCount:1});await pause(180)}
  const key = async key => {await client.send('Input.dispatchKeyEvent',{type:'keyDown',key,code:key});await client.send('Input.dispatchKeyEvent',{type:'keyUp',key,code:key});await pause(150)}
  const route = (view,id=null) => waitFor(`window.navigationFixture.route.view===${JSON.stringify(view)}&&window.navigationFixture.route.conversationId===${JSON.stringify(id)}`,`${view}:${id}`)
  await client.send('Emulation.setDeviceMetricsOverride',{width:1120,height:800,deviceScaleFactor:1,mobile:false})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/navigation-history.html`})
  await waitFor(`!!window.navigationFixture && !!document.querySelector('.fox-sidebar')`,'夹具')
  await pause(700)
  if(!await client.evaluate(`document.querySelector('[aria-label="后退"]').disabled&&document.querySelector('[aria-label="前进"]').disabled`))throw Error('初始历史按钮状态错误')
  await click('[aria-label="切换到资料"]');await route('local-knowledge-home')
  await click('[aria-label="切换到助手"]');await route('chat')
  await click('[aria-label="后退"]');await route('local-knowledge-home')
  await click('[aria-label="前进"]');await route('chat')
  results.push({check:'collapsed mode switch and history both directions',passed:true})
  await click('[aria-label="后退"]');await route('local-knowledge-home')
  await client.evaluate(`window.navigationFixture.navigate('agents')`);await route('agents')
  if(!await client.evaluate(`document.querySelector('[aria-label="前进"]').disabled`))throw Error('新页面没有清理前进分支')
  await client.evaluate(`window.navigationFixture.open('c-a')`);await route('chat','c-a')
  await client.evaluate(`window.navigationFixture.open('c-b')`);await route('chat','c-b')
  await click('[aria-label="后退"]');await route('chat','c-a')
  await click('[aria-label="前进"]');await route('chat','c-b')
  await client.evaluate(`window.failHistoryOnce=true`)
  await click('[aria-label="后退"]');await route('chat','c-b')
  await waitFor(`window.navigationFixture.failure.includes('页面已移除')`,'错误回退')
  results.push({check:'conversation history, branching, failed restore rollback',passed:true})
  await click('[aria-label="应用菜单"]')
  await waitFor(`!!document.querySelector('.fox-app-menu')`,'菜单')
  const menu=await client.evaluate(`(()=>{const e=document.querySelector('.fox-app-menu');return {width:e.getBoundingClientRect().width,overflow:e.scrollWidth>e.clientWidth}})()`)
  if(menu.width>90||menu.width<60||menu.overflow)throw Error(JSON.stringify(menu))
  await screenshot('compact-menu')
  results.push({check:'content-sized application menu',...menu})
  await key('Escape')
  for(const label of ['展开项目列表','展开对话列表']){
    const selector='[aria-label^="'+label+'"]'
    await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',x:800,y:600});await pause(800)
    const p=await point(selector)
    await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',...p});await pause(150)
    const transforms=await client.evaluate(`[...document.querySelectorAll(${JSON.stringify(selector+' svg path')})].map(e=>getComputedStyle(e).transform)`)
    if(transforms.every(value=>value==='none'||value==='matrix(1, 0, 0, 1, 0, 0)'))throw Error('图标没有动画 '+label+' '+JSON.stringify(transforms))
    results.push({check:'animated collapsed icon',label,transforms})
    await key('Escape')
  }
  await screenshot('collapsed-navigation')
  await click('[aria-label="展开左侧边栏"]')
  await pause(300)
  await waitFor(`document.querySelector('.fox-sidebar-date-heading')?.textContent.includes('今天') && [...document.querySelectorAll('.fox-sidebar-tree-main')].every(e=>!e.querySelector('svg.lucide-message-circle-more')) && document.querySelectorAll('.fox-sidebar-tree-main > [data-run-indicator]').length===2`, '日期分组与左侧状态')
  await screenshot('expanded-navigation')
  await client.send('Emulation.setDeviceMetricsOverride',{width:700,height:560,deviceScaleFactor:1,mobile:false})
  await click('[aria-label="收起左侧边栏"]')
  await screenshot('narrow-navigation')
  if(client.errors.length)throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,'report.json'),JSON.stringify({passed:true,results,errors:client.errors},null,2))
  console.log(JSON.stringify({passed:true,results,errors:client.errors}))
} catch(error){console.error(error);if(client)await screenshot('failure');process.exitCode=1} finally {client?.close();browser?.kill();server?.kill()}
