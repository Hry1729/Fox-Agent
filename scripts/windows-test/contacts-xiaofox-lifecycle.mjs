import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { readFile, readdir, realpath, writeFile } from 'node:fs/promises'
import { basename, join } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'

const timezone = 'Asia/Shanghai'
const terminal = new Set(['completed', 'failed', 'interrupted', 'cancelled'])
const schedulerPeriodMs = 10_000

export async function checkXiaoFoxLifecycle({ invoke, db, check, poll, restartDesktop, directory, endpoint }) {
  const id = randomUUID().slice(0, 8)
  const failures = []
  const startedRuns = new Set()
  const scenario = async (name, action) => {
    try {
      await check(name, action)
    } catch (error) {
      failures.push(error)
      for (const runId of startedRuns) {
        try {
          const row = status.get(runId)
          if (row && !terminal.has(row.status)) {
            await invoke('run_cancel', { request: { runId } })
            await runFinish(runId)
          }
        } catch (cleanupError) {
          failures.push(new Error(`停止验收残留 Run ${runId} 失败`, { cause: cleanupError }))
        }
      }
    } finally {
      startedRuns.clear()
    }
  }
  await invoke('model_service_save', { request: {
    name: 'Xiao Fox lifecycle acceptance',
    baseUrl: `${endpoint}/${id}/v1`,
    modelId: 'fox-kernel-acceptance',
    apiType: 'openai-completions',
    contextWindow: 32_000,
    maxOutputTokens: 4096,
  } })

  const owner = contact => ({ companionId: contact.id })
  const task = (action, contact, taskId) => invoke('companion_task_' + action, { request: { ...owner(contact), taskId } })
  const createTask = (contact, name, objective, schedule, enabled = true) => invoke('companion_task_create', {
    request: { ...owner(contact), name: name + ' ' + id, objective, schedule, enabled },
  })
  const once = atMs => ({ kind: 'once', atMs, timezone })
  const status = db.prepare('SELECT status, error_code, started_at, finished_at FROM runs WHERE id = ?')
  const runStart = async (contact, text) => {
    const { run } = await invoke('run_start', { request: { conversationId: contact.conversationId, text } })
    startedRuns.add(run.id)
    return run.id
  }
  const runFinish = runId => poll('运行结束 ' + runId, () => {
    const row = status.get(runId)
    return terminal.has(row?.status) && row
  })
  const assertComplete = async runId => {
    const row = await runFinish(runId)
    assert.equal(row.status, 'completed', row.error_code ?? runId)
    return row
  }
  const taskHistory = (contact, taskId) => task('history', contact, taskId)

  await scenario('小 Fox A02 同专家创建两个独立联系人并分别发起对话', async () => {
    const add = () => invoke('companion_add_from_expert', { request: { expertId: 'fox-debugger' } })
    const first = await add()
    const second = await add()
    assert.equal(first.templateAgentId, 'fox-debugger')
    assert.equal(second.templateAgentId, 'fox-debugger')
    assert.notEqual(first.id, second.id)
    assert.notEqual(first.conversationId, second.conversationId)
    assert.notEqual((await realpath(first.workspaceDir)).toLowerCase(), (await realpath(second.workspaceDir)).toLowerCase())
    const duties = [`第一份职责 ${id}`, `第二份职责 ${id}`]
    const runs = []
    for (const [index, contact] of [first, second].entries()) {
      await invoke('companion_update_profile', { request: { ...owner(contact), duty: duties[index] } })
      const runId = await runStart(contact, `独立会话验收 ${index + 1} ${id}`)
      await assertComplete(runId)
      runs.push(runId)
    }
    for (const [index, contact] of [first, second].entries()) {
      assert.equal((await invoke('companion_open', { request: owner(contact) })).duty, duties[index])
    }
    return { contacts: [first.id, second.id], runs }
  })

  await scenario('小 Fox A03 空白新建隔离默认小狐的聊天、笔记和文件', async () => {
    const { companions } = await invoke('companions_list')
    const builtin = companions.find(contact => contact.isDefault)
    assert.ok(builtin, '缺少默认小狐')
    const proof = `DEFAULT-PRIVATE-${id}`
    const runId = await runStart(builtin, proof)
    await assertComplete(runId)
    await invoke('companion_note_save_user', { ...owner(builtin), content: proof })
    assert.ok(JSON.stringify(await invoke('companion_notes_get', owner(builtin))).includes(proof))
    const source = join(directory, `default-private-${id}.txt`)
    await writeFile(source, proof, { flag: 'wx' })
    const [owned] = await invoke('companion_import_files', { ...owner(builtin), paths: [source] })
    assert.equal(await readFile(owned, 'utf8'), proof)

    const blank = await invoke('companion_create_blank')
    assert.equal(blank.templateAgentId, null)
    const notes = await invoke('companion_notes_get', owner(blank))
    assert.equal(notes.usedChars, 0)
    assert.deepEqual(notes.topics, [])
    assert.equal(notes.draft, null)
    const messages = db.prepare('SELECT role, content FROM messages WHERE conversation_id = ?').all(blank.conversationId)
    assert.ok(messages.every(message => message.role === 'assistant' && !message.content.includes(proof)))
    const files = await readdir(blank.workspaceDir, { recursive: true })
    assert.ok(files.every(path => basename(path) !== basename(owned)))
    return { builtin: builtin.id, blank: blank.id, seedRun: runId, blankMessages: messages.length }
  })

  await scenario('小 Fox A14 暂停无触发、归档停用、恢复保持停用', async () => {
    const contact = await invoke('companion_create_blank')
    const atMs = Date.now() + 5_000
    const paused = await createTask(contact, '暂停验证', '只回复计划暂停验收。', once(atMs))
    assert.equal((await task('pause', contact, paused.task.id)).schedule.enabled, false)
    await sleep(Math.max(0, atMs + schedulerPeriodMs + 500 - Date.now()))
    assert.deepEqual(await taskHistory(contact, paused.task.id), [])

    const enabled = await createTask(contact, '归档验证', '只回复计划归档验收。', once(Date.now() + 86_400_000))
    assert.equal(enabled.schedule.enabled, true)
    assert.ok((await invoke('companion_archive', { request: owner(contact) })).archivedAt)
    const archivedSchedule = db.prepare('SELECT enabled FROM digital_colleague_schedules WHERE id = ?').get(enabled.schedule.id)
    assert.equal(archivedSchedule.enabled, 0)
    assert.equal((await invoke('companion_restore', { request: owner(contact) })).archivedAt, null)
    const restored = await task('list', contact)
    assert.equal(restored.length, 2)
    assert.ok(restored.every(entry => !entry.schedule.enabled))
    return { pausedTaskId: paused.task.id, archivedTaskId: enabled.task.id, disabledAfterRestore: true }
  })

  await scenario('小 Fox A15 桌面离线期间到期的计划重启后只补跑一次', async () => {
    const contact = await invoke('companion_create_blank')
    const atMs = Date.now() + 12_000
    const plan = await createTask(contact, '重启补跑', '只回复计划补跑验收。', once(atMs))
    const persistedTriggers = db.prepare('SELECT id FROM digital_colleague_triggers WHERE colleague_id = ?')
    let triggersBeforeRestart
    await restartDesktop(async () => {
      triggersBeforeRestart = persistedTriggers.all(plan.task.id)
      await sleep(Math.max(0, atMs + 250 - Date.now()))
    })
    assert.deepEqual(triggersBeforeRestart, [], '到期前应先退出桌面')
    const executed = await poll('重启后补跑完成', async () => {
      const [trigger] = await taskHistory(contact, plan.task.id)
      return trigger?.status === 'completed' && trigger
    })
    assert.ok(executed.runId)
    assert.equal(executed.scheduledFor, atMs)
    const row = await assertComplete(executed.runId)
    assert.ok(row.started_at >= atMs)
    await restartDesktop()
    await sleep(schedulerPeriodMs + 500)
    const history = await taskHistory(contact, plan.task.id)
    assert.deepEqual(history.map(trigger => trigger.id), [executed.id])
    assert.equal((await task('list', contact))[0].schedule.enabled, false)
    return { taskId: plan.task.id, triggerId: executed.id, runId: executed.runId, scheduledFor: atMs }
  })

  const childMessage = db.prepare(`SELECT m.content FROM messages m
    WHERE m.run_id = ? AND m.role = 'assistant' AND length(m.content) > 0 ORDER BY m.ordinal DESC LIMIT 1`)
  const activeChild = db.prepare("SELECT id FROM runs WHERE parent_run_id = ? AND run_kind = 'child' AND status = 'running'")
  const waitForChildOutput = parentRunId => poll('子 Run 正在输出', () => {
    const child = activeChild.get(parentRunId)
    const partial = child && childMessage.get(child.id)?.content
    return partial && { id: child.id, partial }
  })
  const assertStopped = async (parentRunId, child) => {
    const [parent, settled] = await Promise.all([runFinish(parentRunId), runFinish(child.id)])
    assert.ok(['cancelled', 'interrupted'].includes(parent.status), JSON.stringify(parent))
    assert.ok(['cancelled', 'interrupted'].includes(settled.status), JSON.stringify(settled))
    assert.ok(childMessage.get(child.id)?.content.startsWith(child.partial), '停止后应保留已输出的内容')
    return { parentRunId, childRunId: child.id, parentStatus: parent.status, childStatus: settled.status, retainedChars: child.partial.length }
  }

  await scenario('小 Fox A18 停止私聊父 Run 同时停止子 Run 并保留部分输出', async () => {
    const contact = await invoke('companion_create_blank')
    const parentRunId = await runStart(contact, '[XIAOFOX_A18] 私聊停止传播验收')
    const child = await waitForChildOutput(parentRunId)
    assert.equal(await invoke('run_cancel', { request: { runId: parentRunId } }), true)
    return assertStopped(parentRunId, child)
  })

  await scenario('小 Fox A18 归档同时停止正在执行的后台父子 Run', async () => {
    const contact = await invoke('companion_create_blank')
    const plan = await createTask(contact, '归档停止传播', '[XIAOFOX_A18] 后台停止传播验收', once(Date.now() + 86_400_000), false)
    const { runId } = await task('run_now', contact, plan.task.id)
    assert.ok(runId)
    startedRuns.add(runId)
    const child = await waitForChildOutput(runId)
    assert.ok((await invoke('companion_archive', { request: owner(contact) })).archivedAt)
    const stopped = await assertStopped(runId, child)
    return { ...stopped, taskId: plan.task.id }
  })

  await scenario('小 Fox A18 删除先停止父子 Run，再按刷新后的清单删除', async () => {
    const contact = await invoke('companion_create_blank')
    const { planHash } = await invoke('companion_delete_preview', owner(contact))
    const parentRunId = await runStart(contact, '[XIAOFOX_A18] 删除停止传播验收')
    const child = await waitForChildOutput(parentRunId)
    await assert.rejects(() => invoke('companion_delete', { ...owner(contact), planHash }), /companion\.plan_changed/)
    const stopped = await assertStopped(parentRunId, child)
    const refreshed = await invoke('companion_delete_preview', owner(contact))
    assert.notEqual(refreshed.planHash, planHash)
    await invoke('companion_delete', { ...owner(contact), planHash: refreshed.planHash })
    assert.ok(!(await invoke('companions_list')).companions.some(item => item.id === contact.id))
    assert.equal(status.get(parentRunId), undefined)
    assert.equal(status.get(child.id), undefined)
    return { ...stopped, deletedContact: contact.id }
  })

  await scenario('小 Fox A24 私聊运行期间计划到期排队，私聊完成后执行', async () => {
    const contact = await invoke('companion_create_blank')
    const privateRunId = await runStart(contact, '[XIAOFOX_A24_PRIVATE] 私聊占用执行位置')
    await poll('私聊已运行', () => status.get(privateRunId)?.status === 'running')
    const atMs = Date.now() + 3_000
    const plan = await createTask(contact, '排队验收', '只回复后台计划验收。', once(atMs))
    const queued = await poll('任务历史显示排队', async () => {
      const [trigger] = await taskHistory(contact, plan.task.id)
      return trigger?.status === 'queued' && trigger
    })
    assert.ok(queued.runId)
    startedRuns.add(queued.runId)
    assert.equal(queued.scheduledFor, atMs)
    assert.equal(status.get(queued.runId).status, 'queued')
    assert.equal(status.get(privateRunId).status, 'running')
    const privateFinished = await assertComplete(privateRunId)
    const taskFinished = await assertComplete(queued.runId)
    assert.ok(taskFinished.started_at >= privateFinished.finished_at)
    const history = await poll('任务历史显示完成', async () => {
      const [trigger] = await taskHistory(contact, plan.task.id)
      return trigger?.status === 'completed' && trigger
    })
    assert.equal(history.id, queued.id)
    return { privateRunId, taskRunId: queued.runId, taskId: plan.task.id, queuedTriggerId: queued.id }
  })

  if (failures.length) throw new AggregateError(failures, '小 Fox 生命周期验收存在失败')
  return { model: 'fox-kernel-acceptance', dailyBudgetBoundary: '由注入时钟的仓储测试覆盖' }
}
