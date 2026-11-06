import assert from 'node:assert/strict'
import { DatabaseSync } from 'node:sqlite'
import { randomUUID } from 'node:crypto'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

export async function checkLiveModel({ source, invoke, db, check, builtin, expert, directory, poll }) {
  const installed = new DatabaseSync(source, { readOnly: true })
  let service
  try {
    service = installed.prepare(`SELECT name,model_id AS modelId,base_url AS baseUrl,api_type AS apiType,
      context_window AS contextWindow,max_output_tokens AS maxOutputTokens FROM model_service WHERE enabled=1`).get()
  } finally { installed.close() }
  assert.ok(service, '安装版 Fox 没有启用的全局模型')
  await invoke('model_service_save', { request: service })
  const configured = await invoke('model_service_get')
  assert.equal(configured.modelId, service.modelId)

  await check('已安装模型的凭据和连接验证', async () => {
    assert.ok(configured.credentialConfigured, '测试进程无法使用安装版留在 Windows 的模型凭据')
    const result = await invoke('model_service_test', { request: {} })
    assert.equal(result.ok, true, result.status)
    return { model: service.modelId, host: new URL(service.baseUrl).hostname }
  })

  const attempt = async (conversationId, text) => {
    const { run } = await invoke('run_start', { request: { conversationId, text } })
    const end = await poll('模型回答 ' + run.id, () => {
      const row = db.prepare('SELECT status,model,error_code FROM runs WHERE id=?').get(run.id)
      return ['completed','failed','interrupted','cancelled'].includes(row?.status) && row
    })
    const tools = db.prepare('SELECT tool,state FROM kernel_tool_calls WHERE run_id=? ORDER BY created_at,source_order').all(run.id)
    if (end.status !== 'completed') return { run: run.id, ...end, tools }
    const reply = db.prepare("SELECT content FROM messages WHERE run_id=? AND role='assistant' ORDER BY ordinal DESC LIMIT 1").get(run.id)
    assert.ok(reply?.content)
    return { run: run.id, ...end, reply: reply.content, tools }
  }
  const exchange = async (conversationId, text) => {
    const first = await attempt(conversationId, text)
    if (first.status === 'completed') return first
    assert.equal(first.error_code, 'runtime.model_first_response_timeout', JSON.stringify(first))
    assert.equal(first.tools.length, 0, 'A turn that ran a tool cannot be retried automatically')
    const retry = await attempt(conversationId, text)
    assert.equal(retry.status, 'completed', JSON.stringify({ first, retry }))
    return { ...retry, firstAttempt: first }
  }
  const shared = () => db.prepare("SELECT content FROM memory_entities WHERE scope='global' AND state='confirmed' AND enabled=1 AND deleted_at IS NULL").all().map(row => row.content)
  const quote = 'XQ-' + randomUUID().slice(0, 8)
  // Run all independent scenarios even if the model misses one of them.
  const failures = []
  const scenario = async (label, action) => {
    try { await check(label, action) } catch (error) { failures.push(label + ' ' + error.message) }
  }
  await scenario('A06 明确自述进入共享偏好', async () => {
    const turn = await exchange(builtin.conversationId, '我喜欢先看结论。请把这个长期偏好记在我们共用的用户偏好里。')
    assert.ok(shared().some(text => text.includes('先看结论')), JSON.stringify(turn))
    return turn
  })
  await scenario('A06 第三方偏好留在本次对话', async () => {
    const before = shared()
    const turn = await exchange(builtin.conversationId, '客户喜欢长报告，我不喜欢。这次给客户的报告写长一些。')
    assert.deepEqual(shared(), before, JSON.stringify(turn))
    return turn
  })
  await invoke('companion_topic_save', { companionId: expert.id, title: '虚构供应商报价', content: '演示供应商甲的内部报价编号为 ' + quote })
  await scenario('A21 私聊可读自己的专题记录', async () => {
    const turn = await exchange(expert.conversationId, '请查你的专题，告诉我演示供应商甲的内部报价编号。')
    assert.ok(turn.reply.includes(quote), JSON.stringify(turn))
    return turn
  })
  await scenario('A08 其他联系人读不到该报价', async () => {
    const turn = await exchange(builtin.conversationId, '演示供应商甲的内部报价编号是什么？请在自己的笔记中查证，查不到就说明。')
    assert.ok(!turn.reply.includes(quote), JSON.stringify(turn))
    return turn
  })
  const projectRoot = join(directory, 'live-project')
  await mkdir(projectRoot)
  const project = await invoke('conversation_create', { request: { agentId: 'fox-general', expertId: expert.templateAgentId, projectRoot } })
  await invoke('companion_grant_project', { companionId: expert.id, projectId: project.projectId })
  await scenario('A08 项目授权并引入专家后可以读报价', async () => {
    const turn = await exchange(project.id, '请查已授权专家的笔记，告诉我演示供应商甲的内部报价编号。')
    assert.ok(turn.reply.includes(quote), JSON.stringify(turn))
    return turn
  })
  const proof = 'FX-' + randomUUID().slice(0, 8)
  const input = join(directory, '验收材料.txt')
  await writeFile(input, '验收编号为 ' + proof)
  await invoke('companion_import_files', { companionId: builtin.id, paths: [input] })
  await scenario('A13 读取用户导入的文件', async () => {
    const turn = await exchange(builtin.conversationId, '请读取“收到的文件/验收材料.txt”，回复文件中的验收编号。')
    assert.ok(turn.reply.includes(proof), JSON.stringify(turn))
    return turn
  })
  await scenario('A26 说明定时任务限制', async () => {
    const turn = await exchange(builtin.conversationId, '以后每周一上午九点定时给我发送供应商报价。你现在能帮我安排好吗？')
    assert.match(turn.reply, /不能|无法|不支持|不具备|没有.{0,12}(定时|能力|功能)/, JSON.stringify(turn))
    return turn
  })
  if (failures.length) throw new Error(failures.join('\n'))
  return { model: service.modelId }
}
