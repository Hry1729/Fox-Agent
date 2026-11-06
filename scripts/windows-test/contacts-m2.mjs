import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { mkdir, readFile, realpath, unlink, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { connectLiveModel } from './contacts-live-model.mjs'
import { SessionManager } from '../../services/agent-runtime/src/pi-adapter.mjs'

export async function checkM2(context) {
  const { invoke, db, check, expert, directory, restartDesktop } = context
  const { model, exchange } = await connectLiveModel(context)
  const chat = expert.conversationId
  const owner = { companionId: expert.id }
  const request = { request: { conversationId: chat } }
  const mapping = () => db.prepare("SELECT runtime_session_id,native_session_path,context_generation FROM runtime_sessions WHERE conversation_id=? AND runtime_type='pi'").get(chat)
  const history = () => db.prepare('SELECT id,content FROM messages WHERE conversation_id=? ORDER BY ordinal').all(chat)
  const turn = text => exchange(chat, text)
  const challenge = 'CX-' + randomUUID().slice(0, 8)
  const remember = '我们开始核对临时台账。本次会话中唯一需要持续保留的验收交接号为 ' + challenge + '。不要使用工具或写入笔记，只回复已收到。'
  const recall = '刚才处理临时台账时，我让你保留的验收交接号是什么？请引用本次对话记录中的完整交接号，不用工具。'
  const verifyRecall = async () => {
    const result = await turn(recall)
    assert.ok(result.reply.includes(challenge), JSON.stringify(result))
    assert.equal(result.tools.length, 0, JSON.stringify(result))
    return result
  }

  await check('A14 重启后沿用原生会话并回忆对话', async () => {
    const first = await turn(remember)
    assert.equal(first.tools.length, 0)
    const before = mapping()
    const native = await readFile(before.native_session_path)
    await restartDesktop()
    assert.deepEqual(mapping(), before)
    assert.deepEqual(await readFile(before.native_session_path), native)
    const reply = await verifyRecall()
    assert.deepEqual(mapping(), before)
    return { session: before, run: reply.run, reply: reply.reply }
  })

  await check('A19 归档拒绝发送，恢复后继续原会话', async () => {
    const before = mapping()
    await invoke('companion_archive', { request: owner })
    await assert.rejects(() => invoke('run_start', { request: { conversationId: chat, text: '归档期间拒绝' } }), /companion.archived/)
    await invoke('companion_restore', { request: owner })
    const reply = await verifyRecall()
    assert.deepEqual(mapping(), before)
    return { run: reply.run, session: before.runtime_session_id }
  })

  await check('A16 主动重置只更新代次，保留历史、笔记、文件和授权', async () => {
    const note = 'NT-' + randomUUID().slice(0, 8)
    await invoke('companion_note_save_user', { ...owner, content: '验收部门识别码为 ' + note })
    await invoke('companion_topic_save', { ...owner, title: '验收专题', content: '保留验收专题原文' })
    const shared = await invoke('memory_create', { request: { scope: 'global', kind: 'preference', canonicalKey: 'units', content: '我习惯用毫米标记尺寸。' } })
    const sharedState = () => db.prepare('SELECT state,enabled,content FROM memory_entities WHERE id=?').get(shared.id)
    const file = join(directory, '重置验收材料.txt')
    await writeFile(file, '重置后保留的原始材料')
    const [owned] = await invoke('companion_import_files', { ...owner, paths: [file] })
    const projectRoot = join(directory, 'm2-project')
    await mkdir(projectRoot)
    const project = await invoke('conversation_create', { request: { agentId: 'fox-general', expertId: expert.templateAgentId, projectRoot } })
    await invoke('companion_grant_project', { ...owner, projectId: project.projectId })
    const original = mapping()
    const messages = history()
    const notes = await invoke('companion_notes_get', owner)
    const grants = await invoke('companion_grants_list', owner)
    const originalShared = sharedState()
    assert.ok(originalShared?.enabled)
    const bytes = await readFile(original.native_session_path)
    assert.equal((await invoke('conversation_load', request)).contextReset.supported, true)
    await invoke('conversation_reset_context', request)
    const next = mapping()
    assert.equal(next.context_generation, original.context_generation + 1)
    assert.notEqual(next.runtime_session_id, original.runtime_session_id)
    assert.deepEqual(await readFile(original.native_session_path), bytes)
    assert.deepEqual(history(), messages)
    assert.deepEqual(sharedState(), originalShared)
    assert.deepEqual(await invoke('companion_notes_get', owner), notes)
    assert.deepEqual(await invoke('companion_grants_list', owner), grants)
    assert.equal(await readFile(owned, 'utf8'), '重置后保留的原始材料')
    const reply = await turn('我在你的主笔记中填写的验收部门识别码是什么？只回复完整识别码。')
    assert.ok(reply.reply.includes(note), JSON.stringify(reply))
    return { original, next, keptMessages: messages.length, project: project.projectId, run: reply.run, reply: reply.reply }
  })
  await invoke('model_service_save', {request: {
    name:'Contacts M2 local fault injection', baseUrl:context.endpoint + '/' + randomUUID() + '/v1',
    modelId:'fox-kernel-acceptance', apiType:'openai-completions', contextWindow:32000, maxOutputTokens:1024
  }})
  await checkReadyFaults(context, chat, mapping, history)
  await checkCompactedContact(context)
  return { model }
}

export async function checkCompactedContact(context) {
  const { invoke, db, check, restartDesktop } = context
  const { exchange } = await connectLiveModel(context, { contextWindow: 24576, maxOutputTokens: 4096 })
  const { companions } = await invoke('companions_list')
  const chat = companions.find(contact => contact.isDefault).conversationId
  const mapping = () => db.prepare("SELECT runtime_session_id,native_session_path,context_generation FROM runtime_sessions WHERE conversation_id=? AND runtime_type='pi'").get(chat)
  const turn = text => exchange(chat, text)
  await check('A15 长对话真实压缩后重启并回忆交接号', async () => {
    assert.equal(db.prepare('SELECT count(*) AS n FROM messages WHERE conversation_id=?').get(chat).n, 0)
    const challenge = 'CX-' + randomUUID().slice(0, 8)
    const seed = await turn('这份临时台账的验收交接号是 ' + challenge + '。请在本次对话中保留，不要写入笔记或调用工具。回复“收到”即可。')
    assert.equal(seed.tools.length, 0)
    const before = mapping()
    const runs = [seed.run]
    let summary
    for (let i = 0; i < 12; i++) {
      const rows = Array.from({ length: 145 }, (_, j) => '样本' + (i * 145 + j + 1) + '：批次审核完成，无异常。')
      const sent = await turn('这是本批台账。明细可以略去，交接号仍用于最后验收。只回复“确认”，不要重复编号或调用工具。\n' + rows.join('\n'))
      assert.equal(sent.tools.length, 0)
      runs.push(sent.run)
      const manager = SessionManager.open(before.native_session_path)
      const messages = manager.buildSessionContext().messages
      summary = messages.find(message => message.role === 'compactionSummary')
      // A retained user message or assistant reply could answer the question without the summary.
      if (summary?.summary.includes(challenge) && messages.every(message =>
        message === summary || !JSON.stringify(message).includes(challenge))) break
      summary = null
    }
    assert.ok(summary, '未获得只在活动摘要中保留交接号的原生上下文')
    const native = await readFile(before.native_session_path)
    await restartDesktop()
    assert.deepEqual(mapping(), before)
    assert.deepEqual(await readFile(before.native_session_path), native)
    const reply = await turn('我们这份台账的验收交接号是什么？请只回复完整编号，不用工具。')
    assert.equal(reply.tools.length, 0, JSON.stringify(reply))
    assert.equal(reply.reply.trim(), challenge, JSON.stringify(reply))
    assert.deepEqual(mapping(), before)
    return { contextWindow: 24576, runs, summaryChars: summary.summary.length,
      session: before.runtime_session_id, recallRun: reply.run, reply: reply.reply }
  })
}

async function checkReadyFaults({invoke, check, db, restartDesktop, directory, poll}, chat, mapping, history) {
  const request = {request: {conversationId: chat}}
  const root = (await realpath(join(directory, 'data/runtime-sessions'))).toLowerCase()
  const cases = [
    ['missing', path => unlink(path)],
    ['corrupt', path => writeFile(path, '{damaged jsonl\n')],
    ['identity_mismatch', async path => {
      const lines = (await readFile(path, 'utf8')).trimEnd().split('\n')
      const header = JSON.parse(lines[0])
      lines[0] = JSON.stringify({...header, id:randomUUID()})
      await writeFile(path, lines.join('\n') + '\n')
    }],
  ]
  for (const [kind, corrupt] of cases) {
    await check('A17 ready 会话故障 ' + kind, async () => {
      const previous = mapping()
      await restartDesktop(async () => {
        const path = await realpath(previous.native_session_path)
        assert.ok(path.toLowerCase().startsWith(root + '\\'))
        await corrupt(path)
      })
      const send = () => invoke('run_start', {request:{conversationId:chat,text:'请回复确认。'}})
      const broken = await send().then(async ({run}) => {
        const record = await poll('恢复失败的 Run', () => {
          const row = db.prepare('SELECT status,error_code,error_message FROM runs WHERE id=?').get(run.id)
          return row?.status === 'failed' && row
        })
        return JSON.stringify(record)
      }, error => error.message)
      assert.ok(broken.includes('session.native_' + kind), broken)
      assert.equal((await invoke('conversation_load', request)).contextRepair.available, true)
      const messages = history()
      await assert.rejects(send, /session\.native_unavailable/)
      assert.deepEqual(history(), messages)
      assert.deepEqual(mapping(), previous)
      await invoke('companion_context_repair', request)
      assert.equal(mapping().context_generation, previous.context_generation + 1)
      const {run} = await send()
      const row = await poll('修复后的 Run', () => {
        const r = db.prepare('SELECT status,error_code FROM runs WHERE id=?').get(run.id)
        return ['failed','completed'].includes(r?.status) && r
      })
      assert.equal(row.status, 'completed', JSON.stringify(row))
      return { kind, before: previous.context_generation, after: mapping().context_generation }
    })
  }
}
