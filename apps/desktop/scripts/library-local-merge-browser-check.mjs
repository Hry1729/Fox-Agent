import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/library-local-merge-20261016/browser')
const port = 1474, cdpPort = 9380
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


  await navigate('folders=1&theme=paper')
  const merged=await client.evaluate(`(()=>{const section=document.querySelector('section[aria-label="本机"]');return {cards:section?.querySelectorAll('.fox-kb-cover-card').length,grids:section?.querySelectorAll('.fox-kb-cover-grid').length,title:section?.querySelector('h2').textContent,placeholder:section?.querySelector('.fox-kb-cover-card p')?.textContent,successDots:document.querySelectorAll('.fox-kb-cover-card .fox-status-dot.is-ok').length,readyLabel:document.querySelector('main').innerText.includes('可以使用'),oldGroup:!!document.querySelector('section[aria-label="本机文件夹"]')}})()`)
  if(merged.cards!==4||merged.grids!==1||merged.placeholder!=='无简介'||merged.successDots||merged.readyLabel||merged.oldGroup)throw Error(JSON.stringify(merged))
  results.push({check:'merged local grid and empty description, no ready badges',...merged})
  await screenshot('merged-local')
  async function edit(name) {
    await client.evaluate(`document.querySelector('button[aria-label="${name} 的更多操作"]').dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,button:0,pointerType:'mouse',ctrlKey:false}))`)
    await waitFor(`[...document.querySelectorAll('[role="menuitem"]')].some(e=>e.textContent.includes('编辑信息'))`,'edit menu')
    await client.evaluate(`[...document.querySelectorAll('[role="menuitem"]')].find(e=>e.textContent.includes('编辑信息')).click()`)
    await waitFor(`!!document.querySelector('#local-folder-edit-name')`,'edit form')
  }
  const setDescription=value=>client.evaluate(`(()=>{const e=document.querySelector('#local-folder-edit-description');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(e,${JSON.stringify(value)});e.dispatchEvent(new Event('input',{bubbles:true}))})()`)
  await edit('项目资料')
  await input('#local-folder-edit-name','年度项目资料')
  await setDescription('合同、方案和交付材料')
  await screenshot('edit-folder')
  await clickText('保存')
  await waitFor(`!!document.querySelector('button[aria-label="打开本机文件夹年度项目资料"]')`,'saved name')
  if(!await client.evaluate(`document.querySelector('button[aria-label="打开本机文件夹年度项目资料"] p').textContent==='合同、方案和交付材料'`))throw Error('description missing')
  await screenshot('saved-description')
  await edit('年度项目资料')
  if(!await client.evaluate(`document.querySelector('#local-folder-edit-description').value==='合同、方案和交付材料'`))throw Error('reopened value missing')
  await setDescription('')
  await clickText('保存')
  await waitFor(`document.querySelector('button[aria-label="打开本机文件夹年度项目资料"] p').textContent==='无简介'`,'cleared description')
  results.push({check:'edit name and description, reopen and clear description',passed:true,operations:await client.evaluate('window.__libraryFixture.operations')})
  if(client.errors.length)throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,'report.json'),JSON.stringify({passed:true,results},null,2));console.log(JSON.stringify({passed:true,results}))
} catch(error) {console.error(error);process.exitCode=1} finally {client?.close();browser?.kill();server?.kill()}
