/**
 * Synthetic, in-memory conversation fixtures for the profile-build harness.
 * All content is generated — no user database, files or messages are touched.
 *
 * Size semantics: `messageCount` is the TOTAL number of chat messages
 * (user + assistant). Turns = floor(messageCount / 2).
 *
 * Each kind produces its real render path:
 * - markdown: headings, lists, tables, inline code (streamdown).
 * - code:      fenced ```typescript blocks (lazy Shiki highlighter).
 * - tools:     tool.started / tool.completed lifecycle runtime events.
 * - mixed:     a realistic blend of all three.
 */
import type {
  ArtifactRecord,
  AttachmentRecord,
  ConversationDetail,
  ConversationMessage,
  ConversationSummary,
  RunEventRecord,
  RunRecord,
  RuntimeEventNotification,
} from '@/features/conversations/model/types'

export type FixtureKind = 'markdown' | 'code' | 'tools' | 'mixed'

export const FIXTURE_CONVERSATION_ID = 'fixture-conversation'
export const FIXTURE_RUN_ID = 'fixture-run'
export const FIXTURE_STREAM_RUN_ID = 'fixture-stream-run'

function baseDetail(messages: ConversationMessage[], runtimeEvents: RunEventRecord[]): ConversationDetail {
  const summary: ConversationSummary = {
    id: FIXTURE_CONVERSATION_ID,
    agentId: 'fixture-agent',
    agentName: 'Fox 默认助手',
    title: '性能夹具（合成数据）',
    projectId: null,
    projectRoot: null,
    status: 'active',
    createdAt: 0,
    updatedAt: 0,
    lastMessageAt: 0,
  }
  const run: RunRecord = {
    id: FIXTURE_RUN_ID,
    conversationId: FIXTURE_CONVERSATION_ID,
    runtimeSessionId: null,
    status: 'completed',
    model: 'fixture-model',
    startedAt: 0,
    finishedAt: 0,
    errorCode: null,
    errorMessage: null,
    lastSeq: runtimeEvents.reduce((max, event) => Math.max(max, event.seq), 0),
  }
  const empty = <T,>(): T[] => [] as T[]
  return {
    conversation: summary,
    messages,
    runtimeEvents,
    toolCalls: [],
    approvals: [],
    attachments: empty<AttachmentRecord>(),
    artifacts: empty<ArtifactRecord>(),
    knowledgeBindings: [],
    expertBindings: [],
    lastRun: run,
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
  }
}

function markdownAnswer(index: number): string {
  return [
    `## 结论 ${index}`,
    '',
    `这是第 **${index}** 条合成回复，用于测量 Markdown 渲染。`,
    '',
    '- 第一项：描述背景与目标。',
    '- 第二项：列出关键步骤。',
    '- 第三项：给出可验证的结果。',
    '',
    '| 指标 | 数值 | 说明 |',
    '| --- | ---: | --- |',
    `| 延迟 | ${index * 3}ms | 合成数据 |`,
    '| 状态 | 正常 | 无回归 |',
    '',
    '更多内容见 [文档](https://example.invalid/docs) 与 `行内代码` 片段。',
    '',
  ].join('\n')
}

const CODE_SNIPPET = [
  '```typescript',
  'export async function summarize(items: number[]): Promise<number> {',
  '  const total = items.reduce((sum, value) => sum + value, 0)',
  '  return items.length ? total / items.length : 0',
  '}',
  '',
  'const values = [1, 2, 3, 4, 5]',
  'console.log(await summarize(values))',
  '```',
].join('\n')

function codeAnswer(index: number): string {
  return [
    `## 代码方案 ${index}`,
    '',
    '下面是可运行的实现，附带类型标注：',
    '',
    CODE_SNIPPET,
    '',
    '关键点：使用 `reduce` 聚合，并在空数组时安全返回。',
    '',
  ].join('\n')
}

function toolsAnswer(index: number): string {
  return [
    `## 工具执行 ${index}`,
    '',
    '已读取文件并运行测试，结果如下：',
    '',
    `- 读取 \`src/mod-${index}.ts\``,
    '- 执行 `pnpm test`',
    '- 全部通过。',
    '',
  ].join('\n')
}

function answerFor(kind: FixtureKind, index: number): string {
  switch (kind) {
    case 'markdown':
      return markdownAnswer(index)
    case 'code':
      return codeAnswer(index)
    case 'tools':
      return toolsAnswer(index)
    case 'mixed':
      if (index % 3 === 0) return codeAnswer(index)
      if (index % 3 === 1) return markdownAnswer(index)
      return toolsAnswer(index)
  }
}

/** Whether a given turn carries tool lifecycle events. */
function turnHasTools(kind: FixtureKind, turn: number): boolean {
  if (kind === 'tools') return true
  if (kind === 'mixed') return turn % 3 === 2
  return false
}

function pushToolEvents(events: RunEventRecord[], runId: string, seq: { value: number }, turn: number): void {
  const pairs: Array<[string, string, Record<string, unknown>]> = [
    ['tool.started', 'read', { toolCallId: `tc-${turn}-read`, tool: 'read', input: { path: `src/mod-${turn}.ts` } }],
    ['tool.completed', 'read', { toolCallId: `tc-${turn}-read`, tool: 'read', output: `文件 ${turn} 内容（合成）` }],
    ['tool.started', 'shell', { toolCallId: `tc-${turn}-test`, tool: 'shell', input: { command: 'pnpm test' } }],
    ['tool.completed', 'shell', { toolCallId: `tc-${turn}-test`, tool: 'shell', output: `测试通过 ${turn}` }],
  ]
  for (const [eventType, , event] of pairs) {
    seq.value += 1
    events.push({ runId, seq: seq.value, eventType, event: { type: eventType, ...event }, createdAt: 0 })
  }
}

/**
 * Builds a completed history. `messageCount` is the TOTAL message count
 * (user + assistant); the actual assistant turn count is floor(messageCount/2).
 */
export function buildHistoryDetail(kind: FixtureKind, messageCount: number): ConversationDetail {
  const messages: ConversationMessage[] = []
  const events: RunEventRecord[] = []
  const turns = Math.max(1, Math.floor(messageCount / 2))
  const seq = { value: 0 }
  let ordinal = 0

  for (let turn = 1; turn <= turns; turn += 1) {
    const runId = `${FIXTURE_RUN_ID}-${turn}`
    messages.push({
      id: `user-${turn}`,
      conversationId: FIXTURE_CONVERSATION_ID,
      runId,
      role: 'user',
      kind: 'text',
      content: `请处理第 ${turn} 个任务（${kind}）。`,
      status: 'completed',
      ordinal: (ordinal += 1),
      createdAt: 0,
      updatedAt: 0,
    })
    if (turnHasTools(kind, turn)) pushToolEvents(events, runId, seq, turn)
    messages.push({
      id: `assistant-${turn}`,
      conversationId: FIXTURE_CONVERSATION_ID,
      runId,
      role: 'assistant',
      kind: 'text',
      content: answerFor(kind, turn),
      status: 'completed',
      ordinal: (ordinal += 1),
      createdAt: 0,
      updatedAt: 0,
    })
  }

  return baseDetail(messages, events)
}

export interface StreamScenario {
  notifications: RuntimeEventNotification[]
  runId: string
  conversationId: string
  finalText: string
  eventCount: number
}

function notification(seq: number, runId: string, event: Record<string, unknown>): RuntimeEventNotification {
  return {
    conversationId: FIXTURE_CONVERSATION_ID,
    runtimeSessionId: null,
    runId,
    seq,
    timestamp: new Date(0).toISOString(),
    event: event as RuntimeEventNotification['event'],
  }
}

/** Builds a full lifecycle stream: run.started → message.started → [tools] → deltas → message.completed → run.completed. */
export function buildStreamScenario(kind: FixtureKind, deltaChunks = 200): StreamScenario {
  const runId = FIXTURE_STREAM_RUN_ID
  const notifications: RuntimeEventNotification[] = []
  let seq = 0

  notifications.push(notification(++seq, runId, { type: 'run.started', model: 'fixture-model' }))
  notifications.push(notification(++seq, runId, { type: 'message.started', messageId: `assistant-${runId}` }))

  if (kind === 'tools' || kind === 'code' || kind === 'mixed') {
    notifications.push(notification(++seq, runId, { type: 'tool.started', toolCallId: 'tc-stream-read', tool: 'read', input: { path: 'src/index.ts' } }))
    notifications.push(notification(++seq, runId, { type: 'tool.completed', toolCallId: 'tc-stream-read', tool: 'read', output: '读取成功（合成）' }))
  }
  if (kind === 'tools' || kind === 'mixed') {
    notifications.push(notification(++seq, runId, { type: 'tool.started', toolCallId: 'tc-stream-test', tool: 'shell', input: { command: 'pnpm test' } }))
    notifications.push(notification(++seq, runId, { type: 'tool.completed', toolCallId: 'tc-stream-test', tool: 'shell', output: '测试通过（合成）' }))
  }

  const base = kind === 'code' ? codeAnswer(1) : kind === 'tools' ? toolsAnswer(1) : markdownAnswer(1)
  const finalText = kind === 'markdown' ? base.repeat(6) : base.repeat(3)
  const chunkSize = Math.max(1, Math.ceil(finalText.length / deltaChunks))
  for (let start = 0; start < finalText.length; start += chunkSize) {
    notifications.push(notification(++seq, runId, { type: 'message.delta', delta: finalText.slice(start, start + chunkSize) }))
  }

  notifications.push(notification(++seq, runId, { type: 'message.completed', messageId: `assistant-${runId}` }))
  notifications.push(notification(++seq, runId, { type: 'run.completed', model: 'fixture-model' }))

  return { notifications, runId, conversationId: FIXTURE_CONVERSATION_ID, finalText, eventCount: notifications.length }
}

/** Initial state for a stream replay: a single user message, run not started. */
export function buildStreamStartDetail(): ConversationDetail {
  const messages: ConversationMessage[] = [
    {
      id: 'user-stream',
      conversationId: FIXTURE_CONVERSATION_ID,
      runId: FIXTURE_STREAM_RUN_ID,
      role: 'user',
      kind: 'text',
      content: '请生成一个较长的回复用于流式测量。',
      status: 'completed',
      ordinal: 1,
      createdAt: 0,
      updatedAt: 0,
    },
  ]
  const detail = baseDetail(messages, [])
  return { ...detail, lastRun: null }
}
