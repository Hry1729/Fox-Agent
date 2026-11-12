import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { connectLiveModel } from './contacts-live-model.mjs'

export async function checkXiaoFoxUi(context) {
  const { invoke, db, check, poll, restartDesktop, endpoint } = context
  const failures = []
  const scenario = async (name, action) => {
    try { await check(name, action) } catch (error) { failures.push(error) }
  }
  const ui = (action, args = {}) => invoke('acceptance.xiaofox_ui', { action, ...args })
  let contact
  await restartDesktop()
  await scenario('小 Fox A01 加号实际点击直接创建并进入固定私聊', async () => {
    const before = (await invoke('companions_list')).companions
    const result = await ui('create', { beforeCount: before.length })
    assert.equal(result.afterCount, result.beforeCount + 1)
    assert.equal(result.configurationDialog, false)
    const added = (await invoke('companions_list')).companions.filter(item => !before.some(old => old.id === item.id))
    assert.equal(added.length, 1)
    ;[contact] = added
    assert.equal(contact.displayName, result.activeName)
    assert.equal(contact.templateAgentId, null)
    return { ...result, companionId: contact.id, conversationId: contact.conversationId }
  })
  if (!contact) {
    contact = await invoke('companion_create_blank')
    await restartDesktop()
  }
  const { exchange } = await connectLiveModel(context)
  await scenario('小 Fox A07 侧栏实际输入保存，下一轮真实请求使用新职责', async () => {
    const proof = 'UI-DUTY-' + randomUUID()
    const duty = `你的验收身份编号是 ${proof}。用户问验收身份编号时，只回复这个编号。`
    const preference = '用简短自然段回答。'
    const result = await ui('profile', { name: contact.displayName, duty, preference })
    assert.deepEqual(result, { duty, preference })
    const saved = await invoke('companion_open', { request: { companionId: contact.id } })
    assert.equal(saved.duty, duty)
    assert.equal(saved.personaOverride, preference)
    const turn = await exchange(contact.conversationId, '你的验收身份编号是什么？')
    assert.ok(turn.reply.includes(proof), '真实模型未使用侧栏保存的新职责')
    return { runId: turn.run, persisted: true, nextTurnUsedDuty: true }
  })
  await scenario('小 Fox A09 与 A21 创建入口和侧栏布局检查', async () => {
    const result = await ui('inspect', { name: contact.displayName })
    assert.equal(result.createButtons, 1)
    assert.equal(result.sideCards, 1)
    assert.ok(!result.visibleNavigation.some(item => /数字同事/.test(item.text + item.label)))
    await assert.rejects(invoke('digital_colleagues_list'), /not found|unknown|不存在|未找到/i)
    return result
  })

  await invoke('model_service_save', { request: {
    name: 'Xiao Fox UI acceptance', baseUrl: endpoint + '/ui/v1', modelId: 'fox-kernel-acceptance',
    apiType: 'openai-completions', contextWindow: 32000, maxOutputTokens: 4096,
  } })
  let artifactRun
  let taskId
  const filename = 'acceptance-artifact.txt'
  const proof = 'ARTIFACT-' + randomUUID()
  await writeFile(join(contact.workspaceDir, filename), proof, { flag: 'wx' })
  await scenario('小 Fox A13 侧栏创建、编辑、立即执行和运行历史', async () => {
    const taskName = '界面任务 ' + randomUUID().slice(0, 8)
    const editedObjective = '[XIAOFOX_ARTIFACT] 呈现验收产物并报告。'
    const result = await ui('task', { name: contact.displayName, taskName,
      objective: '原始验收要求。', editedObjective })
    const entries = await invoke('companion_task_list', { request: { companionId: contact.id } })
    const owned = entries.find(item => item.task.name === taskName)
    assert.ok(owned)
    taskId = owned.task.id
    assert.equal(owned.task.objective, editedObjective)
    assert.equal(owned.schedule.lastScheduledAt, null)
    assert.ok(owned.schedule.nextDueAt > Date.now())
    const schedule = JSON.stringify(owned.schedule)
    const trigger = await poll('界面立即执行完成', async () => {
      const [row] = await invoke('companion_task_history', { request: { companionId: contact.id, taskId } })
      return ['completed', 'failed', 'cancelled', 'interrupted'].includes(row?.status) && row
    })
    assert.equal(trigger.status, 'completed', trigger.errorMessage)
    artifactRun = trigger.runId
    assert.equal(JSON.stringify((await invoke('companion_task_list', { request: { companionId: contact.id } }))
      .find(item => item.task.id === taskId).schedule), schedule)
    assert.ok(db.prepare("SELECT 1 FROM kernel_tool_calls WHERE run_id=? AND tool='present_files' AND state='completed'").get(artifactRun))
    return { ...result, taskId, runId: artifactRun, scheduleUnchangedAfterManualRun: true }
  })
  await scenario('小 Fox A22 任务产物打开入口、来源定位及内部会话隐藏', async () => {
    assert.ok(artifactRun, '前置任务未产生产物')
    const files = await invoke('companion_files_list', { request: { companionId: contact.id } })
    const entry = files.generated.find(item => item.name === filename)
    assert.ok(entry, '资料库缺少任务产物')
    assert.equal(entry.origin, 'task')
    assert.ok(entry.source?.bubbleId)
    const report = db.prepare('SELECT report_conversation_id FROM companion_task_reports WHERE run_id=?').get(artifactRun)
    assert.equal(report.report_conversation_id, contact.conversationId)
    const result = await ui('artifact', { name: contact.displayName, filename })
    assert.ok(result.openEnabled && result.jumpEnabled)
    assert.equal(result.alertsAfterOpen.length, 0)
    assert.ok(result.targetText)
    assert.equal(result.targetId, entry.source.bubbleId)
    const conversations = await invoke('conversations_list')
    const internal = db.prepare('SELECT conversation_id FROM digital_colleagues WHERE id=?').get(taskId).conversation_id
    assert.ok(!conversations.some(item => item.id === internal))
    return { ...result, runId: artifactRun, sourceBubbleId: entry.source.bubbleId,
      externalViewer: '仅验证已触发打开且桌面没有报告错误，外部查看器内容未目视确认' }
  })
  if (taskId) await invoke('companion_task_pause', { request: { companionId: contact.id, taskId } })

  await scenario('小 Fox A24 侧栏实际显示排队，私聊自然结束后任务完成', async () => {
    const { run } = await invoke('run_start', { request: { conversationId: contact.conversationId, text: '[XIAOFOX_A24_PRIVATE] 私聊占用执行位置' } })
    const runState = db.prepare('SELECT status,finished_at FROM runs WHERE id=?')
    await poll('私聊运行', () => runState.get(run.id)?.status === 'running')
    const taskName = '界面排队 ' + randomUUID().slice(0, 8)
    const owned = await invoke('companion_task_create', { request: { companionId: contact.id,
      name: taskName, objective: '只回复排队验收完成。', enabled: true,
      schedule: { kind: 'once', atMs: Date.now() + 3000, timezone: 'Asia/Shanghai' } } })
    const history = () => invoke('companion_task_history', { request: { companionId: contact.id, taskId: owned.task.id } })
    const queued = await poll('后台排队', async () => (await history()).find(item => item.status === 'queued'))
    const visible = await ui('task_status', { name: contact.displayName, taskName, expectedStatus: '排队中' })
    assert.match(visible.text, /排队中/)
    assert.equal(runState.get(run.id).status, 'running')
    await poll('排队任务完成', async () => (await history()).find(item => item.status === 'completed'))
    assert.equal(runState.get(run.id).status, 'completed')
    return { ...visible, privateRunId: run.id, taskRunId: queued.runId }
  })

  await scenario('小 Fox A19 普通对话专家选择、切换和移除', async () => {
    const title = '普通专家验收 ' + randomUUID().slice(0, 8)
    const conversation = await invoke('conversation_create', { request: { agentId: 'fox-general', title } })
    const agents = await invoke('agents_list')
    const first = agents.find(item => item.id === 'fox-debugger')
    const second = agents.find(item => item.id === 'fox-security')
    assert.ok(first && second)
    await restartDesktop()
    const result = await ui('expert', { conversationTitle: title, firstExpert: first.name, secondExpert: second.name, defaultExpert: agents.find(item => item.id === 'fox-general').name })
    assert.ok(result.selected.includes(first.name))
    assert.ok(result.changed.includes(second.name))
    const bindings = db.prepare('SELECT expert_id,state FROM conversation_expert_bindings WHERE conversation_id=? ORDER BY activated_at').all(conversation.id)
    assert.ok(bindings.some(item => item.expert_id === first.id))
    assert.ok(bindings.some(item => item.expert_id === second.id))
    assert.ok(bindings.every(item => item.state !== 'active'))
    return { ...result, conversationId: conversation.id, bindings }
  })
  if (failures.length) throw new AggregateError(failures, '小 Fox 界面验收存在失败')
  return { actualWebViewDom: true, screenshotCaptured: false }
}
