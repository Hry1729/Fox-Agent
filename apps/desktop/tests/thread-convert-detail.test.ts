import { describe, expect, test } from 'bun:test'
import { createFoxThreadCache, deriveFoxThreadMessages, type FoxThreadMessage } from '../src/features/chat/thread/convert-detail'
import type { AttachmentRecord, ConversationDetail, ConversationMessage, RunEventRecord, RunRecord } from '../src/features/conversations/model/types'

function message(patch: Partial<ConversationMessage> & Pick<ConversationMessage, 'id' | 'role'>): ConversationMessage {
  const now = 1_700_000_000_000
  return {
    conversationId: 'c1',
    runId: null,
    kind: 'text',
    content: '',
    status: 'completed',
    ordinal: 0,
    createdAt: now,
    updatedAt: now,
    ...patch,
  }
}

function run(patch: Partial<RunRecord> & Pick<RunRecord, 'id' | 'status'>): RunRecord {
  return {
    conversationId: 'c1',
    runtimeSessionId: null,
    model: 'test-model',
    startedAt: 1_700_000_000_000,
    finishedAt: 1_700_000_010_000,
    errorCode: null,
    errorMessage: null,
    lastSeq: 0,
    ...patch,
  }
}

function event(seq: number, eventType: string, eventPayload: Record<string, unknown>): RunEventRecord {
  return {
    runId: 'r1',
    seq,
    eventType,
    event: { type: eventType, ...eventPayload },
    createdAt: 1_700_000_000_000 + seq,
  }
}

function detail(patch: Partial<ConversationDetail> = {}): ConversationDetail {
  return {
    conversation: {
      id: 'c1', agentId: 'fox-general', agentName: 'Fox', title: '对话',
      projectId: null, projectRoot: null, permissionMode: 'ask', status: 'active',
      createdAt: 0, updatedAt: 0, lastMessageAt: null,
    },
    messages: [],
    runtimeEvents: [],
    toolCalls: [],
    approvals: [],
    attachments: [],
    artifacts: [],
    knowledgeBindings: [],
    expertBindings: [],
    lastRun: null,
    hasEarlierMessages: false,
    goals: [],
    tasks: [],
    evidence: [],
    planRevisions: [],
    reviewFindings: [],
    acceptances: [],
    childRuns: [],
    expertWorkflow: null,
    expertTeam: null,
    ...patch,
  }
}

function textOf(fox: FoxThreadMessage) {
  return fox.content.filter((part) => part.type === 'text').map((part) => (part as { type: 'text'; text: string }).text).join('')
}

function processPart(fox: FoxThreadMessage) {
  return fox.content.find((part) => part.type === 'data-fox-process') as { type: 'data-fox-process'; id: string; data: { reasoning: string; reasoningItems: string[]; tools: Array<{ id: string; name: string; isError: boolean; completed: boolean }>; steps: unknown[]; sources: unknown[] } } | undefined
}

describe('deriveFoxThreadMessages', () => {
  test('普通问答：user + assistant 文本各成一条，id 保持', () => {
    const foxes = deriveFoxThreadMessages({
      detail: detail({
        messages: [
          message({ id: 'm1', role: 'user', content: '你好', ordinal: 1 }),
          message({ id: 'm2', role: 'assistant', content: '你好！有什么可以帮你？', runId: 'r1', ordinal: 2 }),
        ],
        lastRun: run({ id: 'r1', status: 'completed' }),
        runs: [run({ id: 'r1', status: 'completed' })],
      }),
    })
    expect(foxes.map((item) => item.id)).toEqual(['m1', 'm2'])
    expect(foxes[0].role).toBe('user')
    expect(textOf(foxes[0])).toBe('你好')
    expect(textOf(foxes[1])).toBe('你好！有什么可以帮你？')
    expect(foxes[1].failure).toBeNull()
    expect(foxes[1].cancelled).toBe(false)
    expect(foxes[1].isActiveRun).toBe(false)
  })

  test('带思考：<think> 内容并入 data-fox-process，正文不再包含思考', () => {
    const foxes = deriveFoxThreadMessages({
      detail: detail({
        messages: [
          message({ id: 'm1', role: 'user', content: '两步推理', ordinal: 1 }),
          message({ id: 'm2', role: 'assistant', content: '<think>先想一下\n再验证</think>答案是 42。', runId: 'r1', ordinal: 2 }),
        ],
        lastRun: run({ id: 'r1', status: 'completed' }),
        runs: [run({ id: 'r1', status: 'completed' })],
      }),
    })
    const process = processPart(foxes[1])
    expect(process).toBeTruthy()
    expect(process!.data.reasoning).toContain('先想一下')
    expect(process!.data.reasoning).toContain('再验证')
    expect(textOf(foxes[1])).toBe('答案是 42。')
    // 思考排在正文之前
    expect(foxes[1].content[0].type).toBe('data-fox-process')
  })

  test('带工具调用：tool.* 事件进入 data-fox-process 的工具步骤', () => {
    const foxes = deriveFoxThreadMessages({
      detail: detail({
        messages: [
          message({ id: 'm1', role: 'user', content: '读一下文件', ordinal: 1 }),
          message({ id: 'm2', role: 'assistant', content: '读取完成。', runId: 'r1', ordinal: 2 }),
        ],
        runtimeEvents: [
          event(1, 'tool.started', { toolCallId: 't1', tool: 'read_file', input: { path: 'src/a.ts' } }),
          event(2, 'tool.completed', { toolCallId: 't1', tool: 'read_file', result: { ok: true }, isError: false }),
        ],
        lastRun: run({ id: 'r1', status: 'completed', lastSeq: 2 }),
        runs: [run({ id: 'r1', status: 'completed' })],
      }),
    })
    const process = processPart(foxes[1])
    expect(process).toBeTruthy()
    expect(process!.data.tools).toHaveLength(1)
    expect(process!.data.tools[0].name).toBe('read_file')
    expect(process!.data.tools[0].completed).toBe(true)
    expect(textOf(foxes[1])).toBe('读取完成。')
  })

  test('带附件：AttachmentRecord 按 messageId 关联为消息附件', () => {
    const attachment: AttachmentRecord = {
      id: 'a1', conversationId: 'c1', messageId: 'm1', displayName: '报告.pdf',
      storagePath: 'host:/x', mediaType: 'application/pdf', byteSize: 100, sha256: null,
      status: 'saved', createdAt: 0,
    }
    const foxes = deriveFoxThreadMessages({
      detail: detail({
        messages: [message({ id: 'm1', role: 'user', content: '请看附件', ordinal: 1 })],
        attachments: [attachment],
      }),
    })
    expect(foxes[0].attachments).toHaveLength(1)
    expect(foxes[0].attachments[0]).toMatchObject({ id: 'a1', type: 'document', name: '报告.pdf', status: { type: 'complete' } })
    expect(foxes[0].attachments[0].content[0]).toEqual({
      type: 'file', filename: '报告.pdf', mimeType: 'application/pdf', data: 'host:/x', sourceType: 'id',
    })
  })

  test('失败：运行失败的消息带 incomplete 语义的 data-fox-error', () => {
    const foxes = deriveFoxThreadMessages({
      detail: detail({
        messages: [
          message({ id: 'm1', role: 'user', content: '做点什么', ordinal: 1 }),
          message({ id: 'm2', role: 'assistant', content: '部分回答', runId: 'r1', ordinal: 2 }),
        ],
        lastRun: run({ id: 'r1', status: 'failed', errorCode: 'provider.timeout', errorMessage: '连接超时' }),
        runs: [run({ id: 'r1', status: 'failed', errorCode: 'provider.timeout', errorMessage: '连接超时' })],
      }),
    })
    expect(foxes[1].failure).toEqual({ kind: 'failed', code: 'provider.timeout', message: '连接超时' })
    expect(foxes[1].cancelled).toBe(false)
    const errorPart = foxes[1].content.find((part) => part.type === 'data-fox-error')
    expect(errorPart).toBeTruthy()
  })

  test('取消：运行取消的消息没有错误部件', () => {
    const foxes = deriveFoxThreadMessages({
      detail: detail({
        messages: [
          message({ id: 'm1', role: 'user', content: '停一下', ordinal: 1 }),
          message({ id: 'm2', role: 'assistant', content: '写到一半', runId: 'r1', ordinal: 2 }),
        ],
        runtimeEvents: [event(3, 'run.cancelled', {})],
        lastRun: run({ id: 'r1', status: 'cancelled' }),
        runs: [run({ id: 'r1', status: 'cancelled' })],
      }),
    })
    expect(foxes[1].cancelled).toBe(true)
    expect(foxes[1].failure).toBeNull()
    expect(foxes[1].content.some((part) => part.type === 'data-fox-error')).toBe(false)
    expect(textOf(foxes[1])).toBe('写到一半')
  })

  test('流式中：运行中的 run 生成合成 assistant 消息，且只有它随流式变化', () => {
    const base = detail({
      messages: [message({ id: 'm1', role: 'user', content: '写一首诗', ordinal: 1 })],
      lastRun: run({ id: 'r1', status: 'running', finishedAt: null }),
      runs: [run({ id: 'r1', status: 'running', finishedAt: null })],
    })
    const first = deriveFoxThreadMessages({ detail: base, streamingText: '春风' })
    expect(first).toHaveLength(2)
    const streaming = first[1]
    expect(streaming.id).toBe('assistant-r1')
    expect(streaming.role).toBe('assistant')
    expect(streaming.isActiveRun).toBe(true)
    expect(textOf(streaming)).toBe('春风')

    // 同一帧重算（同一缓存）：对象引用稳定，revision 不变
    const stableCache = createFoxThreadCache()
    const firstStable = deriveFoxThreadMessages({ detail: base, streamingText: '春风' }, stableCache)
    const again = deriveFoxThreadMessages({ detail: base, streamingText: '春风' }, stableCache)
    expect(again[1]).toBe(firstStable[1])
    expect(again[0]).toBe(firstStable[0])

    // 流式推进：只有合成消息对象变化，历史消息引用保持不变
    const cache = createFoxThreadCache()
    const before = deriveFoxThreadMessages({ detail: base, streamingText: '春风' }, cache)
    const after = deriveFoxThreadMessages({ detail: base, streamingText: '春风又绿江南岸' }, cache)
    expect(after[0]).toBe(before[0])
    expect(after[1]).not.toBe(before[1])
    expect(textOf(after[1])).toBe('春风又绿江南岸')
  })

  test('加载更早消息后：原有消息 id 与对象引用保持不变，更早消息插入在前', () => {
    const cache = createFoxThreadCache()
    const base = detail({
      messages: [
        message({ id: 'm1', role: 'user', content: '第一问', ordinal: 1 }),
        message({ id: 'm2', role: 'assistant', content: '第一答', runId: 'r1', ordinal: 2 }),
      ],
      lastRun: run({ id: 'r1', status: 'completed' }),
      runs: [run({ id: 'r1', status: 'completed' })],
      hasEarlierMessages: true,
    })
    const first = deriveFoxThreadMessages({ detail: base }, cache)
    expect(first.map((item) => item.id)).toEqual(['m1', 'm2'])

    const extended: ConversationDetail = {
      ...base,
      messages: [
        message({ id: 'm0', role: 'user', content: '第零问', ordinal: 0 }),
        ...base.messages,
      ],
      hasEarlierMessages: false,
    }
    const second = deriveFoxThreadMessages({ detail: extended }, cache)
    expect(second.map((item) => item.id)).toEqual(['m0', 'm1', 'm2'])
    // 旧消息引用与 id 都保持稳定
    expect(second[1]).toBe(first[0])
    expect(second[2]).toBe(first[1])
  })

  test('乐观消息：真实消息落地后不再重复显示', () => {
    const now = 1_700_000_000_000
    const pending = message({ id: 'pending-1', role: 'user', content: '你好', createdAt: now, updatedAt: now, status: 'sending' })
    const withPending = deriveFoxThreadMessages({
      detail: detail({ messages: [] }),
      pendingMessage: pending,
    })
    expect(withPending).toHaveLength(1)
    expect(withPending[0].optimistic).toBe(true)

    const stored = deriveFoxThreadMessages({
      detail: detail({
        messages: [message({ id: 'm1', role: 'user', content: '你好', createdAt: now + 100, updatedAt: now + 100 })],
      }),
      pendingMessage: pending,
    })
    // 真实消息已经出现，乐观消息被去重
    expect(stored).toHaveLength(1)
    expect(stored[0].id).toBe('m1')
  })

  test('tool/system 角色不单独成消息', () => {
    const foxes = deriveFoxThreadMessages({
      detail: detail({
        messages: [
          message({ id: 'm1', role: 'user', content: 'hi', ordinal: 1 }),
          message({ id: 'm2', role: 'tool', content: 'tool output', ordinal: 2 }),
          message({ id: 'm3', role: 'system', content: 'system note', ordinal: 3 }),
          message({ id: 'm4', role: 'assistant', content: 'ok', ordinal: 4 }),
        ],
      }),
    })
    expect(foxes.map((item) => item.id)).toEqual(['m1', 'm4'])
  })
})
