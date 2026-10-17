import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/fox-motion-20261016/browser')
const port = 1475, cdpPort = 9381
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
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/tests/fixtures/fox-motion.html?${params}` })
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
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/tests/fixtures/fox-motion.html`)).ok) { serverReady = true; break } } catch {} await pause(150) }
  if (!serverReady) throw new Error('组件夹具服务未启动')


  await client.send('Emulation.setDeviceMetricsOverride',{width:1100,height:750,deviceScaleFactor:1,mobile:false})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/fox-motion.html`})
  await waitFor(`document.querySelectorAll('[data-animated="true"]').length===13`, 'Fox 动效')
  const probe=await client.evaluate(`(()=>{const fixture=window.motionFixture; const svg=document.querySelector('#engine-probe');const engine=new fixture.BlueFoxEngine(svg,{followPointer:false,autoTricks:false,reduceMotion:false,initialState:'thinking',thinkingStartedAt:Date.now()-13000});engine.setPaused(true);window.probe=engine;return {initial:engine.snapshot(),overlay:engine.overlay.x,viewBox:svg.getAttribute('viewBox')}})()`)
  if(probe.overlay!==1||probe.initial.overlay!=='dots')throw Error(JSON.stringify(probe))
  results.push({check:'first painted frame restores current compact state',...probe})
  const geometry=await client.evaluate(`(()=>{const engine=window.probe, svg=engine.svg; engine.thinkingStartedAt=undefined; const samples=[];for(const state of ['thinking','sending','receiving','orbit','radar','loading','writing']){engine.setState(state);engine._stepOverlay(engine.stateAt);engine.overlay.x=engine.overlay.t;engine.overlayMix.x=1;engine.ovPrev=null;for(let i=0;i<24;i++){const now=engine.stateAt+i*100;engine._paint(now,0,0,false,null);const box=svg.getBoundingClientRect();for(const node of [...engine.fxLayer.parts,...engine.fxLayer.dots,...engine.fxLayer.rings]){if(node.style.display==='none'||Number(node.getAttribute('opacity')??1)<.2)continue;const r=node.getBoundingClientRect();if(r.width<.5||r.height<.5)continue;samples.push({state,inside:r.left>=box.left-1&&r.right<=box.right+1&&r.top>=box.top-1&&r.bottom<=box.bottom+1,width:r.width});}}}engine.setState('thinking');const variants=[0,6100,12100,18100].map(t=>{engine._stepOverlay(engine.stateAt+t);return engine.overlay.t ? engine.ovKind : "face"});return {outside:samples.filter(x=>!x.inside),variants,samples:samples.length,receiveMax:Math.max(...samples.filter(x=>x.state==='receiving').map(x=>x.width))}})()`)
  if(geometry.outside.length||new Set(geometry.variants).size!==3||geometry.receiveMax<4)throw Error(JSON.stringify(geometry))
  results.push({check:'animation bounds and thinking variants',...geometry})
  for(const dark of [false,true])for(const theme of ['graphite','paper','mist']){
    const themeState=await client.evaluate(`new Promise(resolve=>{const svg=document.querySelector('[data-case="thinking"] [data-avatar-engine]');document.documentElement.dataset.colorTheme='${theme}';document.documentElement.classList.toggle('dark',${dark});let frame=0;const widths=[];function next(){widths.push(svg.viewBox.baseVal.width);if(++frame<20)requestAnimationFrame(next);else resolve({same:svg===document.querySelector('[data-case="thinking"] [data-avatar-engine]'),max:Math.max(...widths)})}requestAnimationFrame(next)})`)
    if(!themeState.same||themeState.max>680)throw Error(JSON.stringify(themeState))
    results.push({check:'live theme preserves compact avatar',theme,dark,...themeState})
  }
  await client.evaluate('window.motionFixture.remount()')
  await waitFor(`document.querySelector('[data-case="thinking"] [data-avatar-engine]').viewBox.baseVal.width<680`, '重挂载恢复圆点')
  await client.send('Emulation.setEmulatedMedia',{features:[{name:'prefers-reduced-motion',value:'reduce'}]})
  await pause(200)
  const reduced=await client.evaluate(`(()=>{const svg=document.querySelector('[data-case="thinking"] [data-avatar-engine]');return {width:svg.viewBox.baseVal.width,animated:svg.parentElement.dataset.animated}})()`)
  if(reduced.animated!=='false'||reduced.width!==720)throw Error(JSON.stringify(reduced))
  results.push({check:'reduced motion retains a still thinking face',...reduced})
  await client.send('Emulation.setEmulatedMedia',{features:[]})
  await client.evaluate(`(()=>{const f=window.motionFixture;f.update({...f.detail,runtimeEvents:[{runId:'r',seq:1,eventType:'run.phase',createdAt:Date.now(),event:{phase:'request_sent'}}]})})()`)
  await waitFor(`!!document.querySelector('.fox-run-status-bar [data-animation="sending"]')`, '发送动效')
  await waitFor(`!!document.querySelector('.fox-run-status-bar [data-animation="receiving"]')`, '接收动效')
  for(const [tool,animation] of [['web_search','radar'],['exec_command','orbit'],['write_file','writing'],['read_file','searching']]){
    await client.evaluate(`(()=>{const f=window.motionFixture;f.update({...f.detail,runtimeEvents:[{runId:'r',seq:2,eventType:'tool.started',createdAt:Date.now(),event:{toolCallId:'t',tool:'${tool}'}}]})})()`)
    await waitFor(`!!document.querySelector('.fox-run-status-bar [data-animation="${animation}"]')`, tool)
  }
  results.push({check:'production status strip sending receiving and four tool animations',passed:true})
  await client.evaluate(`(()=>{const f=window.motionFixture;f.update({...f.detail,runtimeEvents:[{runId:'r',seq:3,eventType:'reasoning.delta',createdAt:Date.now(),event:{delta:'分析'}}]})})()`)
  await client.evaluate(`document.documentElement.dataset.colorTheme='graphite';document.documentElement.classList.remove('dark')`)
  await screenshot('motion-states')
  if(client.errors.length)throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,'report.json'),JSON.stringify({passed:true,results,errors:client.errors},null,2))
  console.log(JSON.stringify({passed:true,results}))
} catch(error){console.error(error);process.exitCode=1} finally {client?.close();browser?.kill();server?.kill()}
