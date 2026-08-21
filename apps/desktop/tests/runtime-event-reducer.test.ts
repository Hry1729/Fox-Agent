import { describe, expect, test } from 'bun:test'
import { applyWorkEvent, mergeConversationDetail, reduceRuntimeNotifications } from '../src/features/conversations/model/runtime-event-reducer'
import { pendingRuntimeQuestion } from '../src/features/conversations/model/pending-interactions'
import { enqueueRuntimeEvent, takeRuntimeEventFrame } from '../src/features/conversations/model/runtime-event-queue'
import type { ConversationDetail, RuntimeEventNotification, WorkEventRecord } from '../src/features/conversations/model/types'

function detail(): ConversationDetail {
  return {
    conversation: {
      id: 'conversation-1', agentId: 'fox-general', agentName: 'Fox', title: 'test',
      projectId: null, projectRoot: null, status: 'active', createdAt: 1, updatedAt: 1, lastMessageAt: 1,
    },
    messages: [{
      id: 'user-1', conversationId: 'conversation-1', runId: 'run-1', role: 'user', kind: 'text',
      content: 'hello', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1,
    }],
    runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [],
    lastRun: {
      id: 'run-1', conversationId: 'conversation-1', runtimeSessionId: null, status: 'queued',
      model: 'test-model', startedAt: null, finishedAt: null, errorCode: null, errorMessage: null, lastSeq: 0,
    },
    hasEarlierMessages: false,
    goals: [],
    tasks: [],
    evidence: [],
  }
}

function notification(seq: number, event: RuntimeEventNotification['event']): RuntimeEventNotification {
  return {
    conversationId: 'conversation-1', runtimeSessionId: null, runId: 'run-1', seq,
    timestamp: new Date(seq * 1000).toISOString(), event,
  }
}

describe('runtime event reducer', () => {
  test('keeps the persisted final answer when a later terminal event only updates status', () => {
    const persisted = detail()
    persisted.lastRun = { ...persisted.lastRun!, status: 'completed', lastSeq: 7, finishedAt: 7000 }
    persisted.messages.push({
      id: 'assistant-run-1', conversationId: 'conversation-1', runId: 'run-1', role: 'assistant', kind: 'text',
      content: 'The complete persisted answer', status: 'completed', ordinal: 2, createdAt: 3000, updatedAt: 6999,
    })
    const live = detail()
    live.lastRun = { ...live.lastRun!, status: 'completed', lastSeq: 7, finishedAt: 7000 }
    live.messages.push({
      id: 'assistant-run-1', conversationId: 'conversation-1', runId: 'run-1', role: 'assistant', kind: 'text',
      content: '', status: 'completed', ordinal: 2, createdAt: 3000, updatedAt: 7000,
    })

    const merged = mergeConversationDetail(persisted, live)
    expect(merged.messages.find((message) => message.id === 'assistant-run-1')).toMatchObject({
      content: 'The complete persisted answer', status: 'completed', updatedAt: 7000,
    })
  })

  test('does not regress an optimistically resolved approval back to pending', () => {
    const persisted = detail()
    persisted.approvals = [{
      id: 'approval-1', toolCallId: 'tool-1', runId: 'run-1', conversationId: 'conversation-1',
      toolName: 'write_file', status: 'pending', requestedAction: 'write', request: {}, decision: null,
      requestedAt: 1000, resolvedAt: null,
    }]
    const current = detail()
    current.approvals = [{
      ...persisted.approvals[0], status: 'approved', decision: { approved: true }, resolvedAt: 2000,
    }]

    expect(mergeConversationDetail(persisted, current).approvals[0]).toMatchObject({
      status: 'approved', resolvedAt: 2000,
    })
  })

  test('accepts live events that arrive before run_start returns', () => {
    const pending = detail()
    pending.lastRun = {
      ...pending.lastRun!,
      id: 'pending-run-local',
      status: 'queued',
      lastSeq: 0,
    }

    const next = reduceRuntimeNotifications(pending, [
      notification(1, { type: 'run.started' }),
      notification(2, { type: 'reasoning.delta', delta: 'planning', source: 'test' }),
      notification(3, { type: 'message.delta', delta: 'Live reply' }),
    ])

    expect(next.lastRun).toMatchObject({ id: 'run-1', status: 'running', lastSeq: 3 })
    expect(next.messages.find((message) => message.role === 'assistant')).toMatchObject({
      id: 'assistant-run-1', content: 'Live reply', status: 'streaming',
    })
    expect(next.runtimeEvents.map((event) => event.eventType)).toEqual([
      'run.started', 'reasoning.delta',
    ])
  })

  test('creates and updates one stable assistant message from live deltas', () => {
    const next = reduceRuntimeNotifications(detail(), [
      notification(1, { type: 'run.started' }),
      notification(2, { type: 'reasoning.delta', delta: 'checking', source: 'test' }),
      notification(3, { type: 'message.started' }),
      notification(4, { type: 'message.delta', delta: 'Hello ' }),
      notification(5, { type: 'message.delta', delta: 'world' }),
      notification(6, { type: 'message.completed' }),
      notification(7, { type: 'run.completed' }),
    ])
    const assistants = next.messages.filter((message) => message.role === 'assistant')
    expect(assistants).toHaveLength(1)
    expect(assistants[0]).toMatchObject({ id: 'assistant-run-1', content: 'Hello world', status: 'completed' })
    expect(next.runtimeEvents.map((event) => event.eventType)).toEqual([
      'run.started', 'reasoning.delta', 'message.started', 'message.completed', 'run.completed',
    ])
    expect(next.lastRun?.status).toBe('completed')
  })

  test('creates a completed assistant message when completion arrives without a delta', () => {
    const next = reduceRuntimeNotifications(detail(), [
      notification(1, { type: 'run.started' }),
      notification(2, { type: 'run.completed' }),
    ])
    expect(next.messages.find((message) => message.role === 'assistant')).toMatchObject({
      id: 'assistant-run-1', status: 'completed',
    })
  })

  test('clears a transient Yuxi interruption when a recovered run starts and completes', () => {
    const interrupted = detail()
    interrupted.lastRun = {
      ...interrupted.lastRun!,
      status: 'interrupted',
      errorCode: 'yuxi.connection_interrupted',
      errorMessage: 'stream disconnected',
      lastSeq: 2,
      finishedAt: 2000,
    }
    const next = reduceRuntimeNotifications(interrupted, [
      notification(3, { type: 'run.started', runtime: 'yuxi', resumed: true }),
      notification(4, { type: 'message.started' }),
      notification(5, { type: 'message.delta', delta: 'Recovered answer' }),
      notification(6, { type: 'message.completed' }),
      notification(7, { type: 'run.completed' }),
    ])

    expect(next.lastRun).toMatchObject({
      status: 'completed', errorCode: null, errorMessage: null, lastSeq: 7,
    })
    expect(next.messages.find((message) => message.role === 'assistant')).toMatchObject({
      content: 'Recovered answer', status: 'completed',
    })
  })

  test('keeps late interaction events from an older run without replacing the active run', () => {
    const current = detail()
    current.lastRun = { ...current.lastRun!, id: 'run-2', status: 'running', lastSeq: 3 }
    const lateQuestion: RuntimeEventNotification = {
      ...notification(4, {
        type: 'user.question.requested',
        questions: [{ questionId: 'scene', question: '选择场景' }],
      }),
      runId: 'run-1',
    }

    const next = reduceRuntimeNotifications(current, [lateQuestion])
    expect(next.lastRun).toMatchObject({ id: 'run-2', status: 'running', lastSeq: 3 })
    expect(next.runtimeEvents).toContainEqual(expect.objectContaining({
      runId: 'run-1', eventType: 'user.question.requested',
    }))
  })

  test('ignores terminal events from a background conversation', () => {
    const current = detail()
    const background: RuntimeEventNotification = {
      ...notification(8, { type: 'run.completed' }),
      conversationId: 'conversation-background',
      runId: 'run-background',
    }

    const next = reduceRuntimeNotifications(current, [background])
    expect(next).toBe(current)
    expect(next.conversation.id).toBe('conversation-1')
    expect(next.lastRun).toMatchObject({ id: 'run-1', status: 'queued' })
  })

  test('reduces out-of-order duplicate work events deterministically', () => {
    const events = [
      notification(1, {
        type: 'goal.activated', goalId: 'goal-1',
        data: { goal: {
          id: 'goal-1', title: 'Fix cross-file bug',
          objective: 'Locate, modify and verify', status: 'active', version: 1,
        } },
      }),
      notification(2, {
        type: 'task.created', goalId: 'goal-1', taskId: 'task-1',
        data: { task: {
          id: 'task-1', goalId: 'goal-1', ordinal: 0, title: 'Locate', version: 1,
        } },
      }),
      notification(3, {
        type: 'task.started', goalId: 'goal-1', taskId: 'task-1',
        data: { task: {
          id: 'task-1', ownerRunId: 'run-1', attempt: 1, version: 2,
        } },
      }),
      notification(4, {
        type: 'evidence.added', goalId: 'goal-1', taskId: 'task-1',
        data: { evidence: {
          id: 'evidence-1', taskId: 'task-1', evidenceType: 'tool_call',
          refKind: 'tool_call', refId: 'tool-1', summary: 'Located the defect',
        } },
      }),
      notification(5, {
        type: 'task.completed', goalId: 'goal-1', taskId: 'task-1',
        data: { task: { id: 'task-1', version: 3 } },
      }),
    ]

    const next = reduceRuntimeNotifications(detail(), [
      events[4], events[3], events[2], events[1], events[0],
      events[3], events[1],
    ])

    expect(next.goals).toEqual([
      expect.objectContaining({ id: 'goal-1', status: 'active', version: 1 }),
    ])
    expect(next.tasks).toEqual([
      expect.objectContaining({ id: 'task-1', status: 'completed', attempt: 1 }),
    ])
    expect(next.evidence).toEqual([
      expect.objectContaining({ id: 'evidence-1', taskId: 'task-1' }),
    ])
    expect(next.runtimeEvents).toHaveLength(5)
    expect(next.lastRun?.lastSeq).toBe(5)
  })

  test('persists runtime diagnostics without creating assistant text', () => {
    const next = reduceRuntimeNotifications(detail(), [
      notification(1, { type: 'run.request_snapshot', model: 'model-1', provider: 'provider-1', toolNames: ['read'] }),
      notification(2, { type: 'run.phase', phase: 'preparing' }),
      notification(3, { type: 'run.retrying', attempt: 1, maxAttempts: 2 }),
      notification(4, { type: 'run.retry.completed', success: true, attempt: 1 }),
      notification(5, { type: 'context.compaction.started', reason: 'threshold' }),
      notification(6, { type: 'context.compaction.completed', reason: 'threshold', aborted: false }),
      notification(7, { type: 'planner.started', model: 'model-1' }),
      notification(8, { type: 'planner.completed', stepCount: 3, planHash: 'plan-hash' }),
    ])

    expect(next.runtimeEvents.map((event) => event.eventType)).toEqual([
      'run.request_snapshot',
      'run.phase',
      'run.retrying',
      'run.retry.completed',
      'context.compaction.started',
      'context.compaction.completed',
      'planner.started',
      'planner.completed',
    ])
    expect(next.messages.filter((message) => message.role === 'assistant')).toHaveLength(0)
  })

  test('does not create an empty goal from a status-only live event', () => {
    const next = reduceRuntimeNotifications(detail(), [
      notification(1, {
        type: 'goal.activated',
        goalId: 'goal-without-payload',
      }),
    ])

    expect(next.goals).toEqual([])
    expect(next.runtimeEvents).toHaveLength(1)
  })

  test('updates a host placeholder when the runtime refines the active goal', () => {
    const placeholder = reduceRuntimeNotifications(detail(), [
      notification(1, {
        type: 'goal.activated', goalId: 'goal-1',
        data: { goal: {
          id: 'goal-1', title: '列个目标，再制定计划，依次修改这几个问题',
          objective: '列个目标，再制定计划，依次修改这几个问题', status: 'active', version: 1,
        } },
      }),
    ])
    const refined = reduceRuntimeNotifications(placeholder, [
      notification(2, {
        type: 'goal.activated', goalId: 'goal-1',
        data: { goal: {
          id: 'goal-1', title: '修复 AI Essentials 文档问题',
          objective: '按优先级修正 README 与源码中的六项问题',
          acceptanceSummary: '逐项修改并验证', status: 'active', version: 2,
        } },
      }),
    ])

    expect(refined.goals).toEqual([
      expect.objectContaining({
        id: 'goal-1', title: '修复 AI Essentials 文档问题',
        objective: '按优先级修正 README 与源码中的六项问题',
        acceptanceSummary: '逐项修改并验证', status: 'active', version: 2,
      }),
    ])
  })

  test('applies the dedicated Work Event stream without advancing Runtime sequence or messages', () => {
    const current = detail()
    current.lastRun = { ...current.lastRun!, status: 'awaiting_confirmation', lastSeq: 0 }
    const event: WorkEventRecord = {
      type: 'goal.proposed',
      schemaVersion: 1,
      conversationId: 'conversation-1',
      goalId: 'goal-1',
      taskId: null,
      runId: 'run-1',
      traceId: null,
      spanId: null,
      sequence: 7,
      timestamp: new Date(7000).toISOString(),
      data: {
        goal: {
          id: 'goal-1', title: 'Confirm work mode', objective: 'Wait for the user',
          status: 'proposed', version: 1,
        },
      },
    }

    const next = applyWorkEvent(current, event)!
    expect(next.goals).toEqual([
      expect.objectContaining({ id: 'goal-1', status: 'proposed', version: 1 }),
    ])
    expect(next.lastRun).toEqual(current.lastRun)
    expect(next.messages).toEqual(current.messages)
    expect(next.runtimeEvents).toEqual([])
  })
})

describe('runtime event display queue', () => {
  test('paces text deltas and leaves terminal events behind them', () => {
    const queue = [
      notification(1, { type: 'run.started' }),
      notification(2, { type: 'message.started' }),
      notification(3, { type: 'message.delta', delta: 'first' }),
      notification(4, { type: 'message.delta', delta: 'second' }),
      notification(5, { type: 'message.completed' }),
      notification(6, { type: 'run.completed' }),
    ]

    expect(takeRuntimeEventFrame(queue).map((item) => item.seq)).toEqual([1, 2, 3])
    expect(takeRuntimeEventFrame(queue).map((item) => item.seq)).toEqual([4])
    expect(takeRuntimeEventFrame(queue).map((item) => item.seq)).toEqual([5, 6])
    expect(queue).toEqual([])
  })

  test('sorts a burst before pacing it', () => {
    const queue = [
      notification(3, { type: 'message.delta', delta: 'answer' }),
      notification(1, { type: 'run.started' }),
      notification(2, { type: 'reasoning.delta', delta: 'thinking' }),
    ]

    expect(takeRuntimeEventFrame(queue).map((item) => item.seq)).toEqual([1, 2])
    expect(takeRuntimeEventFrame(queue).map((item) => item.seq)).toEqual([3])
  })

  test('coalesces and drains a large ordered delta burst without delaying completion', () => {
    const queue = [
      notification(1, { type: 'run.started' }),
      notification(2, { type: 'message.started' }),
      ...Array.from({ length: 200 }, (_, index) => (
        notification(index + 3, { type: 'message.delta', delta: `${index},` })
      )),
      notification(203, { type: 'message.completed' }),
      notification(204, { type: 'run.completed' }),
    ]

    const frame = takeRuntimeEventFrame(queue)
    expect(frame.map((item) => item.event.type)).toEqual([
      'run.started', 'message.started', 'message.delta', 'message.completed', 'run.completed',
    ])
    expect(frame[2].event.delta).toBe(Array.from({ length: 200 }, (_, index) => `${index},`).join(''))
    expect(frame[2].seq).toBe(202)
    expect(queue).toEqual([])
  })

  test('keeps the cheap one-delta cadence for a small live queue', () => {
    const queue = [
      notification(1, { type: 'message.delta', delta: 'a' }),
      notification(2, { type: 'message.delta', delta: 'b' }),
    ]

    expect(takeRuntimeEventFrame(queue).map((item) => item.seq)).toEqual([1])
    expect(queue.map((item) => item.seq)).toEqual([2])
  })

  test('coalesces adjacent deltas while animation frames are paused', () => {
    const queue: RuntimeEventNotification[] = []
    enqueueRuntimeEvent(queue, notification(1, { type: 'message.delta', delta: 'a' }))
    enqueueRuntimeEvent(queue, notification(2, { type: 'message.delta', delta: 'b' }))
    enqueueRuntimeEvent(queue, notification(3, { type: 'reasoning.delta', delta: 'c' }))

    expect(queue).toHaveLength(2)
    expect(queue[0]).toMatchObject({ seq: 2, event: { type: 'message.delta', delta: 'ab' } })
    expect(queue[1]).toMatchObject({ seq: 3, event: { type: 'reasoning.delta', delta: 'c' } })
  })
})

describe('pending runtime interactions', () => {
  test('restores the newest unanswered question even when it is not the last run', () => {
    const current = detail()
    current.runtimeEvents = [{
      runId: 'run-1', seq: 2, eventType: 'user.question.requested', createdAt: 2000,
      event: {
        type: 'user.question.requested',
        questions: [{ questionId: 'scene', question: '选择应用场景', multiSelect: true, allowOther: false }],
      },
    }]
    current.lastRun = { ...current.lastRun!, id: 'run-2', status: 'completed', lastSeq: 5 }

    expect(pendingRuntimeQuestion(current)).toEqual({
      runId: 'run-1',
      questions: [{
        questionId: 'scene', question: '选择应用场景', options: [], multiSelect: true, allowOther: false,
      }],
    })
  })

  test('closes a question after its response lifecycle event is persisted', () => {
    const current = detail()
    current.runtimeEvents = [
      {
        runId: 'run-1', seq: 2, eventType: 'user.question.requested', createdAt: 2000,
        event: { type: 'user.question.requested', questions: [{ question_id: 'scene', question: '选择应用场景' }] },
      },
      {
        runId: 'run-1', seq: 3, eventType: 'user.question.responded', createdAt: 3000,
        event: { type: 'user.question.responded', childRunId: 'run-2' },
      },
    ]

    expect(pendingRuntimeQuestion(current)).toBeNull()
  })

  test('does not reopen a question answered before response lifecycle events existed', () => {
    const current = detail()
    current.runtimeEvents = [{
      runId: 'run-1', seq: 2, eventType: 'user.question.requested', createdAt: 2000,
      event: { type: 'user.question.requested', questions: [{ question_id: 'scene', question: '选择应用场景' }] },
    }]
    current.messages.push({
      id: 'answer-1', conversationId: 'conversation-1', runId: 'run-2', role: 'user', kind: 'text',
      content: '选择应用场景：轨道吊', status: 'completed', ordinal: 2, createdAt: 3000, updatedAt: 3000,
    })

    expect(pendingRuntimeQuestion(current)).toBeNull()
  })

  test('keeps a question open when a later user message is unrelated', () => {
    const current = detail()
    current.runtimeEvents = [{
      runId: 'run-1', seq: 2, eventType: 'user.question.requested', createdAt: 2000,
      event: { type: 'user.question.requested', questions: [{ question_id: 'scene', question: '选择应用场景' }] },
    }]
    current.messages.push({
      id: 'unrelated-1', conversationId: 'conversation-1', runId: 'run-2', role: 'user', kind: 'text',
      content: '先做另一件事', status: 'completed', ordinal: 2, createdAt: 3000, updatedAt: 3000,
    })

    expect(pendingRuntimeQuestion(current)?.runId).toBe('run-1')
  })
})
