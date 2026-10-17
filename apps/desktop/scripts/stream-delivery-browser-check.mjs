import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/stream-delivery-20261016/browser')
const port = 1476, cdpPort = 9382
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
async function navigate(params = '', width = 1400) {
  await client.send('Emulation.setDeviceMetricsOverride', { width, height: 950, deviceScaleFactor: 1, mobile: false })
  const previous = await client.evaluate('window.__libraryFixture?.generation')
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/tests/fixtures/stream-delivery.html?${params}` })
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
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/tests/fixtures/stream-delivery.html`)).ok) { serverReady = true; break } } catch {} await pause(150) }
  if (!serverReady) throw new Error('组件夹具服务未启动')


  await client.send('Emulation.setDeviceMetricsOverride',{width:1100,height:750,deviceScaleFactor:1,mobile:false})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/stream-delivery.html`})
  await waitFor(`!!window.streamFixture && !!document.querySelector('.fox-thread-viewport')`, '流式夹具')
  const streaming = await client.evaluate(String.raw`new Promise(resolve=>{let text='',i=0;const samples=[];function step(){text+='第'+i+'段：这是快速流式输出，用于观察页面是否及时跟随。'.repeat(5)+'\n\n';window.streamFixture.setText(text);requestAnimationFrame(()=>requestAnimationFrame(()=>{const v=document.querySelector('.fox-thread-viewport');samples.push({i,gap:v.scrollHeight-v.clientHeight-v.scrollTop});if(++i<20)setTimeout(step,16);else resolve(samples)}))}step()})`)
  results.push({check:'streaming follow',samples:streaming,maxGap:Math.max(...streaming.slice(3).map(s=>s.gap))})

  await client.evaluate(`(()=>{const v=document.querySelector('.fox-thread-viewport');v.dispatchEvent(new WheelEvent('wheel',{deltaY:-600,bubbles:true}));v.scrollTop-=600})()`)
  await pause(150)
  const catchup=await client.evaluate(String.raw`new Promise(resolve=>{document.querySelector('.fox-scroll-button').click();let i=0;const samples=[];function step(){const v=document.querySelector('.fox-thread-viewport');samples.push({i,gap:v.scrollHeight-v.clientHeight-v.scrollTop});if(++i<8)requestAnimationFrame(step);else resolve(samples)}requestAnimationFrame(step)})`)
  results.push({check:'return to latest catchup',samples:catchup})

  if(streaming.slice(3).some(s=>s.gap>2)||catchup.some(s=>s.gap>2))throw Error('流式滚动没有跟上')
  const typography=await client.evaluate(`(()=>{const p=document.querySelector('.fox-answer-body p');const c=getComputedStyle(p);return {fontSize:c.fontSize,lineHeight:c.lineHeight,marginBottom:c.marginBottom}})()`)
  if(Math.abs(parseFloat(typography.lineHeight)/parseFloat(typography.fontSize)-1.5)>.01||typography.marginBottom!=='12px')throw Error(JSON.stringify(typography))
  results.push({check:'paragraph line height and gap',...typography})
  await client.evaluate(`(()=>{const v=document.querySelector('.fox-thread-viewport');v.dispatchEvent(new WheelEvent('wheel',{deltaY:-500,bubbles:true}));v.scrollTop-=500})()`)
  await pause(150)
  const beforeRead=await client.evaluate(`document.querySelector('.fox-thread-viewport').scrollTop`)
  await client.evaluate(String.raw`window.streamFixture.setText(document.querySelector('.fox-answer-body').innerText+'\n\n新增内容'.repeat(30))`)
  await pause(400)
  const afterRead=await client.evaluate(`document.querySelector('.fox-thread-viewport').scrollTop`)
  if(Math.abs(beforeRead-afterRead)>2)throw Error('阅读位置被新输出拉走')
  results.push({check:'manual upward scroll remains paused',beforeRead,afterRead})
  const agvPath=process.env.AGV_FIXTURE || resolve(output,'../agv-fixture.json')
  const agv=JSON.parse(readFileSync(agvPath,'utf8'))
  const records=agv.artifacts.map(a=>({...a,conversationId:'stream-test',runId:'run-test'}))
  await client.evaluate(`window.streamFixture.finish(${JSON.stringify(agv.final)},[])`)
  await waitFor(`!!document.querySelector('.fox-message-footer')`, '最终回复')
  await client.evaluate(`window.streamFixture.setDetail({...window.streamFixture.detail,artifacts:${JSON.stringify(records)}})`)
  await waitFor(`!!document.querySelector('[aria-label="本次文件结果"] .fox-message-artifact-trigger')`, '晚到文件卡片')
  const delivery=await client.evaluate(`(()=>{const region=document.querySelector('[aria-label="本次文件结果"]');return {names:[...region.querySelectorAll('[data-slot="file-name"]')].map(e=>e.textContent),processOpen:document.querySelector('.fox-message-artifact-group-trigger')?.getAttribute('aria-expanded')}})()`)
  if(!delivery.names.some(n=>n.endsWith('.pdf'))||delivery.processOpen!=='false')throw Error(JSON.stringify(delivery))
  results.push({check:'real AGV reply with late artifact records',...delivery,fileChecks:agv.checks})
  await client.evaluate(`document.querySelector('.fox-message-artifacts button').click()`)
  if(!await client.evaluate(`!!window.openedArtifact`))throw Error('文件卡片没有打开记录')
  const selected=records.find(a=>a.displayName.endsWith('.pdf'))
  await client.evaluate(`window.streamFixture.finish(${JSON.stringify('[报告](#fox-artifact='+encodeURIComponent(selected.id)+')')},${JSON.stringify(records)})`)
  await waitFor(`!!document.querySelector('a[href^="#fox-artifact="]')`, '模型文件引用')
  await client.evaluate(`window.openedArtifact=null;document.querySelector('a[href^="#fox-artifact="]').click()`)
  if(await client.evaluate(`window.openedArtifact`)!==selected.id)throw Error('Markdown 文件引用未解析')
  results.push({check:'model reference opens existing artifact',id:selected.id})

  await client.evaluate(`window.streamFixture.finish('[官方文档](https://example.com)',[])`)
  await waitFor(`!!document.querySelector('button[data-streamdown="link"]')`, '外部链接')
  await client.evaluate(`document.querySelector('button[data-streamdown="link"]').click()`)
  await waitFor(`!!document.querySelector('[role="dialog"]')`, '外部链接确认')
  results.push({check:'external links still require confirmation',passed:true})
  await client.evaluate(`window.streamFixture.finish(${JSON.stringify(agv.final)},${JSON.stringify(records)});document.documentElement.dataset.colorTheme='graphite'`)
  await pause(300)
  await client.evaluate(`const v=document.querySelector('.fox-thread-viewport');v.scrollTop=v.scrollHeight`)
  await screenshot('agv-final-files')
  writeFileSync(join(output,'report.json'),JSON.stringify({results,errors:client.errors},null,2))
  console.log(JSON.stringify({results,errors:client.errors}))
} catch(error){console.error(error);process.exitCode=1} finally {client?.close();browser?.kill();server?.kill()}
