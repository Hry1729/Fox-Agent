import { describe, expect, test } from 'bun:test'
import { AGENT_DISABLED_CODE, AGENT_DISABLED_NOTICE, enabledOrFox, isAgentEnabled } from '../src/features/conversations/model/external-agent-enablement'
import { readFileSync } from 'node:fs'
import { resolveConversationAgentId } from '../src/features/conversations/model/agent-initialization'
import { knowledgeReferenceKey, knowledgeReferenceFromLegacyBinding } from '../src/features/conversations/api/desktop-client'
import { conversationRunFingerprint, conversationRunIsActive, conversationRunState, snapshotForRun } from '../src/features/conversations/model/kernel-snapshot'
import { mergeConversationDetail } from '../src/features/conversations/model/runtime-event-reducer'
import {
  EMPTY_CONVERSATION_RUN_OVERLAY,
  applyApprovalToOverlay,
  applyRunEventToOverlay,
  conversationRunIndicators,
  pruneRunOverlay,
} from '../src/features/chat/conversation-run-indicator'
import {
  AGENT_BACKEND_REFUSED,
  agentBackendInputLockedCode,
  agentBackendInputLockedReason,
  conversationAgentBackend,
  isAgentBackendRefusalCode,
  withoutRefusedSend,
  AGENT_BACKEND_PREFERENCES_EVENT,
  newConversationBackend,
  isAgentBackendHidden,
} from '../src/features/conversations/model/agent-backend'

// C07 引用回复的发送链路：真实的 useDesktopConversation 主体在沙箱里运行
// （与 agent-backend-send.test.ts 同一套确定性 state 槽与假 Host，不跑 effect），
// 断言引用身份进入 startRun 请求、Host 回写的引用立刻出现在已发消息上、
// 分页合并不会丢引用与标记，以及被拒绝的发送不消费引用草稿。
const source = readFileSync(new URL('../src/features/conversations/hooks/use-desktop-conversation.ts', import.meta.url), 'utf8')
const body = new Bun.Transpiler({ loader: 'ts' }).transformSync(source.slice(source.indexOf('export function useDesktopConversation'), source.indexOf('export function latestMessage')).replace('export function', 'function'))

function harness(startRun: (request: any) => Promise<any>, options: {
  rewindRun?: (request: any) => Promise<any>
  loadConversation?: (id: string) => Promise<any>
  mergeDetail?: typeof mergeConversationDetail
  /** UI 1103: external Agents enabled in 设置 → Agent (default: all three). */
  enabledAgents?: readonly string[]
  /** The Host reports every external backend as runnable (no administrator switch-off). */
  hostRunnable?: boolean
  /** C07: the older history page the Host returns for loadEarlierMessages. */
  historyPage?: (id: string, beforeOrdinal: number) => Promise<any>
} = {}) {
  const enabledAgents = options.enabledAgents ?? ['pi', 'codex', 'deepseek']
  const slots: any[] = []
  let cursor = 0
  const slot = (initial: any) => {
    const index = cursor++
    if (!(index in slots)) slots[index] = initial
    return index
  }
  const startRequests: any[] = []
  const createdRequests: any[] = []
  const conversations = new Map<string, any>()
  const trashed = new Set<string>()
  const purged: string[] = []
  const env = {
    useState: (initial: any) => { const i = slot(initial); return [slots[i], (value: any) => { slots[i] = typeof value === 'function' ? value(slots[i]) : value }] },
    useRef: (initial: any) => slots[slot({ current: initial })],
    useCallback: (callback: any) => callback,
    useMemo: (callback: any) => callback(),
    useEffect: () => {},
    useLayoutEffect: () => {},
    useRuntimeEventStream: () => {},
    useKernelStateStream: () => {},
    desktopRuntimeAvailable: false,
    desktopClient: {
      initialize: async () => ({ defaultAgentId: 'fox-general' }),
      createConversation: async (request: any) => {
        createdRequests.push(request)
        const conversation = { ...request, id: `conversation-new-${createdRequests.length}`, agentBackend: request.agentBackend ?? 'fox' }
        conversations.set(conversation.id, { conversation, messages: [], attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null })
        return conversation
      },
      // The Host wrote nothing for a refused send: the persisted detail is unchanged.
      loadConversation: async (id: string) => options.loadConversation ? options.loadConversation(id) : conversations.get(id),
      loadConversationHistory: async (id: string, beforeOrdinal: number) => options.historyPage
        ? options.historyPage(id, beforeOrdinal)
        : { messages: [], runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], hasEarlierMessages: false },
      listConversations: async () => [...conversations.values()].map((item) => item.conversation),
      listArchivedConversations: async () => [],
      listTrashedConversations: async () => [],
      runtimeStatus: async () => ({}),
      startRun: (request: any) => { startRequests.push(request); return startRun(request) },
      rewindRun: options.rewindRun,
      setKnowledgeReferences: async () => ({ bindings: [], references: [] }),
      setKnowledgeBindings: async () => [],
      saveAttachments: async () => { throw Object.assign(new Error('磁盘只读'), { code: 'attachment.write_failed' }) },
      deleteConversation: async (id: string) => { trashed.add(id); return true },
      purgeConversation: async (id: string) => {
        if (!trashed.has(id)) throw Object.assign(new Error('请先将对话移入回收站，再执行永久删除'), { code: 'conversation.purge_requires_trash' })
        conversations.delete(id)
        purged.push(id)
        return true
      },
    },
    optimisticRun: () => ({ id: 'pending-run-1', status: 'queued' }),
    optimisticId: () => 'pending-message-1',
    flushSync: (fn: () => void) => fn(),
    extractedAttachmentContext: async () => '',
    runIsActive: conversationRunIsActive,
    conversationRunFingerprint,
    conversationRunState,
    snapshotForRun,
    resolveConversationAgentId,
    withWorkspaceInitializationTimeout: (promise: Promise<unknown>) => promise,
    activeConversationExpertBinding: (detail: any) => detail?.expertBindings?.find((item: any) => item.state === 'active'),
    knowledgeReferenceKey,
    knowledgeReferenceFromLegacyBinding,
    desktopErrorDetails: (error: any) => ({ code: error.code ?? 'unknown', message: error.message, retryable: false }),
    mergeConversationDetail: options.mergeDetail ?? ((persisted: any) => persisted),
    EMPTY_CONVERSATION_RUN_OVERLAY,
    conversationRunIndicators,
    useConversationCompletionNotices: (items: any, overlay: any, snapshotAt: number) => ({
      runIndicators: conversationRunIndicators(items, overlay, snapshotAt),
      acknowledgeCompletion: () => {},
    }),
    pruneRunOverlay,
    applyRunEventToOverlay,
    applyApprovalToOverlay,
    AGENT_BACKEND_REFUSED,
    agentBackendInputLockedCode,
    agentBackendInputLockedReason: options.hostRunnable ? () => null : agentBackendInputLockedReason,
    conversationAgentBackend,
    isAgentBackendRefusalCode,
    withoutRefusedSend,
    AGENT_BACKEND_PREFERENCES_EVENT,
    // UI 1103: these routing tests run with every external Agent enabled in
    // 设置 → Agent (the behaviour they pin down); the opt-in itself is
    // covered by its own tests.
    newConversationBackend: (preferences: any, enabled: readonly string[] = enabledAgents) => newConversationBackend(preferences, enabled),
    isAgentBackendHidden,
    isAgentEnabled: (backend: string, enabled: readonly string[] = enabledAgents) => isAgentEnabled(backend, enabled),
    enabledOrFox: (backend: any, enabled: readonly string[] = enabledAgents) => enabledOrFox(backend, enabled),
    useEnabledExternalAgents: () => enabledAgents,
    AGENT_DISABLED_CODE,
    AGENT_DISABLED_NOTICE,
  }
  const hook = new Function(...Object.keys(env), `${body}; return useDesktopConversation`)(...Object.values(env))
  return {
    render: () => { cursor = 0; return hook() },
    startRequests,
    createdRequests,
    purged: () => purged.slice(),
    addConversation: (detail: any) => conversations.set(detail.conversation.id, detail),
  }
}

const refusal = (code: string, message: string) => Object.assign(new Error(message), { code })

const quote = (patch: any = {}) => ({
  messageId: 'm-2', bubbleId: 'run:response:2', author: 'companion',
  summary: '华东机电的报价单没写税率，我先按 13% 估算。',
  excerpt: '华东机电的报价单没写税率，我先按 13% 估算。',
  deliveredAt: 1700000002000, sourceOrdinal: 2, available: true, ...patch,
})

const started = (request: any, patch: any = {}) => ({
  run: { id: 'run-2', conversationId: request.conversationId, runtimeSessionId: null, model: 'test', status: 'running',
    lastSeq: 0, errorCode: null, errorMessage: null, startedAt: 1700000003000, finishedAt: null },
  userMessage: { id: 'm-2', conversationId: request.conversationId, runId: 'run-2', role: 'user', kind: 'text',
    content: request.text, status: 'completed', ordinal: 2, createdAt: 1700000003000, updatedAt: 1700000003000 },
  attachments: [], ...patch,
})

const chat = (patch: any = {}) => ({
  conversation: { id: 'chat', agentId: 'fox-general', permissionMode: 'ask' },
  companionId: 'contact',
  messages: [{ id: 'm-1', conversationId: 'chat', runId: 'run-old', role: 'user', kind: 'text', content: '旧消息',
    status: 'completed', ordinal: 1, createdAt: 1700000000000, updatedAt: 1700000000000 }],
  runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [],
  knowledgeBindings: [], expertBindings: [],
  goals: [], tasks: [], evidence: [], planRevisions: [], reviewFindings: [], acceptances: [], childRuns: [],
  lastRun: { id: 'run-old', status: 'completed', lastSeq: 3 },
  bubbles: [{ id: 'run:response:2', conversationId: 'chat', runId: 'run-old', messageId: 'a-1', sourceOrdinal: 2,
    author: 'companion', projection: 'segmented', segmentStart: 2, segmentEnd: 2, deliveryOrder: 2,
    deliveredAt: 1700000002000, content: '华东机电的报价单没写税率，我先按 13% 估算。', fingerprint: 'f' }],
  bubbleMarks: [], messageQuotes: [], hasEarlierMessages: false, ...patch,
})

describe('C07 引用随消息一起落库', () => {
  test('引用身份进入 startRun，Host 回写的引用立刻挂在已发消息上', async () => {
    const stored = quote()
    const h = harness(async request => started(request, { quote: stored }))
    h.addConversation(chat())
    await h.render().openConversation('chat')
    const outcome = await h.render().send('按这个税率重算', undefined, [], undefined, 'run:response:2')
    expect(outcome).toBe(true)
    expect(h.startRequests).toHaveLength(1)
    expect(h.startRequests[0].quoteBubbleId).toBe('run:response:2')
    expect(h.startRequests[0].text).toBe('按这个税率重算')
    const state = h.render()
    expect(state.detail.messages.map((message: any) => message.id)).toEqual(['m-1', 'm-2'])
    expect(state.detail.messageQuotes).toEqual([stored])
  })

  test('没有引用草稿时请求不带引用，也不凭空造出引用区', async () => {
    const h = harness(async request => started(request))
    h.addConversation(chat())
    await h.render().openConversation('chat')
    expect(await h.render().send('随便聊聊')).toBe(true)
    expect(h.startRequests[0].quoteBubbleId).toBeUndefined()
    expect(h.render().detail.messageQuotes ?? []).toEqual([])
  })

  test('Host 拒绝引用时发送失败：没有乐观消息、没有引用区，草稿引用不被消费', async () => {
    const h = harness(async () => { throw refusal('companion.quote_unavailable', '原消息已不可用，无法引用') })
    h.addConversation(chat())
    await h.render().openConversation('chat')
    const outcome = await h.render().send('按这个税率重算', undefined, [], undefined, 'run:response:2')
    // workbench 只在 started === true 时清空草稿，所以 false 意味着引用还在。
    expect(outcome).toBe(false)
    const state = h.render()
    expect(state.detail.messages.map((message: any) => message.id)).toEqual(['m-1'])
    expect(state.detail.messageQuotes ?? []).toEqual([])
    expect(state.detail.lastRun.id).toBe('run-old')
  })

  test('翻到更早的历史不会丢掉引用与标记，同一条以最新窗口为准', async () => {
    const older = quote({ messageId: 'm-0', bubbleId: 'run:response:1', sourceOrdinal: 0 })
    const h = harness(async request => started(request, { quote: quote() }), {
      historyPage: async () => ({
        messages: [{ id: 'm-0', conversationId: 'chat', runId: 'run-old', role: 'user', kind: 'text', content: '更早的问题',
          status: 'completed', ordinal: 0, createdAt: 1699999999000, updatedAt: 1699999999000 }],
        runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [],
        bubbles: [{ id: 'run:response:1', conversationId: 'chat', runId: 'run-old', messageId: 'a-0', sourceOrdinal: 1,
          author: 'companion', projection: 'segmented', segmentStart: 1, segmentEnd: 1, deliveryOrder: 1,
          deliveredAt: 1700000001000, content: '更早的回复', fingerprint: 'f' }],
        bubbleMarks: [{ bubbleId: 'run:response:1', favoritedAt: 1700000005000, reaction: null, reactionAt: null }],
        messageQuotes: [older], hasEarlierMessages: false,
      }),
    })
    h.addConversation(chat({
      hasEarlierMessages: true,
      bubbleMarks: [{ bubbleId: 'run:response:2', favoritedAt: null, reaction: '👍', reactionAt: 1700000006000 }],
      messageQuotes: [quote({ messageId: 'm-1', bubbleId: 'run:response:2', summary: '窗口里的最新摘要' })],
    }))
    await h.render().openConversation('chat')
    await h.render().loadEarlierMessages()
    const state = h.render()
    expect(state.detail.messages.map((message: any) => message.id)).toEqual(['m-0', 'm-1'])
    expect(state.detail.messageQuotes.map((item: any) => item.messageId).sort()).toEqual(['m-0', 'm-1'])
    expect(state.detail.messageQuotes.find((item: any) => item.messageId === 'm-1').summary).toBe('窗口里的最新摘要')
    expect(state.detail.bubbleMarks.map((item: any) => item.bubbleId).sort()).toEqual(['run:response:1', 'run:response:2'])
    expect(state.detail.bubbles.map((item: any) => item.id)).toEqual(['run:response:1', 'run:response:2'])
    expect(state.detail.hasEarlierMessages).toBe(false)
  })

  test('分页后再常规刷新：窗口外的引用与标记保留，窗口内以 Host 最新读取为准（含取消）', async () => {
    const older = quote({ messageId: 'm-0', bubbleId: 'run:response:1', sourceOrdinal: 0 })
    const olderMark = { bubbleId: 'run:response:1', favoritedAt: 1700000005000, reaction: null, reactionAt: null }
    const h = harness(async request => started(request), {
      mergeDetail: mergeConversationDetail,
      historyPage: async () => ({
        messages: [{ id: 'm-0', conversationId: 'chat', runId: 'run-old', role: 'user', kind: 'text', content: '更早的问题',
          status: 'completed', ordinal: 0, createdAt: 1699999999000, updatedAt: 1699999999000 }],
        runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [],
        bubbles: [{ id: 'run:response:1', conversationId: 'chat', runId: 'run-old', messageId: 'a-0', sourceOrdinal: 1,
          author: 'companion', projection: 'segmented', segmentStart: 1, segmentEnd: 1, deliveryOrder: 1,
          deliveredAt: 1700000001000, content: '更早的回复', fingerprint: 'f' }],
        bubbleMarks: [olderMark],
        messageQuotes: [older], hasEarlierMessages: false,
      }),
    })
    h.addConversation(chat({
      hasEarlierMessages: true,
      bubbleMarks: [{ bubbleId: 'run:response:2', favoritedAt: null, reaction: '👍', reactionAt: 1700000006000 }],
      messageQuotes: [quote({ messageId: 'm-1', bubbleId: 'run:response:2', summary: '窗口里的旧摘要' })],
    }))
    await h.render().openConversation('chat')
    await h.render().loadEarlierMessages()
    expect(h.render().detail.messageQuotes).toHaveLength(2)
    expect(h.render().detail.bubbleMarks).toHaveLength(2)

    // 常规刷新只重读最新窗口：窗口内的表情被取消、引用摘要更新；窗口外的一行都不会再读到。
    h.addConversation(chat({
      hasEarlierMessages: true,
      bubbleMarks: [],
      messageQuotes: [quote({ messageId: 'm-1', bubbleId: 'run:response:2', summary: '窗口里的最新摘要' })],
    }))
    await h.render().setKnowledgeReferences([], {})
    const state = h.render()
    expect(state.detail.messages.map((message: any) => message.id)).toEqual(['m-0', 'm-1'])
    expect(state.detail.messageQuotes.map((item: any) => item.messageId).sort()).toEqual(['m-0', 'm-1'])
    expect(state.detail.messageQuotes.find((item: any) => item.messageId === 'm-0')).toEqual(older)
    expect(state.detail.messageQuotes.find((item: any) => item.messageId === 'm-1').summary).toBe('窗口里的最新摘要')
    expect(state.detail.bubbleMarks).toEqual([olderMark])
    expect(state.detail.bubbles.map((item: any) => item.id)).toEqual(['run:response:1', 'run:response:2'])
  })
})
