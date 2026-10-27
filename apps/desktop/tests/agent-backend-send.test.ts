import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolveConversationAgentId } from '../src/features/conversations/model/agent-initialization'
import { knowledgeReferenceKey, knowledgeReferenceFromLegacyBinding } from '../src/features/conversations/api/desktop-client'
import { conversationRunFingerprint, conversationRunState, snapshotForRun } from '../src/features/conversations/model/kernel-snapshot'
import {
  EMPTY_CONVERSATION_RUN_OVERLAY,
  applyApprovalToOverlay,
  applyRunEventToOverlay,
  conversationRunIndicators,
  pruneRunOverlay,
} from '../src/features/chat/conversation-run-indicator'
import {
  AGENT_BACKEND_REFUSED,
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
// Single Pi mainline (product decision 2026-10-27): `agent_backend_retired` is
// the refusal a retired backend produces, and a conversation frozen to one is
// read-only, so a send is refused locally with that same code and wording.
const source = readFileSync(new URL('../src/features/conversations/hooks/use-desktop-conversation.ts', import.meta.url), 'utf8')
const body = new Bun.Transpiler({ loader: 'ts' }).transformSync(source.slice(source.indexOf('export function useDesktopConversation'), source.indexOf('export function latestMessage')).replace('export function', 'function'))

function harness(startRun: (request: any) => Promise<any>) {
  const slots: any[] = []
  let cursor = 0
  const slot = (initial: any) => {
    const index = cursor++
    if (!(index in slots)) slots[index] = initial
    return index
  }
  const startRequests: any[] = []
  const conversations = new Map<string, any>()
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
        const conversation = { ...request, id: 'conversation-new', agentBackend: request.agentBackend ?? 'fox' }
        conversations.set(conversation.id, { conversation, messages: [], attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null })
        return conversation
      },
      // The Host wrote nothing for a refused send: the persisted detail is unchanged.
      loadConversation: async (id: string) => conversations.get(id),
      listConversations: async () => [...conversations.values()].map((item) => item.conversation),
      listArchivedConversations: async () => [],
      listTrashedConversations: async () => [],
      runtimeStatus: async () => ({}),
      startRun: (request: any) => { startRequests.push(request); return startRun(request) },
    },
    optimisticRun: () => ({ id: 'pending-run-1', status: 'queued' }),
    optimisticId: () => 'pending-message-1',
    flushSync: (fn: () => void) => fn(),
    extractedAttachmentContext: async () => '',
    runIsActive: () => false,
    conversationRunFingerprint,
    conversationRunState,
    snapshotForRun,
    resolveConversationAgentId,
    withWorkspaceInitializationTimeout: (promise: Promise<unknown>) => promise,
    activeConversationExpertBinding: (detail: any) => detail?.expertBindings?.find((item: any) => item.state === 'active'),
    knowledgeReferenceKey,
    knowledgeReferenceFromLegacyBinding,
    desktopErrorDetails: (error: any) => ({ code: error.code ?? 'unknown', message: error.message, retryable: false }),
    mergeConversationDetail: (persisted: any) => persisted,
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
    addConversation: (detail: any) => conversations.set(detail.conversation.id, detail),
  }
}

const refusal = (code: string, message: string) => Object.assign(new Error(message), { code })

describe('a send refused by backend routing leaves no optimistic trace', () => {
  test('a new conversation that asks for a retired backend: the refusal is a routing refusal, not a failed run', async () => {
    const refusalMessage = 'Agent 后端「pi」已在本版本退休：新会话只使用内置 Pi 驱动，不能再选择或启动它'
    const h = harness(async () => { throw refusal('agent_backend_retired', refusalMessage) })
    h.render().setDraftAgentBackend('pi')
    const outcome = await h.render().send('你好')
    expect(outcome).toBe(AGENT_BACKEND_REFUSED)
    expect(h.startRequests).toHaveLength(1)
    expect(h.startRequests[0].agentBackend).toBe('pi')
    const state = h.render()
    expect(state.detail?.conversation.id).toBe('conversation-new')
    expect(state.detail?.messages).toEqual([])
    expect(state.detail?.lastRun).toBeNull()
    // Not the generic 「处理失败」 state: one routing refusal instead.
    expect(state.error).toBeNull()
    expect(state.errorDetails).toBeNull()
    expect(state.agentBackendRefusal).toEqual({ conversationId: 'conversation-new', code: 'agent_backend_retired', message: refusalMessage })
    expect(isAgentBackendRefusalCode(state.agentBackendRefusal?.code)).toBe(true)
    // Without a loaded Host status the open pi conversation fails closed as
    // read-only with the retirement code: the next send never reaches the Host.
    expect(await h.render().send('再试一次')).toBe(AGENT_BACKEND_REFUSED)
    expect(h.startRequests).toHaveLength(1)
    expect(h.render().agentBackendRefusal?.code).toBe('agent_backend_retired')
    expect(h.render().agentBackendRefusal?.message).toContain('退休')
    expect(h.render().detail?.messages).toEqual([])
  })

  test('an existing conversation bound to a retired backend is read-only: nothing is sent', async () => {
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
    // The retirement refusal, naming the conversation's own backend: it is
    // never relabelled as the built-in Pi driver.
    expect(state.agentBackendRefusal?.code).toBe('agent_backend_retired')
    expect(state.agentBackendRefusal?.message).toContain('DeepSeek')
    expect(state.agentBackendRefusal?.message).toContain('退休')
    expect(state.agentBackendRefusal?.message).not.toContain('内置')
  })

  test('any other start failure keeps the existing error path unchanged', async () => {
    const h = harness(async () => { throw refusal('runtime.resolve_failed', '运行时不可用') })
    const outcome = await h.render().send('你好')
    expect(outcome).toBe(false)
    const state = h.render()
    expect(state.error).toBe('运行时不可用')
    expect(state.agentBackendRefusal).toBeNull()
  })
})
