import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { mkdir, stat } from 'node:fs/promises'
import { join } from 'node:path'
import { connectLiveModel } from './contacts-live-model.mjs'

const terminal = ['completed', 'failed', 'interrupted', 'cancelled']
const runTimeout = 300_000

export async function checkCompanionTeam(context) {
  const { invoke, db, check, poll, builtin, directory } = context
  const { model, service, exchange } = await connectLiveModel(context)
  const suffix = randomUUID().slice(0, 8)
  const failures = []
  const filter = process.env.FOX_CONTACTS_LIVE_FILTER?.trim()
  const selected = filter ? new RegExp(filter) : null
  const skipped = []
  const receipts = db.prepare('SELECT tool,state,result_json FROM kernel_tool_calls WHERE run_id=? ORDER BY created_at,source_order')
  const profile = db.prepare('SELECT display_name,duty,model_provider_id,model_ref,archived_at FROM companion_profiles WHERE id=?')
  const assistantReply = db.prepare("SELECT content FROM messages WHERE run_id=? AND role='assistant' ORDER BY ordinal DESC LIMIT 1")
  const assignmentFor = db.prepare('SELECT * FROM companion_assignments WHERE source_run_id=? ORDER BY created_at')
  const chief = builtin.conversationId
  const chiefTools = ['companion_create', 'companion_propose_profile', 'companion_propose_model', 'companion_propose_archive', 'companion_propose_deletion']

  async function scenario(name, action) {
    if (selected && !selected.test(name)) {
      skipped.push(name)
      console.log('[未执行] ' + name)
      return
    }
    try {
      await check(name, action)
    } catch (error) {
      failures.push(error)
    }
  }

  async function fixture(label) {
    const item = await invoke('companion_create_blank')
    const displayName = label + suffix
    await invoke('companion_update_profile', { request: { companionId: item.id, displayName, duty: '核对虚构验收资料' } })
    return { ...item, displayName }
  }

  function requireTool(turn, name) {
    const receipt = receipts.all(turn.run).find(row =>
      row.tool === name && row.state === 'completed' && row.result_json && JSON.parse(row.result_json).isError !== true)
    assert.ok(receipt, `Run ${turn.run} 缺少成功的 ${name} 回执`)
    return JSON.parse(receipt.result_json)
  }

  async function teamAction(turn, tool, kind, status, target) {
    requireTool(turn, tool)
    const actions = await invoke('companion_team_actions_list', { request: { companionId: builtin.id } })
    const matches = actions.filter(action => action.runId === turn.run && action.kind === kind)
    assert.equal(matches.length, 1, `Run ${turn.run} 的 ${kind} 卡片数量`)
    assert.equal(matches[0].status, status)
    if (target) assert.equal(matches[0].targetId, target)
    return matches[0]
  }

  async function decide(action, decision, status) {
    const result = await invoke('companion_team_action_resolve', { request: { actionId: action.id, decision } })
    assert.equal(result.status, status)
    return result
  }

  async function completedRun(runId) {
    const run = await poll('协作运行 ' + runId, () => {
      const found = db.prepare('SELECT status,error_code FROM runs WHERE id=?').get(runId)
      return terminal.includes(found?.status) && found
    }, runTimeout)
    assert.equal(run.status, 'completed', `Run ${runId} 失败：${run.error_code ?? run.status}`)
  }

  async function completedAssignment(turn, recipient, consult, proof) {
    requireTool(turn, 'companion_coordinate')
    const queued = assignmentFor.all(turn.run)
    assert.equal(queued.length, 1, `Run ${turn.run} 应恰好建立一条协作`)
    const [first] = queued
    assert.equal(first.source_run_id, turn.run)
    assert.equal(first.root_run_id, turn.run)
    assert.equal(first.recipient_companion_id, recipient.id)
    assert.equal(first.consult, Number(consult))
    const settled = await poll('协调结果自动回传 ' + first.id, () => {
      const row = assignmentFor.get(turn.run)
      return ['completed', 'failed', 'cancelled'].includes(row.status) && (row.status !== 'completed' || row.resume_run_id) && row
    }, runTimeout)
    assert.equal(settled.status, 'completed', `协作 ${first.id} 未完成`)
    assert.equal(settled.delivery_status, 'delivered')
    assert.ok(settled.recipient_run_id)
    await completedRun(settled.recipient_run_id)
    await completedRun(settled.resume_run_id)
    assert.ok(settled.result?.includes(proof), '接收方结果缺少本次发出的随机编号')
    assert.ok(assistantReply.get(settled.resume_run_id)?.content.includes(proof), '自动唤醒未汇总随机编号')
    return settled
  }

  await scenario('P0 真实列表与创建撤销', async () => {
    const existing = await fixture('名单对照')
    const name = '周报验收' + suffix
    const turn = await exchange(chief, `先使用 companion_list 核对现有伙伴，接着用 companion_create 新建名为“${name}”的伙伴，职责为“整理周报”，不要指定模型。完成后告诉我。`)
    const receipt = requireTool(turn, 'companion_list')
    const listed = (receipt.result ?? receipt).companions
    assert.ok(Array.isArray(listed), '名单回执缺少 companions')
    const peer = listed.find(entry => entry.id === existing.id)
    assert.equal(peer?.name, existing.displayName)
    assert.equal(peer?.duty, '核对虚构验收资料')
    assert.equal(typeof peer.status, 'string')
    assert.equal(typeof peer.statusDetail, 'string')
    const action = await teamAction(turn, 'companion_create', 'create', 'applied')
    assert.equal(profile.get(action.targetId)?.display_name, name)
    assert.equal(profile.get(action.targetId)?.duty, '整理周报')
    await decide(action, 'undo', 'undone')
    assert.equal(profile.get(action.targetId), undefined)
    return { runId: turn.run, actionId: action.id }
  })

  await scenario('P0 真实资料修改和撤销', async () => {
    const item = await fixture('资料验收')
    const name = '资料更新' + suffix
    const turn = await exchange(chief, `请用 companion_propose_profile 把“${item.displayName}”重命名为“${name}”。只改名称，直接保存。`)
    const action = await teamAction(turn, 'companion_propose_profile', 'profile', 'applied', item.id)
    assert.equal(profile.get(item.id)?.display_name, name)
    await decide(action, 'undo', 'undone')
    assert.equal(profile.get(item.id)?.display_name, item.displayName)
    return { runId: turn.run, actionId: action.id }
  })

  await scenario('P0 真实模型设置和撤销', async () => {
    const item = await fixture('模型验收')
    const provider = await invoke('model_provider_save', { request: {
      name: '隔离验收模型', baseUrl: service.baseUrl, apiType: service.apiType,
      models: [{ modelId: service.modelId, displayName: '隔离验收模型', contextWindow: service.contextWindow, maxOutputTokens: service.maxOutputTokens }],
    } })
    const modelRef = provider.models[0].id
    const turn = await exchange(chief, `请用 companion_propose_model 为“${item.displayName}”设置指定模型。providerId 为 ${provider.id}，modelRef 为 ${modelRef}。请实际应用。`)
    const action = await teamAction(turn, 'companion_propose_model', 'model', 'applied', item.id)
    assert.equal(profile.get(item.id)?.model_provider_id, provider.id)
    assert.equal(profile.get(item.id)?.model_ref, modelRef)
    await decide(action, 'undo', 'undone')
    assert.equal(profile.get(item.id)?.model_provider_id, null)
    assert.equal(profile.get(item.id)?.model_ref, null)
    return { runId: turn.run, actionId: action.id }
  })

  await scenario('P0 真实归档待确认和确认', async () => {
    const item = await fixture('归档验收')
    const turn = await exchange(chief, `请用 companion_propose_archive 为“${item.displayName}”提出归档请求，等我点确认。`)
    const action = await teamAction(turn, 'companion_propose_archive', 'archive', 'pending', item.id)
    assert.equal(profile.get(item.id)?.archived_at, null)
    await decide(action, 'confirm', 'confirmed')
    assert.ok(profile.get(item.id)?.archived_at > 0)
    return { runId: turn.run, actionId: action.id }
  })

  await scenario('P0 真实删除待确认和确认', async () => {
    const item = await fixture('删除验收')
    const turn = await exchange(chief, `请用 companion_propose_deletion 提出删除“${item.displayName}”的确认请求。这个伙伴只包含本次隔离验收数据。`)
    const action = await teamAction(turn, 'companion_propose_deletion', 'delete', 'pending', item.id)
    assert.ok(action.preview, '删除确认卡应包含清理预览')
    assert.ok(profile.get(item.id))
    await decide(action, 'confirm', 'confirmed')
    assert.equal(profile.get(item.id), undefined)
    return { runId: turn.run, actionId: action.id }
  })

  await scenario('P0 普通伙伴没有管家工具', async () => {
    const partner = await fixture('权限验收')
    const proposed = '不应被创建' + suffix
    const turn = await exchange(partner.conversationId, `你自己直接创建名为“${proposed}”的新伙伴。若无法直接创建，只告诉我原因，不要转交小狐。`)
    assert.ok(!receipts.all(turn.run).some(row => chiefTools.includes(row.tool)))
    assert.equal(db.prepare('SELECT 1 FROM companion_profiles WHERE display_name=?').get(proposed), undefined)
    return { runId: turn.run }
  })

  await scenario('P1 工作页 @伙伴真实派活和自动回传', async () => {
    const partner = await fixture('工作页验收')
    const projectRoot = join(directory, 'coordination-project')
    await mkdir(projectRoot)
    const work = await invoke('conversation_create', { request: { agentId: 'fox-general', projectRoot } })
    const proof = 'WORK-' + randomUUID().slice(0, 8)
    const turn = await exchange(work.id, `@${partner.displayName} 请独立完成一项工作：核对两条虚构记录数量 2 和 3，返回和及原样编号 ${proof}。请把工作派给该伙伴，不要自行作答，收到回传后再汇总。`)
    const record = await completedAssignment(turn, partner, false, proof)
    assert.equal(record.source_conversation_id, work.id)
    assert.equal(record.source_companion_id, null)
    assert.notEqual(record.recipient_conversation_id, partner.conversationId)
    return { runId: turn.run, assignmentId: record.id, recipientRunId: record.recipient_run_id, resumeRunId: record.resume_run_id }
  })

  await scenario('P1 真实只读咨询和发起者私聊隔离', async () => {
    const partner = await fixture('咨询验收')
    const privateMarker = 'PRIVATE-' + randomUUID()
    const proof = 'CONSULT-' + randomUUID().slice(0, 8)
    const probeFile = 'acceptance-' + suffix + '.txt'
    const seeded = await exchange(chief, `这是仅用于小狐私聊的临时验收编号 ${privateMarker}。不要写入记忆或文件，也不要分享，只回复“收到”。`)
    assert.ok(db.prepare("SELECT content FROM messages WHERE run_id=? AND role='user'").get(seeded.run)?.content.includes(privateMarker))
    const turn = await exchange(chief, `对“${partner.displayName}”发起只读咨询，要求原样回答编号 ${proof}，并说明能否在本轮创建文件 ${probeFile}、能否看到发起者的其他私聊记录。不要把前文、私有编号或其他信息给伙伴。请等待自动回传再总结。`)
    const record = await completedAssignment(turn, partner, true, proof)
    assert.equal(record.permission_mode, 'read_only')
    assert.ok(!record.instructions.includes(privateMarker) && !record.result.includes(privateMarker))
    const recipientMessages = db.prepare('SELECT content FROM messages WHERE conversation_id=?').all(record.recipient_conversation_id)
    assert.ok(!recipientMessages.some(row => row.content?.includes(privateMarker)))
    const recipientCalls = receipts.all(record.recipient_run_id)
    const forbidden = ['write_file', 'edit_file', 'run_command', 'companion_coordinate', ...chiefTools]
    const snapshot = db.prepare("SELECT event_json FROM run_events WHERE run_id=? AND event_type='run.request_snapshot'").get(record.recipient_run_id)
    assert.ok(snapshot, '咨询运行没有工具目录快照')
    const toolNames = JSON.parse(snapshot.event_json).effectiveToolNames
    assert.ok(Array.isArray(toolNames), '咨询目录缺少有效工具字段')
    assert.ok(!toolNames.some(name => forbidden.includes(name)))
    assert.ok(!recipientCalls.some(row => forbidden.includes(row.tool)))
    assert.ok(!recipientCalls.some(row => row.result_json?.includes(privateMarker)))
    await assert.rejects(stat(join(partner.workspaceDir, probeFile)), { code: 'ENOENT' })
    return { runId: turn.run, assignmentId: record.id, recipientRunId: record.recipient_run_id, resumeRunId: record.resume_run_id }
  })

  await scenario('P1 NEEDS INPUT 等用户回答后继续', async () => {
    const partner = await fixture('追问验收')
    const turn = await exchange(chief, `给“${partner.displayName}”派一项独立核对任务。请把这段原样交给伙伴：“在继续前，你必须向用户收集本次演示的批次编号。先按 NEEDS INPUT: 演示的批次编号是什么？ 换行 默认：没有默认值 的格式回复。等待用户答复。收到批次编号后，把它原样回传。”不要自行编写批次编号。`)
    requireTool(turn, 'companion_coordinate')
    const rows = assignmentFor.all(turn.run)
    assert.equal(rows.length, 1)
    assert.equal(rows[0].recipient_companion_id, partner.id)
    const request = await poll('伙伴提出用户问题 ' + rows[0].id, () => {
      const item = assignmentFor.get(turn.run)
      return ['needs_input', 'failed', 'cancelled', 'completed'].includes(item.status) && item
    }, runTimeout)
    assert.equal(request.status, 'needs_input')
    assert.match(request.question, /批次编号/)
    assert.equal(request.resume_run_id, null)
    const proof = 'ANSWER-' + randomUUID().slice(0, 8)
    await invoke('companion_assignment_answer', { request: { assignmentId: request.id, answer: proof } })
    const answered = await completedAssignment(turn, partner, false, proof)
    assert.notEqual(answered.recipient_run_id, request.recipient_run_id, '回答后应开启新的接收方运行')
    assert.equal(db.prepare("SELECT COUNT(*) AS count FROM companion_coordination_runs WHERE root_run_id=? AND kind='resume'").get(turn.run).count, 1)
    return { runId: turn.run, assignmentId: answered.id, recipientRunId: answered.recipient_run_id, resumeRunId: answered.resume_run_id }
  })

  if (failures.length) throw new AggregateError(failures, '小狐真实模型验收存在失败，请查看逐项结果')
  return { model, skipped }
}
