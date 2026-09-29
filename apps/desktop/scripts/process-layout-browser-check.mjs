// Real Fox components and styles in a browser. This is not a live Host/model test.
import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('..', import.meta.url))
const output = fileURLToPath(new URL('../../../output/process-layout-browser/', import.meta.url))
const htmlPath = join(root, 'process-layout-check.html')
const entryPath = join(root, 'process-layout-check.tsx')
const port = 1451, cdpPort = 9357
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
const source = `import React from 'react';
import { createRoot } from 'react-dom/client';
import './src/styles/globals.css';
import './src/styles/workbench.css';
import './src/styles/workspace-pages.css';
import { RuntimeTimeline } from './src/features/chat/workbench';
import { answerDeltaFingerprint } from './src/features/conversations/model/runtime-delta-fingerprint';
import { persistProcessDisplayMode } from './src/features/chat/process-display-mode';
import { RunStatusText } from './src/features/chat/components/FoxAssistantAvatar';
const query = new URLSearchParams(location.search);
if(query.get('dark')) document.documentElement.classList.add('dark');
persistProcessDisplayMode('standard');
const live = query.get('live') === '1';
const start=Date.now()-62000;
const event=(seq:number,type:string,data:any={})=>({runId:'run',seq,eventType:type,event:{type,...data},createdAt:start+seq*1000});
const reply=(seq:number,text:string)=>event(seq,'message.delta',{deltaLength:text.length,deltaFingerprint:answerDeltaFingerprint(text)});
const events=[event(1,'run.started'),event(2,'reasoning.delta',{delta:'检查布局与滚动。'.repeat(50)+'\\n首行结束后正文保持独立。'}),reply(3,'阶段说明。'),event(4,'tool.started',{toolCallId:'read',tool:'read',input:{path:'README.md'}}),event(5,'tool.completed',{toolCallId:'read',tool:'read',result:{content:'ok'}}),reply(6,'最终答案保持可见。'),...(live?[]:[event(7,'run.completed')])];
const messages=[{id:'u',conversationId:'c',runId:'run',role:'user',kind:'text',content:'展示测试',status:'completed',ordinal:1,createdAt:start,updatedAt:start},{id:'assistant-run',conversationId:'c',runId:'run',role:'assistant',kind:'text',content:'阶段说明。最终答案保持可见。',status:live?'streaming':'completed',ordinal:2,createdAt:start,updatedAt:start+7000}];
if(query.has('shimmer')){events.splice(2);messages[1].content=''}
const artifacts=query.has('artifacts') ? [
  {id:'deliverable',conversationId:'c',runId:'run',displayName:'报告.docx',artifactType:'created_file',artifactClass:'deliverable',artifactOrigin:'project',storagePath:'C:/project/报告.docx',mediaType:null,byteSize:1024,sha256:null,status:'ready',createdAt:start,updatedAt:start},
  {id:'process',conversationId:'c',runId:'run',displayName:'计算.csv',artifactType:'created_file',artifactClass:'process',artifactOrigin:'project',storagePath:'C:/project/计算.csv',mediaType:null,byteSize:1024,sha256:null,status:'ready',createdAt:start,updatedAt:start},
] : [];
document.body.style.margin='0';
createRoot(document.getElementById('root')!).render(<div className="fox-conversation-content" style={{padding:16,maxWidth:900,margin:'0 auto'}}><RuntimeTimeline messages={messages as any} attachments={[]} artifacts={artifacts as any} events={events} runtimeRunning={live} activeRunId={live?'run':undefined} state={live?'streaming':'idle'} streamingText="" onRetry={()=>{}} onRerun={async()=>false}/>{query.has('shimmer')&&<div id="status-probe"><RunStatusText text="正在分析请求" active /></div>}</div>);
(window as any).__processCase={live,generation:crypto.randomUUID()};
`

async function connect() {
  let target
  for(let i=0;i<80;i++) {
    try { target=(await fetch(`http://127.0.0.1:${cdpPort}/json`).then(r=>r.json())).find(t=>t.type==='page'); if(target)break }catch{}
    await sleep(250)
  }
  if(!target)throw Error('test browser unavailable')
  const socket=new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((r,j)=>{socket.addEventListener('open',r,{once:true});socket.addEventListener('error',j,{once:true})})
  let id=0;const pending=new Map();const errors=[];const requests=new Map()
  socket.addEventListener('message',({data})=>{const m=JSON.parse(data),p=pending.get(m.id);if(m.method==='Network.requestWillBeSent')requests.set(m.params.requestId,m.params.request.url);if(['Network.loadingFinished','Network.loadingFailed'].includes(m.method))requests.delete(m.params.requestId);if(m.method==='Runtime.exceptionThrown')errors.push(m.params.exceptionDetails);if(m.method==='Runtime.consoleAPICalled'&&m.params.type==='error')errors.push(m.params.args);if(m.method==='Network.loadingFailed')errors.push(m.params);if(p){clearTimeout(p.timer);pending.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result)}})
  const send=(method,params={})=>new Promise((resolve,reject)=>{const n=++id;const timer=setTimeout(()=>{pending.delete(n);reject(Error('timeout '+method))},15000);pending.set(n,{resolve,reject,timer});socket.send(JSON.stringify({id:n,method,params}))})
  const evaluate=async expression=>{const r=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});if(r.exceptionDetails)throw Error(r.exceptionDetails.exception?.description||r.exceptionDetails.text);return r.result.value}
  return {send,evaluate,errors,requests,close:()=>socket.close()}
}

mkdirSync(output,{recursive:true})
writeFileSync(htmlPath,'<!doctype html><html><head><meta charset="utf-8"><title>Fox process layout check</title></head><body><div id="root"></div><script type="module" src="/process-layout-check.tsx"></script></body></html>')
writeFileSync(entryPath,source)
let vite,chrome,client
let browserProfile
let chromeDiagnostics=''
const results=[]
try {
  vite=spawn(process.execPath,[join(root,'node_modules/vite/bin/vite.js'),'--host','127.0.0.1','--port',String(port),'--strictPort'],{cwd:root,stdio:'ignore',windowsHide:true})
  browserProfile=mkdtempSync(join(output,'browser-profile-'))
  chrome=spawn(process.env.CHROME_PATH||'C:/Program Files/Google/Chrome/Application/chrome.exe',['--headless=new','--disable-gpu',`--remote-debugging-port=${cdpPort}`,'--no-first-run','--no-default-browser-check',`--user-data-dir=${browserProfile}`,'about:blank'],{stdio:['ignore','ignore','pipe'],windowsHide:true})
  chrome.stderr.on('data',chunk=>{chromeDiagnostics=(chromeDiagnostics+chunk.toString()).slice(-3000)})
  client=await connect();await client.send('Page.enable');await client.send('Runtime.enable');await client.send('Network.enable')
  let serving=false
  for(let i=0;i<80;i++){try{serving=(await fetch(`http://127.0.0.1:${port}/process-layout-check.html`)).ok;if(serving)break}catch{}await sleep(250)}
  if(!serving)throw Error('fixture dev server unavailable')
  for(const scenario of [{name:'wide',width:1100,live:false,dark:false},{name:'narrow-dark',width:360,live:false,dark:true},{name:'running',width:850,live:true,dark:false},{name:'artifacts',width:850,live:false,dark:false,artifacts:true}]) {
    await client.send('Emulation.setDeviceMetricsOverride',{width:scenario.width,height:900,deviceScaleFactor:1,mobile:false})
    const previous=await client.evaluate('window.__processCase?.generation')
    await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/process-layout-check.html?live=${scenario.live?1:0}&${scenario.dark?'dark=1':''}&${scenario.artifacts?'artifacts=1':''}`})
    let ready=false
    for(let i=0;i<300;i++) {ready=await client.evaluate(`window.__processCase?.generation!==${JSON.stringify(previous??null)} && !!document.querySelector('.fox-process-stage') && !!document.querySelector('.fox-process-turn-toggle')`);if(ready)break;await sleep(200)}
    if(!ready)throw Error(scenario.name+': fixture did not mount '+JSON.stringify(client.errors)+' pending='+JSON.stringify([...client.requests.values()])+' '+await client.evaluate('JSON.stringify({url:location.href,html:document.documentElement.outerHTML.slice(-1500),case:window.__processCase})'))
    // MarkdownResponse first shows a Suspense fallback. Its final paragraph
    // margins affect neighboring stages, so compare disclosure geometry only
    // after the real response has hydrated.
    await client.evaluate('document.fonts.ready.then(() => true)')
    let markdownReady=false
    for(let i=0;i<100;i++){markdownReady=await client.evaluate(`(()=>{const answers=[...document.querySelectorAll('.fox-answer-segment')];return answers.length>0&&answers.every(e=>!!e.querySelector('.fox-streamdown-response')&&!e.querySelector('.fox-markdown-loading'))})()`);if(markdownReady)break;await sleep(100)}
    if(!markdownReady)throw Error(scenario.name+': answer markdown did not hydrate')
    await client.evaluate(`(()=>{const b=document.querySelector('button.fox-process-turn-toggle');if(b?.getAttribute('aria-expanded')==='false')b.click()})()`)
    await sleep(100)
    const result=await client.evaluate(`(()=>{const rect=e=>e.getBoundingClientRect();const turn=document.querySelector('.fox-process-turn-toggle').closest('.fox-turn-anchor');const avatar=turn.querySelector('.fox-assistant-avatar');const toggle=turn.querySelector('.fox-process-turn-toggle');const stages=[...turn.querySelectorAll('.fox-process-stage')];const a=rect(avatar),b=rect(toggle),s=stages.map(rect);const footer=turn.querySelector('.fox-message-footer');const v={sameHeaderRow:Math.abs((a.top+a.height/2)-(b.top+b.height/2))<6,buttonRight:b.left>=a.right-1,allStagesBelow:s.every(r=>r.top>=Math.max(a.bottom,b.bottom)-1),stageAlignment:s.every(r=>Math.abs(r.left-s[0].left)<1),avatarCount:turn.querySelectorAll('.fox-assistant-avatar').length,footerVisible:!!footer&&rect(footer).height>0,finalVisible:[...turn.querySelectorAll('.fox-answer-segment')].some(e=>e.textContent.includes('最终答案')&&!e.hidden),overflow:document.documentElement.scrollWidth>innerWidth+1};return v})()`)
    for(const key of ['sameHeaderRow','buttonRight','allStagesBelow','stageAlignment','footerVisible','finalVisible'])if(!result[key])throw Error(scenario.name+': '+key+' '+JSON.stringify(result))
    if(result.avatarCount!==1||result.overflow)throw Error(scenario.name+': duplicate avatar or overflow '+JSON.stringify(result))
    if(scenario.name==='wide') {
      const measure=()=>client.evaluate(`(()=>{const stages=[...document.querySelectorAll('.fox-process-stage')];const headers=stages.map(stage=>stage.querySelector('.fox-chain-of-thought-header')).filter(Boolean);const rect=e=>e.getBoundingClientRect();const content=document.querySelector('.fox-process-turn-content');const head=stages[0].querySelector('.fox-runtime-process-head');return {positions:headers.map(h=>rect(h).top),stageHeights:stages.map(s=>rect(s).height),contentHeight:rect(content).height,headMargin:getComputedStyle(head).marginBottom}})()`)
      const setGroup=async open=>{await client.evaluate(`(()=>{const b=document.querySelector('.fox-process-stage .fox-chain-of-thought-header');if((b.getAttribute('aria-expanded')==='true')!==${open})b.click()})()`);await sleep(350)}
      const setRow=async open=>{await client.evaluate(`(()=>{const b=document.querySelector('.fox-process-stage .fox-runtime-step-trigger');if(!b)throw Error('missing first process row');if((b.getAttribute('aria-expanded')==='true')!==${open})b.click()})()`);await sleep(350)}
      const closedBase=await measure()
      await setGroup(true)
      const rowClosedBase=await measure()
      const cycles=[]
      for(let n=0;n<3;n++) {await setRow(true);await setRow(false);cycles.push({row:await measure()});await setGroup(false);cycles[n].group=await measure();await setGroup(true)}
      for(const [index,cycle] of cycles.entries()) {
        if(Math.abs(cycle.row.positions[1]-rowClosedBase.positions[1])>1 || Math.abs(cycle.row.stageHeights[0]-rowClosedBase.stageHeights[0])>1 || Math.abs(cycle.row.contentHeight-rowClosedBase.contentHeight)>1)throw Error('row collapse leaves whitespace '+JSON.stringify({index,rowClosedBase,cycle}))
        if(Math.abs(cycle.group.positions[1]-closedBase.positions[1])>1 || Math.abs(cycle.group.stageHeights[0]-closedBase.stageHeights[0])>1 || Math.abs(cycle.group.contentHeight-closedBase.contentHeight)>1 || cycle.group.headMargin!=='0px')throw Error('group collapse leaves whitespace '+JSON.stringify({index,closedBase,cycle}))
        if(index && (Math.abs(cycle.group.positions[1]-cycles[0].group.positions[1])>1 || Math.abs(cycle.row.positions[1]-cycles[0].row.positions[1])>1))throw Error('repeated toggles accumulate spacing '+JSON.stringify({index,cycles}))
      }
      if(closedBase.stageHeights.some(height=>height>32))throw Error('process headings are too loose '+JSON.stringify(closedBase))
      result.spacing={closedBase,rowClosedBase,cycles}
      await setGroup(false)
    }
    if(scenario.artifacts) {
      const clickFileTab=async label=>{
        const point=await client.evaluate(`(()=>{const b=[...document.querySelectorAll('.fox-message-file-results-row button')].find(e=>e.textContent.includes(${JSON.stringify(label)}));if(!b)throw Error('missing file tab');const r=b.getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height/2}})()`)
        await client.send('Input.dispatchMouseEvent',{type:'mousePressed',x:point.x,y:point.y,button:'left',clickCount:1})
        await client.send('Input.dispatchMouseEvent',{type:'mouseReleased',x:point.x,y:point.y,button:'left',clickCount:1})
        await sleep(100)
      }
      const fileState=()=>client.evaluate(`(()=>{const row=document.querySelector('.fox-message-file-results-row');const buttons=[...row.querySelectorAll('button')];const panels=[...document.querySelectorAll('.fox-message-artifact-group-content[role="region"]')];const panel=panels.find(e=>!e.hidden);const rect=e=>e.getBoundingClientRect();return {names:buttons.map(b=>b.textContent.trim()),expanded:buttons.map(b=>b.getAttribute('aria-expanded')),sameRow:Math.abs(rect(buttons[0]).top-rect(buttons[1]).top)<1,panelLabel:panel?.getAttribute('aria-label'),panelBelow:!panel||rect(panel).top>=rect(row).bottom-1,panelText:panel?.textContent||'',panelCount:panels.filter(e=>!e.hidden).length}})()`)
      const initial=await fileState()
      await clickFileTab('本次文件结果');const deliverable=await fileState()
      await clickFileTab('过程文件');const process=await fileState()
      await clickFileTab('过程文件');const closed=await fileState()
      if(!initial.sameRow||initial.panelCount||deliverable.expanded.join()!=='true,false'||!deliverable.panelBelow||!deliverable.panelText.includes('报告.docx')||deliverable.panelText.includes('计算.csv')||process.expanded.join()!=='false,true'||!process.panelBelow||!process.panelText.includes('计算.csv')||process.panelText.includes('报告.docx')||closed.panelCount||closed.expanded.join()!=='false,false')throw Error('artifact tab regression '+JSON.stringify({initial,deliverable,process,closed}))
      result.artifactTabs={initial,deliverable,process,closed}
    }
    const shot=await client.send('Page.captureScreenshot',{format:'png'});writeFileSync(join(output,scenario.name+'.png'),Buffer.from(shot.data,'base64'))
    await client.evaluate(`document.querySelector('button.fox-process-turn-toggle')?.click()`);await sleep(80)
    result.fold=await client.evaluate(`(()=>{const t=document.querySelector('.fox-process-turn-toggle').closest('.fox-turn-anchor');return {hidden:[...t.querySelectorAll('.fox-process-stage')].every(e=>e.hidden),final:[...t.querySelectorAll('.fox-answer-segment')].some(e=>e.textContent.includes('最终答案')&&!e.hidden),footer:t.querySelector('.fox-message-footer').getBoundingClientRect().height>0}})()`)
    if(!result.fold.hidden||!result.fold.final||!result.fold.footer)throw Error(scenario.name+': collapse '+JSON.stringify(result.fold))
    results.push({scenario:scenario.name,...result})
  }
  const generation=await client.evaluate('window.__processCase.generation')
  await client.send('Emulation.setDeviceMetricsOverride',{width:360,height:900,deviceScaleFactor:1,mobile:false})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/process-layout-check.html?live=1&shimmer=1`})
  let shimmerReady=false
  for(let i=0;i<300;i++){shimmerReady=await client.evaluate(`window.__processCase?.generation!==${JSON.stringify(generation)} && !!document.querySelector('#status-probe') && !!document.querySelector('.fox-chain-of-thought-header')`);if(shimmerReady)break;await sleep(200)}
  if(!shimmerReady)throw Error('shimmer fixture did not mount')
  for(let i=0;i<100;i++){
    if(await client.evaluate(`!!document.querySelector('.fox-run-status-step')`))break
    await client.evaluate(`(()=>{const b=document.querySelector('.fox-process-stage .fox-chain-of-thought-header');if(b?.getAttribute('aria-expanded')!=='true')b?.click()})()`)
    await sleep(100)
  }
  if(!await client.evaluate(`!!document.querySelector('.fox-run-status-step')`))throw Error('missing reasoning row '+await client.evaluate(`JSON.stringify({headers:[...document.querySelectorAll('.fox-chain-of-thought-header')].map(e=>({text:e.textContent,expanded:e.getAttribute('aria-expanded')})),stages:document.querySelectorAll('.fox-process-stage').length,steps:document.querySelectorAll('.fox-runtime-step-row').length,html:document.querySelector('.fox-turn-anchor')?.outerHTML.slice(0,1800)})`))
  const shimmer=await client.evaluate(`(()=>{const row=document.querySelector('.fox-run-status-step');const label=row.querySelector(':scope > .fox-run-status-viewport > .fox-run-status-label');const copy=row.querySelector('.fox-row-shimmer-highlight .fox-run-status-label');const prefix=row.querySelector(':scope > .fox-run-status-prefix');const values=[row,document.querySelector('#status-probe .fox-row-shimmer')].map(box=>{const sweep=box.querySelector('.fox-row-shimmer-sweep'),highlight=box.querySelector('.fox-row-shimmer-highlight');return {sweep:getComputedStyle(sweep).animationName,highlight:getComputedStyle(highlight).animationName,decorationHidden:box.querySelector('.fox-row-shimmer-decoration').getAttribute('aria-hidden')==='true',inert:box.querySelector('.fox-row-shimmer-decoration').hasAttribute('inert')}});return {values,title:prefix.textContent,titleMask:getComputedStyle(prefix).maskImage,scroll:label.scrollLeft,copyScroll:copy?.scrollLeft,bodyCount:row.textContent.split('检查布局').length-1}})()`)
  if(shimmer.values.some(v=>v.sweep!=='fox-row-shimmer-sweep'||v.highlight!=='fox-row-shimmer-highlight'||!v.decorationHidden||!v.inert)||shimmer.titleMask!=='none'||shimmer.scroll<=0||Math.abs(shimmer.scroll-shimmer.copyScroll)>1||shimmer.bodyCount!==50)throw Error('shimmer regression '+JSON.stringify(shimmer))
  results.push({scenario:'shimmer-narrow',...shimmer})
  const shimmerShot=await client.send('Page.captureScreenshot',{format:'png'});writeFileSync(join(output,'shimmer-narrow.png'),Buffer.from(shimmerShot.data,'base64'))
  writeFileSync(join(output,'result.json'),JSON.stringify({results},null,2));rmSync(join(output,'failure.json'),{force:true});console.log(JSON.stringify({results},null,2))
} catch(error) {writeFileSync(join(output,'failure.json'),JSON.stringify({error:String(error),chromeExit:chrome?.exitCode,chromeDiagnostics,results},null,2));console.error(error,'chrome exit',chrome?.exitCode,'stderr',chromeDiagnostics);process.exitCode=1}
finally {client?.close();chrome?.kill();vite?.kill();for(const file of [htmlPath,entryPath])rmSync(file,{force:true});if(browserProfile?.startsWith(output)){if(chrome?.exitCode===null)await new Promise(resolve=>{chrome.once('exit',resolve);setTimeout(resolve,3000)});try{rmSync(browserProfile,{recursive:true,force:true,maxRetries:10,retryDelay:250})}catch{console.warn('browser profile cleanup pending:',browserProfile)}}}

