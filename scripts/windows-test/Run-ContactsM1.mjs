// Node 24+. The installed model service is used only with --live-profile.
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { createReadStream, closeSync, openSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, realpath, stat, truncate, unlink, writeFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { fileURLToPath } from 'node:url'
import { setTimeout as sleep } from 'node:timers/promises'
import { checkContactRecords } from './contacts-m1-records.mjs'

const repo = fileURLToPath(new URL('../../', import.meta.url))
const argv = process.argv.slice(2)
let liveProfile
if (argv.at(-2) === '--live-profile') liveProfile = argv.splice(-2)[1]
const dev = argv.at(-1) === '--dev'
if (dev) argv.pop()
if (argv.length && !(argv.length === 2 && argv[0] === '--exe') && !(argv.length === 4 && argv[0] === '--exe' && argv[2] === '--sidecar')) {
  throw new Error('Usage: node scripts/windows-test/Run-ContactsM1.mjs [--exe candidate.exe [--sidecar candidate-runtime.exe] [--dev] [--live-profile installed-fox.db]]')
}
const directory = await mkdtemp(join(tmpdir(), 'fox-contacts-m1-'))
const evidence = { revision: spawnSync('git', ['rev-parse', 'HEAD'], {cwd:repo,encoding:'utf8'}).stdout?.trim(), started: new Date().toISOString(), checks: [] }
console.log('Isolation directory:', directory)
const children = []
let socket, db
const timeout = 90000
const check = async (name, action) => {
  try { const facts = await action(); evidence.checks.push({name, status:'passed', facts}) }
  catch (error) { evidence.checks.push({name, status:'failed', error:error.message}); throw error }
}

async function poll(label, action, waitMs = timeout) {
  const until = Date.now() + waitMs
  let last = ''
  while (Date.now() < until) {
    try { const value = await action(); if (value) return value } catch (error) { last = error.message }
    const stopped = children.find(p => p.exitCode !== null || p.spawnError)
    if (stopped) throw new Error(label + ': child stopped (' + (stopped.spawnError ?? stopped.exitCode) + ')')
    await sleep(250)
  }
  throw new Error(label + ' timed out. ' + last)
}

async function port() {
  const listener = createServer()
  await new Promise((done,fail) => { listener.once('error',fail); listener.listen(0,'127.0.0.1',done) })
  const number = listener.address().port
  await new Promise(done => listener.close(done))
  return number
}

async function sha256(path) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest('hex')
}

function start(command, args, env, label, cwd = dirname(command), ipc = false) {
  const log = openSync(join(directory, label + '.log'),'wx')
  try {
    const child = spawn(command,args,{cwd,env,windowsHide:true,stdio:ipc ? ['ignore',log,log,'ipc'] : ['ignore',log,log]})
    child.once('error', error => { child.spawnError = error.message })
    children.push(child)
    return child
  } finally {closeSync(log)}
}

function stop(child) {
  if(!child.pid || child.exitCode !== null || child.killed) return
  if(process.platform === 'win32') spawnSync('taskkill',['/PID',String(child.pid),'/T','/F'],{windowsHide:true,stdio:'ignore'})
  else child.kill()
}

async function attach(cdp) {
  const target = await poll('isolated WebView',async () => {
    const views = await (await fetch('http://127.0.0.1:' + cdp + '/json', {signal:AbortSignal.timeout(2000)})).json()
    const origins = dev ? ['http://127.0.0.1:1421'] : ['http://tauri.localhost','https://tauri.localhost','tauri://localhost']
    return views.find(v => v.type === 'page' && origins.some(origin => v.url === origin || v.url.startsWith(origin + '/')))
  })
  const ws = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((done,fail) => {ws.addEventListener('open',done,{once:true});ws.addEventListener('error',fail,{once:true})})
  socket = ws
  let sequence = 0
  const pending = new Map()
  ws.addEventListener('message', ({data}) => {const item = JSON.parse(data); pending.get(item.id)?.(item)})
  return async (command, args = {}) => {
    const id = ++sequence
    const expression = 'window.__TAURI_INTERNALS__.invoke(' + JSON.stringify(command) + ',' + JSON.stringify(args) + ')'
    const result = await new Promise((done,fail) => {
      const timer = setTimeout(() => {pending.delete(id);fail(new Error(command + ' timed out'))}, timeout)
      pending.set(id, item => {clearTimeout(timer);pending.delete(id);done(item)})
      ws.send(JSON.stringify({id,method:'Runtime.evaluate',params:{expression,awaitPromise:true,returnByValue:true}}))
    })
    if (result.error || result.result?.exceptionDetails) throw new Error(command + ': ' + JSON.stringify(result.error ?? result.result.exceptionDetails))
    const resultData = result.result.result.value
    assert.equal(resultData?.ok,true, command + ': ' + JSON.stringify(resultData?.error))
    return resultData.data
  }
}

async function desktop() {
  assert.equal(process.platform,'win32','The candidate desktop must be run on Windows')
  const exe = await realpath(argv[1])
  const sidecar = argv[3] ? await realpath(argv[3]) : undefined
  evidence.desktop = {file:basename(exe),sha256:await sha256(exe),sidecar:sidecar && {file:basename(sidecar),sha256:await sha256(sidecar)}}
  const data = join(directory,'data'), home = join(directory,'home')
  const profile = {FOX_DATA_DIR:data,USERPROFILE:home,HOME:home,APPDATA:join(home,'Roaming'),LOCALAPPDATA:join(home,'Local'),
    TEMP:join(directory,'tmp'),TMP:join(directory,'tmp'),WEBVIEW2_USER_DATA_FOLDER:join(data,'webviews'),
    PI_CODING_AGENT_DIR:join(home,'pi'),NO_PROXY:'127.0.0.1,localhost',FOX_EXTERNAL_BACKENDS:'off'}
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    /^(path|systemroot|systemdrive|windir|comspec|pathext|programfiles|programfiles\(x86\)|programw6432|programdata|commonprogramfiles|commonprogramfiles\(x86\)|processor_architecture)$/i.test(key)))
  Object.assign(env,profile)
  for (const folder of [data,profile.TEMP,profile.APPDATA,profile.LOCALAPPDATA,profile.PI_CODING_AGENT_DIR]) await mkdir(folder,{recursive:true})
  const outside = join(directory,'outside.txt'), proof = join(directory,'proof.txt')
  await writeFile(outside,'FOX_OUTSIDE_PROOF_31'); await writeFile(proof,'FOX_PRIVATE_PROOF_31')
  const provider = await port(), endpoint = 'http://127.0.0.1:' + provider
  start(process.execPath,[join(repo,'scripts/kernel-desktop-acceptance-provider.mjs')],{...env,FOX_ACCEPTANCE_PROVIDER_PORT:String(provider),FOX_CONTACTS_ACCEPTANCE:'1',FOX_CONTACTS_OUTSIDE:outside},'provider')
  const health = async () => (await fetch(endpoint + '/health',{signal:AbortSignal.timeout(2000)})).json()
  await poll('loopback provider',health)
  let bridge
  if(dev) {
    const desktop = join(repo,'apps/desktop')
    const probe = createServer()
    await new Promise((ok,fail) => { probe.once('error',fail); probe.listen(1421,'127.0.0.1',ok) })
    await new Promise(ok => probe.close(ok))
    bridge = start(process.execPath,[join(repo,'scripts/windows-test/contacts-m1-dev-webview.mjs')],env,'vite',desktop,true)
    bridge.on('message', item => { if(item.ready) bridge.ready = Date.now() })
    await poll('candidate frontend',async () => (await fetch('http://127.0.0.1:1421/')).ok)
  }
  const cdp = await port()
  if (sidecar) env.FOX_RUNTIME_EXECUTABLE = sidecar
  const appEnv = dev ? env : {...env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-address=127.0.0.1 --remote-debugging-port=' + cdp}
  let app = start(exe,[],appEnv,'desktop',repo)
  console.log('Candidate pid:',app.pid,dev ? 'isolated Vite/WebView IPC' : 'CDP: ' + cdp)
  let invoke
  if(bridge) {
    await poll('candidate WebView IPC connection',() => bridge.ready && Date.now() - bridge.ready >= 2500)
    invoke = (command,args = {}) => new Promise((done,fail) => {
      const id = randomUUID()
      const timer = setTimeout(() => {bridge.off('message',reply);fail(new Error(command + ' timed out'))},timeout)
      function reply(item) {
        if(item.id !== id) return
        clearTimeout(timer);bridge.off('message',reply)
        if(item.error || !item.value?.ok) return fail(new Error(command + ': ' + JSON.stringify(item.error ?? item.value?.error)))
        done(item.value.data)
      }
      bridge.on('message',reply);bridge.send({id,command,args})
    })
  } else invoke = await attach(cdp)
  const list = await poll('contacts command',() => invoke('companions_list'))
  const builtin = list.companions.find(c => c.isDefault)
  assert.ok(builtin)
  assert.equal((await realpath(builtin.workspaceDir)).toLowerCase(),(await realpath(join(data,'companions',builtin.id,'workspace'))).toLowerCase(),'Client is attached to a different data directory')
  db = new DatabaseSync(join(data,'fox.db'),{readOnly:true})
  const chat = builtin.conversationId
  const expert = await invoke('companion_add_from_expert',{request:{expertId:'fox-debugger'}})
  if (liveProfile) {
    const { checkLiveModel } = await import('./contacts-live-model.mjs')
    const filter = process.env.FOX_CONTACTS_LIVE_FILTER?.trim() || null
    evidence.liveSelection = { scope: filter ? 'partial' : 'full', filter, selected: [], skipped: [] }
    evidence.liveModel = await checkLiveModel({ source: liveProfile, invoke, db, check, builtin, expert, directory, poll, endpoint, selection: evidence.liveSelection })
    return
  }
  await check('A01/A02 isolated startup and duplicate add',async () => {
    assert.equal(list.companions.filter(c => c.isDefault).length,1)
    const again = await invoke('companion_add_from_expert',{request:{expertId:'fox-debugger'}})
    assert.equal(again.id,expert.id)
    const row = db.prepare('SELECT project_id,project_root,full_access FROM conversations WHERE id=?').get(chat)
    assert.equal(row.project_id,null); assert.equal(row.full_access,0); assert.equal(row.project_root,builtin.workspaceDir)
    const schema = db.prepare('SELECT MAX(version) AS version FROM schema_migrations').get().version
    assert.equal(schema,106)
    return {schema,projectRoot:row.project_root}
  })
  const dropped = { filename: basename(proof), mediaType: 'text/plain', dataUrl: 'data:text/plain;base64,' + (await readFile(proof)).toString('base64') }
  const upload = (conversationId, files) => invoke('attachments_save', { request: { conversationId, files } })
  const [attachment] = await upload(chat, [dropped])
  await check('A04/A13 composer attachments survive contact deletion as independent workspace copies', async () => {
    const [other] = await upload(expert.conversationId, [dropped])
    const [duplicate] = await invoke('companion_import_files', { companionId: builtin.id, paths: [proof] })
    const [renamed] = await upload(chat, [dropped])
    assert.equal(basename(duplicate), 'proof (1).txt')
    assert.equal(renamed.displayName, 'proof (2).txt')
    const copy = join(builtin.workspaceDir, '收到的文件', attachment.displayName)
    const otherCopy = join(expert.workspaceDir, '收到的文件', other.displayName)
    assert.equal(await readFile(copy, 'utf8'), 'FOX_PRIVATE_PROOF_31')
    await writeFile(otherCopy, '专家独立修改')
    assert.equal(await readFile(other.storagePath, 'utf8'), 'FOX_PRIVATE_PROOF_31')
    const invalid = { ...dropped, filename: '../outside.txt' }
    await assert.rejects(() => upload(chat, [{ ...dropped, filename: 'rollback.txt' }, invalid]), /attachment.invalid_name/)
    await assert.rejects(() => stat(join(builtin.workspaceDir, '收到的文件', 'rollback.txt')), { code: 'ENOENT' })
    const {planHash} = await invoke('companion_delete_preview', {companionId:expert.id})
    const removed = await invoke('companion_delete', {companionId:expert.id,planHash})
    assert.equal(await readFile(otherCopy, 'utf8'), '专家独立修改')
    await assert.rejects(() => stat(other.storagePath), { code: 'ENOENT' })
    assert.equal(await readFile(proof, 'utf8'), 'FOX_PRIVATE_PROOF_31')
    assert.equal(await readFile(copy, 'utf8'), 'FOX_PRIVATE_PROOF_31')
    assert.ok((await invoke('companion_retained_list')).some(item => item.companionId === expert.id))
    return { keptWorkspace: expert.workspaceDir, files: removed.plan.workspaceFiles, renamed: renamed.displayName }
  })
  const baseUrl = endpoint + '/' + randomUUID() + '/v1'
  const recordsExpert = await invoke('companion_add_from_expert',{request:{expertId:'fox-debugger'}})
  await checkContactRecords({invoke,db,check,expert:recordsExpert,builtin,directory})
  await invoke('model_service_save',{request:{name:'Contacts local acceptance',baseUrl,modelId:'fox-kernel-acceptance',
    apiType:'openai-completions',contextWindow:32000,maxOutputTokens:1024}})
  const send = async (text, attachmentIds = []) => {
    const {run} = await invoke('run_start',{request:{conversationId:chat,text,attachmentIds}})
    const row = await poll('PiNative run ' + run.id,() => {
      const r = db.prepare('SELECT status,error_code FROM runs WHERE id=?').get(run.id)
      return !['queued','running','cancelling','awaiting_confirmation'].includes(r.status) && r
    })
    assert.equal(row.status,'completed',JSON.stringify(row))
    const frozen = JSON.parse(db.prepare('SELECT binding_json FROM run_control_bindings WHERE run_id=?').get(run.id).binding_json)
    assert.equal(frozen.executorRoute,'pi_native')
    assert.equal(frozen.permission.projectRoot,builtin.workspaceDir)
    return run.id
  }
  await check('A07/A13/A25 actual model request and root-bound read',async () => {
    const run = await send('[CONTACTS_M1_READ] 请读取收到的文件中的 proof.txt。', [attachment.id])
    const facts = await health()
    assert.equal(facts.model,'fox-kernel-acceptance')
    assert.equal(facts.contactReadEvidence,true)
    assert.ok(facts.availableTools.includes('read'))
    for (const tool of ['git_read','sqlite_read','run_command','http_request','memory_propose']) assert.ok(!facts.availableTools.includes(tool),tool)
    assert.ok(db.prepare("SELECT 1 FROM kernel_tool_calls WHERE run_id=? AND tool='read' AND state='completed'").get(run))
    return {model:facts.model,tools:facts.availableTools}
  })
  await check('A13 reject outside file at Host admission',async () => {
    const run = await send('[CONTACTS_M1_OUTSIDE] 尝试读取工作目录外的文件。')
    assert.equal((await health()).outsideContentReturned,false)
    const receipts = db.prepare('SELECT tool,state,result_json FROM kernel_tool_calls WHERE run_id=? ORDER BY created_at,source_order').all(run)
    assert.deepEqual(receipts.map(row => row.tool),['read','ls','find','grep','present_files','write_file','edit_file'])
    for (const row of receipts) {
      const result = JSON.parse(row.result_json)
      assert.equal(result.isError,true,row.tool)
      if (['read','present_files','write_file','edit_file'].includes(row.tool)) {
        assert.equal(row.state,'not_started',row.tool)
        assert.equal(result.errorDetails.code,'companion.path_blocked',row.tool)
      }
    }
    assert.equal(await readFile(outside,'utf8'),'FOX_OUTSIDE_PROOF_31')
    return {refused:receipts.map(({tool,state}) => ({tool,state}))}
  })
  if(dev) await check('A20 zero-byte refusal, confirmed repair and safe allocated takeover',async () => {
    let restart = 0
    const fixture = join(data,'fox.db')
    const mapping = () => db.prepare("SELECT runtime_session_id,native_session_path,status,context_generation FROM runtime_sessions WHERE conversation_id=? AND runtime_type='pi'").get(chat)
    const history = () => db.prepare('SELECT COUNT(*) AS n FROM messages WHERE conversation_id=?').get(chat).n
    async function coldAllocated(empty) {
      const current = mapping()
      const path = await realpath(current.native_session_path)
      assert.ok(path.toLowerCase().startsWith((await realpath(join(data,'runtime-sessions'))).toLowerCase() + '\\'))
      stop(app)
      const at = children.indexOf(app); if(at >= 0) children.splice(at,1)
      if(empty) await truncate(path,0); else await unlink(path)
      const writeDb = new DatabaseSync(fixture)
      try {
        assert.equal(writeDb.prepare("UPDATE runtime_sessions SET status='allocated' WHERE conversation_id=? AND runtime_type='pi'")
          .run(chat).changes,1)
      } finally {writeDb.close()}
      bridge.ready = 0
      app = start(exe,[],appEnv,'desktop-restart-' + ++restart,repo)
      await poll('restarted WebView',() => bridge.ready && Date.now()-bridge.ready>=1000)
      return {current,path}
    }
    const request = {request:{conversationId:chat}}
    await assert.rejects(() => invoke('companion_context_repair',request),/companion\.repair_not_needed/)
    const before = history()
    const {current,path} = await coldAllocated(true)
    assert.equal((await invoke('conversation_load',request)).contextRepair.available,true)
    await assert.rejects(() => invoke('run_start',{request:{conversationId:chat,text:'[CONTACTS_M1_READ]'}}),/session\.native_not_persisted/)
    assert.equal(history(),before)
    assert.equal(mapping().status,'allocated')
    assert.equal(mapping().context_generation,current.context_generation)
    assert.equal((await stat(path)).size,0)
    await invoke('companion_context_repair',request)
    assert.equal(mapping().context_generation,current.context_generation+1)
    assert.equal(history(),before)
    await send('[CONTACTS_M1_READ] 检查重新连接后的文件。')
    assert.ok(db.prepare('SELECT 1 FROM runtime_session_generations WHERE conversation_id=? AND context_generation=?')
      .get(chat,current.context_generation))
    const normal = await coldAllocated(false)
    assert.equal((await invoke('conversation_load',request)).contextRepair.available,false)
    await assert.rejects(() => invoke('companion_context_repair',request),/companion\.repair_not_needed/)
    await send('[CONTACTS_M1_READ] 检查同代次接管后的文件。')
    assert.equal(mapping().context_generation,normal.current.context_generation)
    return {before,confirmedGeneration:current.context_generation+1,takeoverGeneration:mapping().context_generation}
  })
}

try {
  await check('JS protocol generation and companion selector',() => {
    for(const args of [['scripts/generate-engine-protocol.mjs','--check'],['--test','packages/fox-engine-protocol/test/wire-contract.test.mjs','services/agent-runtime/test/companion-tools.test.mjs']]) {
      const r = spawnSync(process.execPath,args,{cwd:repo,encoding:'utf8',windowsHide:true})
      process.stdout.write(r.stdout ?? ''); process.stderr.write(r.stderr ?? '')
      assert.equal(r.status,0,args.join(' '))
    }
  })
  if(argv.length) await desktop()
  else evidence.desktop = {status:'not_run',reason:'No Windows candidate executable supplied'}
} catch(error) {
  evidence.error = error.message
  process.exitCode = 1
} finally {
  db?.close();socket?.close()
  for(const child of children.reverse()) stop(child)
  evidence.finished = new Date().toISOString()
  await writeFile(join(directory,'evidence.json'),JSON.stringify(evidence,null,2))
  console.log(JSON.stringify({directory,...evidence},null,2))
}
