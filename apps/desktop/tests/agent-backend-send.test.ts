import { describe, expect, test } from 'bun:test'
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

// Host wiring fix 1 (Windows acceptance of 3a82f5d): a send refused by backend
// routing (`agent_backend_*`) must leave no optimistic message / pending run,
// must not set the generic error (the header would show 「处理失败」), and must
// surface exactly one refusal. Same sandbox technique as
// conversation-draft-selection.test.ts: the production hook body runs with
// deterministic state slots and a fake Host; effects are not run.
//
// Plan C1 (2026-10-31): the external Agents are back; the administrator can
// still switch one off (`FOX_EXTERNAL_BACKENDS`), and the Host then refuses a
// new conversation with `agent_backend_disabled`. A conversation frozen to a
// backend that is switched off (or retired: the mechanism is kept) is
// read-only, so a send is refused locally with the same code and wording.
// Without a loaded Host status the mirror fails closed the same way.
const source = readFileSync(new URL('../src/features/conversations/hooks/use-desktop-conversation.ts', import.meta.url), 'utf8')
const body = new Bun.Transpiler({ loader: 'ts' }).transformSync(source.slice(source.indexOf('export function useDesktopConversation'), source.indexOf('export function latestMessage')).replace('export function', 'function'))

function harness(startRun: (request: any) => Promise<any>, options: {
  rewindRun?: (request: any) => Promise<any>
  loadConversation?: (id: string) => Promise<any>
  mergeDetail?: typeof mergeConversationDetail
} = {}) {
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
    agentBackendInputLockedReason,
    conversationAgentBackend,
    isAgentBackendRefusalCode,
    withoutRefusedSend,
    AGENT_BACKEND_PREFERENCES_EVENT,
    newConversationBackend,
    isAgentBackendHidden,
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

describe('a send refused by backend routing leaves no optimistic trace', () => {
  test('a new conversation the Host refuses before a run exists is undone, not kept as a chat', async () => {
    // 旧断言是：新建会话被 agent_backend_disabled 拒绝后仍保持选中，并显示路由拒绝。
    // 这次发送失败要回到新对话，空会话不能留在侧栏，所以改成断言 purge 和草稿恢复。
    // 已打开会话的只读拒绝仍由下一条测试覆盖。
    const refusalMessage = 'Agent 后端「pi」已被管理员关闭（FOX_EXTERNAL_BACKENDS），不能用于新会话'
    const h = harness(async () => { throw refusal('agent_backend_disabled', refusalMessage) })
    h.render().setDraftAgentBackend('pi')
    h.render().selectProject('D:/work/app', 'ask')
    const outcome = await h.render().send('你好')
    expect(outcome).toBe(false)
    expect(h.startRequests).toHaveLength(1)
    expect(h.startRequests[0].agentBackend).toBe('pi')
    expect(h.purged()).toEqual(['conversation-new-1'])
    const state = h.render()
    expect(state.selectedConversationId).toBeNull()
    expect(state.detail).toBeNull()
    expect(state.error).toBeNull()
    expect(state.errorDetails).toBeNull()
    expect(state.agentBackendRefusal).toBeNull()
    expect(state.draftAgentBackend).toBe('pi')
    expect(state.draftProjectRoot).toBe('D:/work/app')
    expect(state.lastSendFailureReason()).toBe(`agent_backend_disabled: ${refusalMessage}`)
  })

  test('an existing conversation bound to a switched-off backend is read-only: nothing is sent', async () => {
    const h = harness(async () => { throw new Error('startRun must not be called') })
    const previousRun = { id: 'run-old', status: 'completed', lastSeq: 3 }
    h.addConversation({
      conversation: { id: 'ds-1', agentId: 'fox-general', agentBackend: 'deepseek', permissionMode: 'ask' },
      messages: [{ id: 'm-1', role: 'user', content: '旧消息', ordinal: 1 }],
      attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: previousRun,
    })
    await h.render().openConversation('ds-1')
    const outcome = await h.render().send('还能发吗')
    expect(outcome).toBe(AGENT_BACKEND_REFUSED)
    expect(h.startRequests).toHaveLength(0)
    const state = h.render()
    expect(state.detail?.messages.map((message: any) => message.id)).toEqual(['m-1'])
    expect(state.detail?.lastRun).toBe(previousRun)
    expect(state.error).toBeNull()
    expect(state.agentBackendRefusal?.conversationId).toBe('ds-1')
    // The switched-off refusal, naming the conversation's own backend: it is
    // never relabelled as the built-in Pi driver, and it is not a retirement.
    expect(state.agentBackendRefusal?.code).toBe('agent_backend_disabled')
    expect(state.agentBackendRefusal?.message).toContain('DeepSeek')
    expect(state.agentBackendRefusal?.message).toContain('未启用')
    expect(state.agentBackendRefusal?.message).not.toContain('退休')
    expect(state.agentBackendRefusal?.message).not.toContain('内置')
  })

  test('any other start failure on an existing conversation keeps the error path unchanged', async () => {
    // 旧断言用的是新对话首发。新对话在 Run 写入前失败现在会撤掉空会话，
    // 所以这条改为已有对话：失败仍留下错误，且不 purge。
    const h = harness(async () => { throw refusal('runtime.resolve_failed', '运行时不可用') })
    h.addConversation({
      conversation: { id: 'fox-1', agentId: 'fox-general', agentBackend: 'fox', permissionMode: 'ask' },
      messages: [{ id: 'm-1', role: 'user', content: '旧消息', ordinal: 1 }],
      attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null,
    })
    await h.render().openConversation('fox-1')
    const outcome = await h.render().send('你好')
    expect(outcome).toBe(false)
    const state = h.render()
    expect(state.selectedConversationId).toBe('fox-1')
    expect(state.error).toBe('运行时不可用')
    expect(state.agentBackendRefusal).toBeNull()
    expect(h.purged()).toEqual([])
  })
})

describe('a new conversation that fails before a run row is rolled back', () => {
  test('startRun throwing model.connection_required purges the empty chat and restores the draft', async () => {
    const h = harness(async () => { throw refusal('model.connection_required', '需要模型连接') })
    const references = [{ source: 'local' as const, id: 'kb-1' }]
    const names = { [knowledgeReferenceKey(references[0])]: '经验库' }
    expect(h.render().createConversationDraftWithKnowledge('agent-writer', references, names)).toBe(true)
    h.render().setDraftAgentBackend('deepseek')
    h.render().setDraftNativePermission('workspace-write')
    h.render().selectProject('D:/work/app', 'ask')
    const outcome = await h.render().send('你好')
    expect(outcome).toBe(false)
    expect(h.purged()).toEqual(['conversation-new-1'])
    expect(h.createdRequests[0].agentBackend).toBe('deepseek')
    expect(h.createdRequests[0].agentId).toBe('agent-writer')
    expect(h.createdRequests[0].projectRoot).toBe('D:/work/app')
    expect(h.createdRequests[0].nativePermission).toBe('workspace-write')
    const state = h.render()
    expect(state.selectedConversationId).toBeNull()
    expect(state.detail).toBeNull()
    expect(state.draftProjectRoot).toBe('D:/work/app')
    expect(state.draftPermissionMode).toBe('ask')
    expect(state.draftAgentBackend).toBe('deepseek')
    expect(state.selectedAgentId ?? state.draftAgentId).toBe('agent-writer')
    expect(state.knowledgeReferences).toEqual(references)
    expect(state.knowledgeReferenceNames).toEqual(names)
    expect(state.lastSendFailureReason()).toBe('model.connection_required: 需要模型连接')
    expect(state.conversations.map((item: any) => item.id)).not.toContain('conversation-new-1')
    await h.render().send('再试一次')
    expect(h.createdRequests[1].agentId).toBe('agent-writer')
    expect(h.createdRequests[1].agentBackend).toBe('deepseek')
    expect(h.createdRequests[1].projectRoot).toBe('D:/work/app')
    expect(h.createdRequests[1].nativePermission).toBe('workspace-write')
    expect(h.purged()).toEqual(['conversation-new-1', 'conversation-new-2'])
  })

  test('a newly created conversation that already has messages is not purged', async () => {
    let loads = 0
    const h = harness(async () => { throw refusal('model.connection_required', '需要模型连接') }, {
      loadConversation: async (id) => {
        loads += 1
        const stored = {
          conversation: { id, agentId: 'fox-general', agentBackend: 'fox', permissionMode: 'ask' },
          messages: [] as any[],
          attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null,
        }
        // 创建后的第一次读取仍是空的；失败后的确认读取发现 Host 已经写入了消息。
        if (loads > 1) stored.messages = [{ id: 'written', role: 'user', content: '已写入', ordinal: 1 }]
        return stored
      },
    })
    const outcome = await h.render().send('你好')
    expect(outcome).toBe(false)
    expect(h.purged()).toEqual([])
    const state = h.render()
    expect(state.selectedConversationId).toBe('conversation-new-1')
    expect(state.error).toBe('需要模型连接')
    expect(state.agentBackendRefusal).toBeNull()
  })

  test('saving attachments failing before a run also purges the empty chat', async () => {
    const h = harness(async () => { throw new Error('startRun must not be called') })
    const outcome = await h.render().send('你好', undefined, [{ filename: 'a.txt', mediaType: 'text/plain', url: 'data:text/plain;base64,YQ==' }])
    expect(outcome).toBe(false)
    expect(h.startRequests).toHaveLength(0)
    expect(h.purged()).toEqual(['conversation-new-1'])
    expect(h.render().selectedConversationId).toBeNull()
    expect(h.render().lastSendFailureReason()).toContain('附件保存失败')
  })

  test('confirming the new conversation fails closed and does not purge', async () => {
    let loads = 0
    const h = harness(async () => { throw refusal('model.connection_required', '需要模型连接') }, {
      loadConversation: async (id) => {
        loads += 1
        if (loads > 1) throw Object.assign(new Error('读取对话失败'), { code: 'conversation.load_failed' })
        return {
          conversation: { id, agentId: 'fox-general', agentBackend: 'fox' },
          messages: [], attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null,
        }
      },
    })
    const outcome = await h.render().send('你好')
    expect(outcome).toBe(false)
    expect(h.purged()).toEqual([])
    const state = h.render()
    expect(state.selectedConversationId).toBe('conversation-new-1')
    expect(state.error).toBe('读取对话失败')
    expect(state.errorDetails?.code).toBe('conversation.load_failed')
  })
})

describe('a refused rewind restores the failed run for another retry', () => {
  function failedDetail() {
    return {
      conversation: { id: 'native-1', agentId: 'fox-general', agentBackend: 'fox', permissionMode: 'ask' },
      messages: [{ id: 'user-1', conversationId: 'native-1', runId: 'failed-1', role: 'user', kind: 'text', content: '旧问题', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1 }],
      runtimeEvents: [{ runId: 'failed-1', seq: 1, eventType: 'run.failed', event: { type: 'run.failed' }, createdAt: 2 }],
      toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [], expertBindings: [],
      goals: [], tasks: [], evidence: [],
      lastRun: { id: 'failed-1', status: 'failed', errorCode: 'runtime.start_failed', errorMessage: 'session.native_corrupt', lastSeq: 1 },
    }
  }

  for (const reloadFails of [false, true]) {
    test(`two rejected retries retain the original message and failure (reload fails: ${reloadFails})`, async () => {
      const persisted = failedDetail()
      let attempts = 0
      const h = harness(async () => { throw new Error('startRun must not be called') }, {
        rewindRun: async () => {
          attempts += 1
          throw refusal('session.native_unavailable', '原生会话不可用')
        },
        loadConversation: async () => {
          if (reloadFails && attempts > 0) throw new Error('read unavailable')
          return persisted
        },
        mergeDetail: mergeConversationDetail,
      })
      h.addConversation(persisted)
      await h.render().openConversation('native-1')
      for (let attempt = 1; attempt <= 2; attempt += 1) {
        expect(await h.render().rerunFromMessage('user-1', '更长的编辑问题')).toBe(false)
        const state = h.render()
        expect(state.detail?.messages).toEqual(persisted.messages)
        expect(state.detail?.lastRun).toEqual(persisted.lastRun)
        expect(state.detail?.runtimeEvents).toEqual(persisted.runtimeEvents)
        expect(conversationRunIsActive(state.detail)).toBe(false)
        expect(state.errorDetails?.code).toBe('session.native_unavailable')
        expect(attempts).toBe(attempt)
      }
    })
  }

  test('a failure after Host commits the rewind does not restore removed history', async () => {
    const original = failedDetail()
    original.messages.push({ ...original.messages[0], id: 'later-user', ordinal: 2, content: '随后已删除的问题' })
    const committed = {
      ...failedDetail(),
      messages: [{ ...original.messages[0], runId: 'replacement-1', content: '修改后的问题', updatedAt: 2 }],
      runtimeEvents: [],
      lastRun: { ...original.lastRun, id: 'replacement-1', errorCode: 'work_mode.gate_failed' },
    }
    let stored = original
    const h = harness(async () => { throw new Error('startRun must not be called') }, {
      rewindRun: async () => {
        stored = committed
        throw refusal('work_mode.gate_failed', '工作模式检查失败')
      },
      loadConversation: async () => stored,
      mergeDetail: mergeConversationDetail,
    })
    h.addConversation(original)
    await h.render().openConversation('native-1')
    expect(await h.render().rerunFromMessage('user-1', '修改后的问题')).toBe(false)
    expect(h.render().detail?.messages).toEqual(committed.messages)
    expect(h.render().detail?.runtimeEvents).toEqual([])
    expect(h.render().detail?.lastRun).toEqual(committed.lastRun)
  })
})
