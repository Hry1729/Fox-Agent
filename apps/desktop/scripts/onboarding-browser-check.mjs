import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/onboarding-20261016/browser')
const port = 1466, cdpPort = 9372
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
  const previous = await client.evaluate('window.__themeFixture?.generation')
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/tests/fixtures/theme-refinements.html?${params}` })
  await waitFor(`window.__themeFixture?.generation && window.__themeFixture.generation!==${JSON.stringify(previous ?? null)} && !!document.querySelector('.fox-onboarding-screen')`, '组件夹具')
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
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/tests/fixtures/theme-refinements.html`)).ok) { serverReady = true; break } } catch {} await pause(150) }
  if (!serverReady) throw new Error('组件夹具服务未启动')
  for (const theme of ['graphite','paper','mist']) for (const dark of [false,true]) {
    await navigate(`surface=onboarding&theme=${theme}${dark?'&dark=1':''}`)
    await waitFor('document.body.innerText.includes("测试模型")', '模型状态')
    await screenshot(`${theme}-${dark?'dark':'light'}`)
    const colors = await client.evaluate(`(()=>{const s=e=>getComputedStyle(document.querySelector(e));return {theme:document.documentElement.dataset.colorTheme,main:s('.fox-onboarding-main').backgroundColor,panel:s('.fox-onboarding-visual').backgroundColor,status:s('.fox-onboarding-status').backgroundColor}})()`)
    results.push({colors})
  }
  await navigate('surface=onboarding&theme=paper', 900)
  const motionSample = async (label) => client.evaluate(`new Promise(resolve=>{[...document.querySelectorAll('button')].find(e=>e.textContent.trim()===${JSON.stringify(label)}).click();setTimeout(()=>resolve([...document.querySelectorAll('.fox-onboarding-step-content')].map(e=>({title:e.querySelector('h1').textContent,x:new DOMMatrixReadOnly(getComputedStyle(e).transform).m41}))),90)})`)
  let samples = await motionSample('下一步')
  if (!(samples.find(s=>s.title==='连接模型服务')?.x<0 && samples.find(s=>s.title==='连接资料库服务')?.x>0)) throw Error('下一步方向错误 '+JSON.stringify(samples))
  await pause(500)
  samples = await motionSample('上一步')
  if (!(samples.find(s=>s.title==='连接资料库服务')?.x>0 && samples.find(s=>s.title==='连接模型服务')?.x<0)) throw Error('上一步方向错误 '+JSON.stringify(samples))
  await pause(500)
  await clickText('下一步'); await pause(500)
  samples = await motionSample('下一步')
  if (!(samples.find(s=>s.title==='连接资料库服务')?.x<0 && samples.find(s=>s.title==='开始使用 Fox')?.x>0)) throw Error('第三步方向错误 '+JSON.stringify(samples))
  await pause(500)
  await screenshot('third-step-compact')
  await client.evaluate('document.documentElement.dataset.colorTheme="graphite";document.documentElement.classList.add("dark")')
  await pause(500)
  await screenshot('live-theme-switch')
  await client.send('Emulation.setEmulatedMedia',{features:[{name:'prefers-reduced-motion',value:'reduce'}]})
  await pause(200)
  await clickText('上一步'); await pause(100)
  if (!await client.evaluate('document.querySelectorAll(".fox-onboarding-step-content").length===1')) throw Error('减少动态效果未生效')
  if (client.errors.length) throw Error(JSON.stringify(client.errors))
  if (results.some(item=>item.overflow)) throw Error('横向溢出')
  writeFileSync(join(output,'report.json'),JSON.stringify({passed:true,evidence:'synthetic browser fixture',checks:['six themes','forward and backward transforms','third step','live theme switch','reduced motion'],results},null,2))
  process.stdout.write(JSON.stringify({passed:true,output}))
} catch (error) {
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: false, error: String(error), diagnostics, results }, null, 2))
  process.stderr.write(`${error.stack ?? error}\n`)
  process.exitCode = 1
} finally { client?.close(); browser?.kill(); server?.kill() }
