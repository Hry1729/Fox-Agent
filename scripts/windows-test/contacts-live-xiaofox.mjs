import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { connectLiveModel } from './contacts-live-model.mjs'

const timezone = 'Asia/Shanghai'
const dayMs = 86_400_000
const terminal = ['completed', 'failed', 'interrupted', 'cancelled']

export async function checkXiaoFox(context) {
  const { invoke, db, check, poll, restartDesktop, directory } = context
  const { model, exchange } = await connectLiveModel(context)
  const contact = await invoke('companion_create_blank')
  assert.equal(contact.templateAgentId, null)
  assert.equal(contact.workspaceState, 'ready')
  const workspace = await realpath(contact.workspaceDir)
  const expectedWorkspace = await realpath(join(directory, 'data', 'companions', contact.id, 'workspace'))
  assert.equal(workspace.toLowerCase(), expectedWorkspace.toLowerCase(), '小 Fox 工作区不在本次验收目录')

  const id = randomUUID().slice(0, 8)
  const request = { companionId: contact.id }
  const invokeTask = (name, taskId) => invoke('companion_task_' + name, { request: { ...request, taskId } })
  const failures = []
  const filter = process.env.FOX_CONTACTS_LIVE_FILTER?.trim()
  const pattern = filter ? new RegExp(filter) : null
  console.log('小 Fox 真实模型验收范围：' + (filter || '全量'))
  const scenario = async (name, action) => {
    if (pattern && !pattern.test(name) && !name.startsWith('停用')) {
      console.log('[未执行] ' + name)
      return
    }
    try {
      await check(name, action)
    } catch (error) {
      failures.push(error)
    }
  }
  const requireTool = (turn, tool) => assert.ok(
    turn.tools.some(call => call.tool === tool && call.state === 'completed'),
    `缺少成功工具回执 ${tool}，Run ${turn.run}`,
  )
  const delegation = (objective) => [
    '必须使用 child_run_start 启动一个 worker 子任务。',
    `子任务要求：${objective}`,
    '收到真实 childRunId 后，使用 child_run_collect 等待结果，回复子任务返回的原始编号。',
    '主任务不要自行代做，不调用其他工具，不写文件，不发送外部消息。',
  ].join('\n')
  const observedDelegation = async (runId) => {
    const root = await poll('父 Run 结束', () => {
      const row = db.prepare('SELECT id, status, error_code, budget_json FROM runs WHERE id = ?').get(runId)
      return terminal.includes(row?.status) && row
    }, 300_000)
    assert.equal(root.status, 'completed', root.error_code ?? '父 Run 未成功完成')
    const children = db.prepare(
      "SELECT id, parent_run_id, root_run_id, status FROM runs WHERE root_run_id = ? AND run_kind = 'child'",
    ).all(runId)
    assert.equal(children.length, 1, '应有一个真实子 Run')
    const [child] = children
    assert.equal(child.parent_run_id, runId)
    assert.equal(child.root_run_id, runId)
    assert.equal(child.status, 'completed')
    for (const tool of ['child_run_start', 'child_run_collect']) {
      assert.ok(db.prepare("SELECT 1 FROM kernel_tool_calls WHERE run_id = ? AND tool = ? AND state = 'completed'").get(runId, tool),
        `父 Run 缺少已完成的 ${tool}`)
    }
    return { root, child }
  }
  const assertReply = (runId, proof) => {
    const reply = db.prepare("SELECT content FROM messages WHERE run_id = ? AND role = 'assistant' ORDER BY ordinal DESC LIMIT 1").get(runId)
    assert.ok(reply?.content.includes(proof), `Run ${runId} 未回传测试编号`)
  }

  try {
    await scenario('小 Fox A05 真实对话更新资料、侧栏同步及重启持久化', async () => {
      const expected = {
        displayName: '验收小 Fox ' + id,
        duty: '整理虚构的验收台账 ' + id,
        personaOverride: '回答先给结论 ' + id,
      }
      const turn = await exchange(contact.conversationId, [
        '请用 companion_profile_update 永久更新你自己的资料。以下三项按原文保存。',
        `名称：${expected.displayName}`,
        `职责：${expected.duty}`,
        `沟通偏好：${expected.personaOverride}（整体替换现有偏好）`,
      ].join('\n'))
      requireTool(turn, 'companion_profile_update')
      const verifyProfile = async () => {
        const saved = await invoke('companion_open', { request })
        for (const [key, value] of Object.entries(expected)) assert.equal(saved[key], value, key)
        const visible = await invoke('acceptance.read_xiaofox_profile', { displayName: expected.displayName })
        assert.deepEqual(visible, expected)
      }
      await verifyProfile()
      await restartDesktop()
      await verifyProfile()
      return { runId: turn.run, sideCardSynced: true, persistedAfterRestart: true }
    })

    await scenario('小 Fox A11 真实对话创建单次、间隔和每周计划', async () => {
      const onceAt = Date.now() + 3 * dayMs
      const intervalSeconds = 86_400
      const localDate = new Date(Date.now() + 4 * dayMs + 8 * 3_600_000)
      localDate.setUTCHours(9, 0, 0, 0)
      const weekday = localDate.getUTCDay()
      const calendarDue = localDate.getTime() - 8 * 3_600_000
      const objective = '只在本地返回虚构验收编号，不写文件，不发送外部消息。'
      const cases = [
        { name: '单次验收 ' + id, schedule: { kind: 'once', atMs: onceAt, timezone } },
        { name: '间隔验收 ' + id, schedule: { kind: 'interval', intervalSeconds, timezone } },
        { name: '每周验收 ' + id, schedule: { kind: 'weekly', weekdays: [weekday], localTime: '09:00', timezone } },
      ]
      const verified = []
      for (const plan of cases) {
        const turn = await exchange(contact.conversationId, [
          '请调用 companion_task_create 原样保存这条定时计划，保持启用，不立即执行。',
          JSON.stringify({ ...plan, objective, enabled: true }),
        ].join('\n'))
        requireTool(turn, 'companion_task_create')
        const matches = (await invokeTask('list')).filter(entry => entry.task.name === plan.name)
        assert.equal(matches.length, 1)
        const [{ task, schedule }] = matches
        assert.equal(task.companionId, contact.id)
        assert.equal(task.expertId, null)
        assert.equal(schedule.scheduleKind, plan.schedule.kind)
        assert.equal(schedule.timezone, timezone)
        assert.equal(schedule.enabled, true)
        if (plan.schedule.kind === 'interval') {
          assert.equal(schedule.intervalSeconds, intervalSeconds)
          assert.ok(Number.isSafeInteger(schedule.anchorAt))
          assert.equal(schedule.nextDueAt, schedule.anchorAt + intervalSeconds * 1000)
        } else if (plan.schedule.kind === 'once') {
          assert.equal(schedule.onceAt, onceAt)
          assert.equal(schedule.nextDueAt, onceAt)
        } else {
          assert.deepEqual(schedule.weekdays, [weekday])
          assert.equal(schedule.localTime, '09:00')
          assert.equal(schedule.nextDueAt, calendarDue)
        }
        assert.ok(schedule.nextDueAt > Date.now())
        const paused = await invokeTask('pause', task.id)
        assert.equal(paused.schedule.enabled, false)
        verified.push({ runId: turn.run, taskId: task.id, kind: schedule.scheduleKind, nextDueAt: schedule.nextDueAt, timezone })
      }
      const unsupported = await exchange(contact.conversationId,
        '请给当前小 Fox 新建每月第一天上午 9 点的定时任务。请按每月执行，不换成其他周期，不立即运行。')
      assert.match(unsupported.reply, /不支持|不能|无法|不具备/)
      assert.deepEqual(
        (await invokeTask('list')).map(entry => entry.task.id).sort(),
        verified.map(entry => entry.taskId).sort(),
        '不支持的规则不得另存为近似计划',
      )
      return verified
    })

    await scenario('小 Fox A16 与 A23 真实模型私聊委派及等待', async () => {
      const proof = 'PRIVATE-' + randomUUID()
      const before = (await invoke('companions_list')).companions.map(item => item.id).sort()
      const turn = await exchange(contact.conversationId, delegation('只返回这个虚构编号：' + proof))
      const { child } = await observedDelegation(turn.run)
      assertReply(child.id, proof)
      assertReply(turn.run, proof)
      const after = (await invoke('companions_list')).companions.map(item => item.id).sort()
      assert.deepEqual(after, before, '临时子任务不得创建联系人')
      return { runId: turn.run, childRunId: child.id, contacts: after.length }
    })

    await scenario('小 Fox A17 与 A23 真实定时任务委派、子 Run 读工作区及根预算', async () => {
      const proof = 'TASK-' + randomUUID()
      const filename = 'xiaofox-acceptance-' + id + '.txt'
      await writeFile(join(workspace, filename), proof, { flag: 'wx' })
      const owned = await invoke('companion_task_create', { request: {
        ...request,
        name: '预算验收 ' + id,
        objective: delegation(`必须用 read 工具读取小 Fox 工作目录中的 ${filename}，只返回文件里的虚构编号。`),
        schedule: { kind: 'once', atMs: Date.now() + 3 * dayMs, timezone },
        enabled: false,
      } })
      const started = await invokeTask('run_now', owned.task.id)
      assert.ok(started.runId)
      const { root, child } = await observedDelegation(started.runId)
      assertReply(child.id, proof)
      assertReply(root.id, proof)
      const childRead = db.prepare("SELECT 1 FROM kernel_tool_calls WHERE run_id = ? AND tool = 'read' AND state = 'completed'").get(child.id)
      assert.ok(childRead, '子 Run 应在所属工作区完成 read')
      const persisted = await poll('定时任务回报', () => db.prepare(
        'SELECT companion_id, report_conversation_id, outcome FROM companion_task_reports WHERE run_id = ?',
      ).get(root.id))
      assert.equal(persisted.companion_id, contact.id)
      assert.equal(persisted.report_conversation_id, contact.conversationId)
      const [trigger] = (await invokeTask('history', owned.task.id)).filter(row => row.runId === root.id)
      assert.equal(trigger?.status, 'completed')
      const latestUsage = db.prepare(
        "SELECT event_json FROM run_events WHERE run_id = ? AND event_type = 'usage.updated' ORDER BY seq DESC LIMIT 1",
      )
      const usage = [root.id, child.id].map(runId => {
        const event = latestUsage.get(runId)
        assert.ok(event, `Run ${runId} 缺少实际用量`)
        return JSON.parse(event.event_json)
      })
      for (const key of ['inputTokens', 'outputTokens', 'totalTokens']) {
        assert.equal(trigger[key], usage[0][key] + usage[1][key], key)
      }
      const toolCount = db.prepare('SELECT count(*) AS total FROM tool_calls WHERE run_id IN (?, ?)').get(root.id, child.id).total
      assert.equal(trigger.toolCallCount, toolCount)
      const { maxToolCalls } = JSON.parse(root.budget_json)
      assert.equal(maxToolCalls, 16)
      assert.ok(toolCount > 0 && toolCount <= maxToolCalls)
      return { runId: root.id, childRunId: child.id, totalTokens: trigger.totalTokens, toolCount, maxToolCalls }
    })
  } finally {
    await scenario('停用本次验收创建的全部计划', async () => {
      const entries = await invokeTask('list')
      const results = await Promise.allSettled(entries.filter(entry => entry.schedule.enabled)
        .map(async entry => assert.equal((await invokeTask('pause', entry.task.id)).schedule.enabled, false)))
      assert.ok(results.every(result => result.status === 'fulfilled'), '至少一条验收计划未能停用')
      assert.ok((await invokeTask('list')).every(entry => !entry.schedule.enabled))
      return { disabledPlans: entries.length }
    })
  }
  if (failures.length) throw new AggregateError(failures, '小 Fox 真实验收存在失败')
  return { model, selection: { scope: filter ? 'partial' : 'full', filter: filter || null }, hostCoverage: 'A17 超额终止及重复用量、A23 越界/撤权/拒绝直接请求/停止传播由独立 Host 测试判定' }
}
