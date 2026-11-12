import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { connectLiveModel } from './contacts-live-model.mjs'

export async function checkXiaoFoxBehavior(context) {
  const { invoke, db, check, directory } = context
  const { model, exchange } = await connectLiveModel(context)
  const failures = []
  const scenario = async (name, action) => {
    try {
      await check(name, action)
    } catch (error) {
      failures.push(error)
    }
  }
  const profile = db.prepare('SELECT display_name, duty, persona_override, avatar_ref FROM companion_profiles WHERE id = ?')
  const memories = db.prepare('SELECT id, state, enabled, version FROM memory_entities ORDER BY id')
  const receipts = db.prepare('SELECT tool, state, result_json FROM kernel_tool_calls WHERE run_id = ? ORDER BY created_at, source_order')
  const successful = (row, tool) => row.tool === tool && row.state === 'completed'
    && row.result_json && JSON.parse(row.result_json).isError !== true

  await scenario('小 Fox A06 临时格式、单独沟通偏好和待确认共享记忆', async () => {
    const contact = await invoke('companion_create_blank')
    const other = await invoke('companion_create_blank')
    const initial = profile.get(contact.id)
    const untouched = profile.get(other.id)
    const before = memories.all()
    const temporary = await exchange(contact.conversationId,
      '这次用表格列出虚构的台账条目“验收纸张”和“验收文具”，数量各 1。此要求只用于本轮。')
    assert.match(temporary.reply, /\|[^\r\n]+\|/)
    assert.ok(!temporary.tools.some(call => ['companion_profile_update', 'memory_write', 'note_organize'].includes(call.tool)),
      '本轮格式要求不应调用写入工具')
    assert.deepEqual(profile.get(contact.id), initial)
    assert.deepEqual(profile.get(other.id), untouched)
    assert.deepEqual(memories.all(), before)

    const preference = await exchange(contact.conversationId,
      '以后你回答我时先给结论。请把“回答先给结论”作为你自己的长期沟通偏好保存，只对当前小 Fox 生效。')
    assert.ok(receipts.all(preference.run).some(row => successful(row, 'companion_profile_update')),
      '没有成功的沟通偏好更新回执')
    const afterPreference = profile.get(contact.id)
    assert.equal(afterPreference.persona_override, '回答先给结论')
    assert.deepEqual(afterPreference, { ...initial, persona_override: '回答先给结论' })
    assert.deepEqual(profile.get(other.id), untouched)
    assert.deepEqual(memories.all(), before)

    const canonicalKey = 'units-acceptance-' + randomUUID()
    const old = await invoke('memory_create', { request: {
      scope: 'global', kind: 'preference', canonicalKey, content: '我习惯用英制单位',
    } })
    assert.equal(old.state, 'confirmed')
    const statement = '我习惯用公制单位'
    const shared = await exchange(contact.conversationId, [
      statement + '。',
      '请更新所有小 Fox 共用的用户计量单位偏好，替换旧的英制习惯。',
      `共享条目的键是 ${canonicalKey}。请按系统确认流程处理。`,
    ].join('\n'))
    const requests = db.prepare(`
      SELECT r.id, r.status, m.id AS memory_id, m.state, m.scope, m.content, m.evidence_excerpt
      FROM companion_shared_requests r JOIN memory_entities m ON m.id = r.candidate_memory_id
      WHERE r.companion_id = ? AND m.source_run_id = ?
    `).all(contact.id, shared.run)
    assert.equal(requests.length, 1)
    const [pending] = requests
    assert.equal(pending.status, 'open')
    assert.equal(pending.state, 'candidate')
    assert.equal(pending.scope, 'global')
    assert.equal(pending.content.replace(/[。！!]$/, ''), statement)
    assert.equal(pending.evidence_excerpt.replace(/[。！!]$/, ''), statement)
    assert.ok(receipts.all(shared.run).some(row => successful(row, 'memory_write')
      && row.result_json.includes('pending') && row.result_json.includes(pending.id)), '没有对应的待确认工具回执')
    assert.match(shared.reply, /确认|同意|批准/)
    assert.doesNotMatch(shared.reply, /已(?:经)?(?:记住|写入|保存|更新).{0,16}(?:公制|共享|偏好)/)
    assert.equal(db.prepare('SELECT enabled FROM memory_entities WHERE id = ?').get(old.id).enabled, 1)
    assert.deepEqual(profile.get(contact.id), afterPreference)
    assert.deepEqual(profile.get(other.id), untouched)

    const confirmed = await invoke('companion_shared_request_resolve', {
      companionId: contact.id, requestId: pending.id, decision: 'replace', replaceMemoryId: old.id,
    })
    assert.equal(confirmed.status, 'updated')
    assert.equal(confirmed.memoryId, pending.memory_id)
    const enabled = db.prepare('SELECT state, enabled FROM memory_entities WHERE id = ?')
    assert.deepEqual(enabled.get(pending.memory_id), { state: 'confirmed', enabled: 1 })
    assert.equal(enabled.get(old.id).enabled, 0)
    assert.equal(db.prepare('SELECT status FROM companion_shared_requests WHERE id = ?').get(pending.id).status, 'replaced')
    return { temporaryRunId: temporary.run, preferenceRunId: preference.run, sharedRunId: shared.run, confirmedAfterUserAction: true }
  })

  await scenario('小 Fox A08 重名真实工具失败，两个对象均无改动', async () => {
    const contact = await invoke('companion_create_blank')
    const other = await invoke('companion_create_blank')
    const before = profile.get(contact.id)
    const untouched = profile.get(other.id)
    assert.notEqual(before.display_name, untouched.display_name)
    const turn = await exchange(contact.conversationId, [
      `请用 companion_profile_update 把你自己的名字改成“${untouched.display_name}”，同时把职责改成“重名验收职责”。`,
      '只提交这个准确的名字，不自动生成替代名。失败时报告实际原因。',
    ].join('\n'))
    const updates = receipts.all(turn.run).filter(row => row.tool === 'companion_profile_update')
    assert.ok(updates.some(row => row.result_json
      && /companion\.name_conflict|已有联系人使用这个名字/.test(row.result_json)
      && (row.state === 'failed' || JSON.parse(row.result_json).isError === true)), '没有真实的重名失败工具回执')
    assert.ok(!updates.some(row => successful(row, 'companion_profile_update')), '重名后不得改写其他字段')
    assert.match(turn.reply, /重名|同名|已有|占用|重复/)
    assert.doesNotMatch(turn.reply, /已(?:经)?(?:成功|为你)?(?:保存|修改|更新|改名|改成|设置)/)
    assert.deepEqual(profile.get(contact.id), before)
    assert.deepEqual(profile.get(other.id), untouched)
    return { runId: turn.run, duplicateRejected: true, unchangedProfiles: 2 }
  })

  await scenario('小 Fox A10 同专家的两个小 Fox 按当前项目的显式引用读取', async () => {
    const expertId = 'fox-debugger'
    const contacts = await Promise.all([1, 2].map(() => invoke('companion_add_from_expert', { request: { expertId } })))
    assert.notEqual(contacts[0].id, contacts[1].id)
    for (const contact of contacts) assert.equal(contact.templateAgentId, expertId)
    const title = '隔离验收台账'
    const markers = contacts.map(() => 'PROJECT-' + randomUUID())
    for (const [index, contact] of contacts.entries()) {
      const result = await invoke('companion_topic_save', {
        companionId: contact.id, title, content: '虚构台账编号：' + markers[index],
      })
      assert.ok(result.topicId)
    }
    const projectRoot = join(directory, 'xiaofox-reference-project')
    await mkdir(projectRoot)
    const createConversation = () => invoke('conversation_create', { request: { agentId: 'fox-general', expertId, projectRoot } })
    const conversations = [await createConversation(), await createConversation()]
    const projectId = conversations[0].projectId
    assert.ok(projectId)
    assert.equal(conversations[1].projectId, projectId)
    for (const contact of contacts) await invoke('companion_grant_project', { companionId: contact.id, projectId })
    const ask = `请使用 note_read 打开当前项目显式引用的小 Fox 笔记《${title}》，并只回答原文中的虚构台账编号。查不到就说明。`
    const runIds = []
    for (const [index, conversation] of conversations.entries()) {
      const options = await invoke('companion_project_note_options', { conversationId: conversation.id })
      assert.equal(options.selectedCompanionId, null)
      assert.deepEqual(options.options.map(item => item.id).sort(), contacts.map(item => item.id).sort())
      await invoke('companion_project_note_reference_set', { conversationId: conversation.id, companionId: contacts[index].id })
      assert.equal((await invoke('companion_project_note_options', { conversationId: conversation.id })).selectedCompanionId, contacts[index].id)
      const turn = await exchange(conversation.id, ask)
      const results = receipts.all(turn.run)
      assert.ok(results.some(row => successful(row, 'note_read') && row.result_json.includes(markers[index])),
        '缺少当前显式引用对应的随机编号回执')
      assert.ok(turn.reply.includes(markers[index]))
      assert.ok(!turn.reply.includes(markers[1 - index]))
      assert.ok(results.every(row => !row.result_json?.includes(markers[1 - index])))
      runIds.push(turn.run)
    }
    await invoke('companion_revoke_project', { companionId: contacts[1].id, projectId })
    const revoked = await invoke('companion_project_note_options', { conversationId: conversations[1].id })
    assert.equal(revoked.selectedCompanionId, null)
    assert.deepEqual(revoked.options.map(item => item.id), [contacts[0].id])
    const withoutReference = await createConversation()
    await assert.rejects(invoke('companion_project_note_reference_set', {
      conversationId: withoutReference.id, companionId: contacts[1].id,
    }), /companion\.project_note_reference_invalid_companion/)
    const turn = await exchange(withoutReference.id, ask)
    assert.match(turn.reply, /查不到|无法|不能|没有|未配置|未引用|未授权|不具备/)
    for (const marker of markers) {
      assert.ok(!turn.reply.includes(marker))
      assert.ok(receipts.all(turn.run).every(row => !row.result_json?.includes(marker)))
    }
    return { selectedRunIds: runIds, unselectedRunId: turn.run, revokedReferenceRejected: true }
  })

  if (failures.length) throw new AggregateError(failures, '小 Fox 真实行为验收存在失败')
  return { model }
}
