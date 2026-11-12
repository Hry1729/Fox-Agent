import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'

export async function checkLiveNoteOrganization({ invoke, expert, exchange, project, assertToolRead, endpoint, service }) {
  const owner = { companionId: expert.id }
  const notes = () => invoke('companion_notes_get', owner)
  const pin = '报价绝不能低于 85 折。'
  const written = await invoke('companion_note_save_user', { ...owner, content: pin })
  assert.ok(written.memoryId)
  const code = 'QB-' + randomUUID().slice(0, 8)
  const detail = '演示供应商丁八月报价：满二十箱免运费，九月起按十箱一票结算，十月继续沿用；复核凭证号为 ' + code + '。'
  const local = { name: '联系人验收历史夹具', baseUrl: endpoint + '/' + randomUUID() + '/v1',
    modelId: 'fox-kernel-acceptance', apiType: 'openai-completions', contextWindow: 32000, maxOutputTokens: 1024 }
  let seeded
  try {
    const configured = await invoke('model_service_save', { request: local })
    assert.equal(configured.credentialConfigured, false)
    seeded = await exchange(expert.conversationId, '[CONTACTS_M1_SEED_NOTE]\n' + detail)
    assert.equal(seeded.model, local.modelId)
    assert.ok(seeded.tools.some(row => row.tool === 'memory_write' && row.state === 'completed'), JSON.stringify(seeded))
  } finally {
    const restored = await invoke('model_service_save', { request: service })
    assert.equal(restored.modelId, service.modelId)
  }
  const initial = await notes()
  const agentNote = initial.notes.find(note => note.content === detail)
  assert.equal(agentNote?.sourceRunId, seeded.run, JSON.stringify(seeded))
  assert.equal(agentNote.createdBy, 'agent')

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
  assert.ok([organized, requested, turn].every(result => result.model === service.modelId))
  return { historyFixture: { run: seeded.run, model: seeded.model }, before: 3999, after: state.usedChars, moveId: move.id, declined: card.id,
    liveTurns: [organized.run, requested.run, turn.run] }
}

export async function checkLiveFrozenTemplate({ invoke, exchange, model }) {
  const prompt = token => '你的内部工牌编号是 ' + token + '。用户询问当前工牌时，只返回当前专家说明中写的编号。'
  const previous = 'TPL-' + randomUUID().slice(0, 8)
  const next = 'TPL-' + randomUUID().slice(0, 8)
  const request = { name: '版本验收专家', description: '验收冻结模板', category: '自定义',
    defaultModel: model, systemPrompt: prompt(previous) }
  const agent = await invoke('agent_save', { request })
  const contact = await invoke('companion_add_from_expert', { request: { expertId: agent.id } })
  await invoke('agent_save', { request: { ...request, id: agent.id, systemPrompt: prompt(next) } })
  const question = '只回复当前专家说明中生效的内部工牌编号。'
  const before = await exchange(contact.conversationId, question)
  assert.equal(answeredBadge(before.reply), previous, JSON.stringify(before))
  const after = await exchange(contact.conversationId, question)
  assert.equal(answeredBadge(after.reply), previous, JSON.stringify(after))
  return { before: { run: before.run, badge: previous }, after: { run: after.run, badge: previous } }
}

function answeredBadge(reply) {
  const firstLine = reply.trimStart().split(/\r?\n/, 1)[0]
  return firstLine.match(/\bTPL-[0-9a-f]{8}\b/)?.[0]
}
