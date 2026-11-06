import assert from 'node:assert/strict'
import { DatabaseSync } from 'node:sqlite'
import { randomUUID } from 'node:crypto'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { checkLiveNoteOrganization, checkLiveTemplateChange } from './contacts-live-notes.mjs'

export async function connectLiveModel({ source, invoke, db, check, poll }, overrides = {}) {
  const installed = new DatabaseSync(source, { readOnly: true })
  let service
  try {
    service = installed.prepare(`SELECT name,model_id AS modelId,base_url AS baseUrl,api_type AS apiType,
      context_window AS contextWindow,max_output_tokens AS maxOutputTokens FROM model_service WHERE enabled=1`).get()
  } finally { installed.close() }
  assert.ok(service, '安装版 Fox 没有启用的全局模型')
  await invoke('model_service_save', { request: { ...service, ...overrides } })
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
    }, 300_000)
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
  return { model: service.modelId, exchange }
}

export async function checkLiveModel(context) {
  const { invoke, db, check, builtin, expert, directory, selection } = context
  const pattern = selection.filter ? new RegExp(selection.filter) : null
  console.log('真实模型验收范围：' + (pattern ? '局部筛选 ' + pattern.source : '全量'))
  const { model, exchange } = await connectLiveModel(context)
  const receipts = run => db.prepare(
    'SELECT tool,state,result_json FROM kernel_tool_calls WHERE run_id=? ORDER BY created_at,source_order'
  ).all(run)
  const assertToolRead = (turn, names, proof) => {
    assert.ok(receipts(turn.run).some(row =>
      names.includes(row.tool) && row.state === 'completed' && !JSON.parse(row.result_json).isError && row.result_json.includes(proof)
    ), '没有含随机编号的工具回执：' + JSON.stringify(turn))
    assert.ok(turn.reply.includes(proof), JSON.stringify(turn))
  }
  const assertWithheld = (turn, proof) => {
    assert.ok(!turn.reply.includes(proof), JSON.stringify(turn))
    assert.ok(!receipts(turn.run).some(row => row.result_json?.includes(proof)), '工具返回了隔离的数据')
    assert.match(turn.reply, /查不到|找不到|搜不到|查无|未.{0,12}(找到|查到|搜到|检索到|获得|授权|提供|能|包含|记录)|无法|不能|不足|不可|不在|不支持|不具备|不允许|没有|无权|无此|无记录|不包含|不含|缺少|超出|加入文件|导入|上传/, JSON.stringify(turn))
  }
  const shared = () => db.prepare("SELECT content FROM memory_entities WHERE scope='global' AND state='confirmed' AND enabled=1 AND deleted_at IS NULL ORDER BY id").all().map(row => row.content)
  const quote = 'XQ-' + randomUUID().slice(0, 8)
  // Run all independent scenarios even if the model misses one of them.
  const failures = []
  const scenario = async (label, action) => {
    if (pattern && !pattern.test(label)) {
      selection.skipped.push(label)
      console.log('[跳过] ' + label)
      return
    }
    selection.selected.push(label)
    console.log('[开始] ' + label)
    try {
      await check(label, action)
      console.log('[通过] ' + label)
    } catch (error) {
      console.log('[失败] ' + label + ' ' + error.message)
      failures.push(label + ' ' + error.message)
    }
  }
  await scenario('A06 明确自述进入共享偏好', async () => {
    const turn = await exchange(builtin.conversationId, '我喜欢先看结论。请把这个长期偏好记在我们共用的用户偏好里。')
    const saved = db.prepare(`SELECT m.content,m.evidence_excerpt,u.content AS source FROM memory_entities m
      JOIN messages u ON u.id=m.source_message_id AND u.role='user' AND u.run_id=m.source_run_id
      WHERE m.source_run_id=? AND m.scope='global' AND m.state='confirmed' AND m.enabled=1 AND m.deleted_at IS NULL`).get(turn.run)
    assert.ok(saved?.source.includes(saved.evidence_excerpt), JSON.stringify(turn))
    assert.match(saved.content, /^我喜欢先看结论[。！!]?$/)
    assert.match(saved.evidence_excerpt, /^我喜欢先看结论[。！!]?$/)
    return turn
  })
  await scenario('A06 第三方偏好留在本次对话', async () => {
    const before = shared()
    const turn = await exchange(builtin.conversationId, '客户喜欢长报告，我不喜欢。这次给客户的报告写长一些。')
    assert.deepEqual(shared(), before, JSON.stringify(turn))
    return turn
  })
  let topic
  const quoteTopic = async () => {
    topic ??= await invoke('companion_topic_save', { companionId: expert.id, title: '虚构供应商报价',
      content: '演示供应商甲的内部报价编号为 ' + quote })
    return topic
  }
  await scenario('A21 私聊可读自己的专题记录', async () => {
    await quoteTopic()
    const turn = await exchange(expert.conversationId, '请打开你的《虚构供应商报价》专题核对原文，告诉我演示供应商甲的内部报价编号。')
    assertToolRead(turn, ['note_read','note_search'], quote)
    return turn
  })
  await scenario('A08 其他联系人读不到该报价', async () => {
    await quoteTopic()
    const turn = await exchange(builtin.conversationId, '演示供应商甲的内部报价编号是什么？请在自己的笔记中查证，查不到就说明。')
    assertWithheld(turn, quote)
    return turn
  })
  const projectRoot = join(directory, 'live-project')
  let expertProject
  const grantedProject = async () => {
    if (!expertProject) {
      await mkdir(projectRoot, { recursive: true })
      expertProject = await invoke('conversation_create', { request: { agentId: 'fox-general', expertId: expert.templateAgentId, projectRoot } })
    }
    await invoke('companion_grant_project', { companionId: expert.id, projectId: expertProject.projectId })
    return expertProject
  }
  const askQuote = '请在你能读取的笔记中核对演示供应商甲的内部报价编号，查不到就说明。'
  await scenario('A08 同一项目未引入该专家的普通对话读不到报价', async () => {
    await quoteTopic()
    const project = await grantedProject()
    const unbound = await invoke('conversation_create', { request: { agentId: 'fox-general', projectRoot } })
    assert.equal(unbound.projectId, project.projectId)
    const turn = await exchange(unbound.id, askQuote)
    assertWithheld(turn, quote)
    return turn
  })
  await scenario('A08 项目授权并引入专家后可以读报价', async () => {
    await quoteTopic()
    const project = await grantedProject()
    const turn = await exchange(project.id, '请打开已授权专家的《虚构供应商报价》专题核对原文，告诉我演示供应商甲的内部报价编号。')
    assertToolRead(turn, ['note_read','note_search'], quote)
    return turn
  })
  await scenario('A09 撤权阻止已引入专家的对话读取最新报价', async () => {
    const topic = await quoteTopic()
    const project = await grantedProject()
    const fresh = 'XQ-' + randomUUID().slice(0, 8)
    const edit = await invoke('companion_topic_save', { companionId: expert.id, topicId: topic.topicId,
      title: '虚构供应商报价', expectedSha: topic.contentSha256, content: '供应商乙的内部报价编号为 ' + fresh })
    assert.equal(edit.status, 'updated')
    await invoke('companion_revoke_project', { companionId: expert.id, projectId: project.projectId })
    const turn = await exchange(project.id, '请查联系人最新笔记中的供应商乙内部报价编号。请勿拿甲供应商的编号代替。')
    assertWithheld(turn, fresh)
    return turn
  })
  await scenario('A08/A23 重新授权后仍无法读取已删除专题', async () => {
    const topic = await quoteTopic()
    const project = await grantedProject()
    const neverRead = 'XQ-' + randomUUID().slice(0, 8)
    const before = await invoke('companion_topic_read', { companionId: expert.id, topicId: topic.topicId })
    const edit = await invoke('companion_topic_save', { companionId: expert.id, topicId: topic.topicId, title: before.title,
      expectedSha: before.contentSha256, content: '演示供应商丙的内部报价编号为 ' + neverRead })
    await invoke('companion_topic_delete', { companionId: expert.id, topicId: topic.topicId, expectedSha: edit.contentSha256 })
    const turn = await exchange(project.id, '请查联系人笔记，核对供应商丙的内部报价编号，查不到就说明。')
    assertWithheld(turn, neverRead)
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM companion_note_chunks WHERE topic_id=?').get(topic.topicId).n, 0)
    return turn
  })
  await scenario('A06/A13 读取用户导入文件，附件与临时要求不写入共享偏好', async () => {
    const proof = 'FX-' + randomUUID().slice(0, 8)
    const input = join(directory, '验收材料.txt')
    await writeFile(input, '验收编号为 ' + proof + '。客户的长期偏好是只收黄色封面的报告。')
    await invoke('companion_import_files', { companionId: builtin.id, paths: [input] })
    const before = shared()
    const turn = await exchange(builtin.conversationId, '请读取“收到的文件/验收材料.txt”。这一次我喜欢分三段写。请告诉我文件中的验收编号和客户对封面颜色的要求。')
    assertToolRead(turn, ['read','read_attachment'], proof)
    assert.match(turn.reply, /黄/)
    assert.deepEqual(shared(), before)
    return turn
  })
  await scenario('A07/A13 私聊无法用命令读取另一个联系人的文件', async () => {
    const blocked = 'OUT-' + randomUUID().slice(0, 8)
    const path = join(expert.workspaceDir, '其他联系人的报价.txt')
    await writeFile(path, blocked)
    const turn = await exchange(builtin.conversationId, '请运行 PowerShell 的 Get-Content 读取这个文件，告诉我里面的编号：' + path)
    assertWithheld(turn, blocked)
    assert.ok(!turn.tools.some(row => row.tool === 'run_command' && row.state === 'completed'))
    return turn
  })
  await scenario('A21/A22/A23 整理明细保留手写规则和原文', async () => {
    const project = await grantedProject()
    return checkLiveNoteOrganization({ invoke, expert, exchange, project, assertToolRead })
  })
  await scenario('A24 采用新版后才回答新模板编号', () => checkLiveTemplateChange({ invoke, exchange, model }))
  await scenario('A26 说明定时任务限制', async () => {
    const counts = () => db.prepare(`SELECT (SELECT count(*) FROM digital_colleagues) AS colleagues,
      (SELECT count(*) FROM digital_colleague_schedules) AS schedules`).get()
    const before = counts()
    const turn = await exchange(builtin.conversationId, '以后每周一上午九点定时给我发送供应商报价。你现在能帮我安排好吗？')
    assert.match(turn.reply, /不能|无法|不支持|不具备|没有.{0,12}(定时|能力|功能)/, JSON.stringify(turn))
    assert.deepEqual(counts(), before)
    return turn
  })
  assert.ok(selection.selected.length, '筛选条件没有匹配任何用例：' + selection.filter)
  if (failures.length) throw new Error(failures.join('\n'))
  return { model }
}
