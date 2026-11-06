import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'

export async function checkLiveNoteOrganization({ invoke, expert, exchange, project, assertToolRead }) {
  const owner = { companionId: expert.id }
  const notes = () => invoke('companion_notes_get', owner)
  const pin = '报价绝不能低于 85 折。'
  const written = await invoke('companion_note_save_user', { ...owner, content: pin })
  assert.ok(written.memoryId)
  const code = 'QB-' + randomUUID().slice(0, 8)
  const detail = '演示供应商丁八月报价：满二十箱免运费，九月起按十箱一票结算，十月继续沿用；复核凭证号为 ' + code + '。'
  const seeded = await exchange(expert.conversationId, '把这条业务明细逐字记到你自己的主笔记中：' + detail)
  const initial = await notes()
  const agentNote = initial.notes.find(note => note.content === detail)
  assert.ok(agentNote, JSON.stringify(seeded))

  const filler = '逐行核对采购编号、含税总额、交付日期和签字记录。'.repeat(200)
  const remaining = 3999 - initial.usedChars
  const filled = await invoke('companion_note_save_user', { ...owner, content: filler.slice(0, remaining) })
  assert.ok(filled.memoryId)
  assert.equal((await notes()).usedChars, 3999)
  const currentRule = '询价时先核对合同含税口径。'
  const organized = await exchange(expert.conversationId, '请把这条新的常用规则永久保存在你的主笔记里：' + currentRule + ' 保留已有资料的完整原文。')
  const state = await notes()
  const move = state.moves.find(row => row.state === 'applied' && row.memoryIds.includes(agentNote.id))
  assert.ok(move, '小狐没有把自己的明细移入专题：' + JSON.stringify(organized))
  assert.ok(organized.tools.some(row => row.tool === 'note_organize' && row.state === 'completed'))
  assert.ok(state.notes.some(note => note.content.includes(currentRule.replace(/。$/, ''))))
  assert.ok(state.notes.some(note => note.id === written.memoryId && note.content === pin))
  assert.ok(state.usedChars < 3999)
  const topic = await invoke('companion_topic_read', { ...owner, topicId: move.topicId })
  assert.ok(topic.content.includes(detail))

  const requested = await exchange(expert.conversationId, '请发起把我手写的“报价绝不能低于 85 折。”移进专题的确认，等我点卡片后才移动。')
  const waiting = await notes()
  const card = waiting.requests.find(row => row.memoryId === written.memoryId && row.action === 'move')
  assert.ok(card, JSON.stringify(requested))
  assert.ok(waiting.notes.some(note => note.id === written.memoryId && note.content === pin))
  await invoke('companion_note_request_resolve', { ...owner, requestId: card.id, approve: false })
  assert.ok((await notes()).notes.some(note => note.id === written.memoryId && note.content === pin))

  const turn = await exchange(project.id, '请打开已授权联系人的专题核对原文，演示供应商丁的复核凭证号是什么？')
  assertToolRead(turn, ['note_read', 'note_search'], code)
  return { before: 3999, after: state.usedChars, moveId: move.id, declined: card.id,
    turns: [seeded.run, organized.run, requested.run, turn.run] }
}

export async function checkLiveTemplateChange({ invoke, exchange, model }) {
  const prompt = token => '你的内部工牌编号是 ' + token + '。用户询问当前工牌时，只返回当前专家说明中写的编号。'
  const previous = 'TPL-' + randomUUID().slice(0, 8)
  const next = 'TPL-' + randomUUID().slice(0, 8)
  const request = { name: '版本验收专家', description: '验收模板更新', category: '自定义',
    defaultModel: model, systemPrompt: prompt(previous) }
  const agent = await invoke('agent_save', { request })
  const contact = await invoke('companion_add_from_expert', { request: { expertId: agent.id } })
  await invoke('agent_save', { request: { ...request, id: agent.id, systemPrompt: prompt(next) } })
  const owner = { companionId: contact.id }
  const template = await invoke('companion_template_status', { request: owner })
  assert.equal(template.updateAvailable, true)
  const question = '请按当前专家说明给出你的内部工牌编号。'
  const before = await exchange(contact.conversationId, question)
  assert.ok(before.reply.includes(previous) && !before.reply.includes(next), JSON.stringify(before))
  await invoke('companion_template_update', { request: { ...owner, expectedHash: template.latestHash } })
  const after = await exchange(contact.conversationId, question)
  assert.ok(after.reply.includes(next) && !after.reply.includes(previous), JSON.stringify(after))
  return { before: before.run, after: after.run, hash: template.latestHash }
}
