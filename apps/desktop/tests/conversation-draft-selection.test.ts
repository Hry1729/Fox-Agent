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

// Run the production hook's actions with deterministic state/ref slots. Effects
// are excluded here; these tests exercise selection ordering without a Host or DOM.
const source = readFileSync(new URL('../src/features/conversations/hooks/use-desktop-conversation.ts', import.meta.url), 'utf8')
const body = new Bun.Transpiler({ loader: 'ts' }).transformSync(source.slice(source.indexOf('export function useDesktopConversation'), source.indexOf('export function latestMessage')).replace('export function', 'function'))

function harness() {
  const slots: any[] = []
  let cursor = 0
  const slot = (initial: any) => {
    const index = cursor++
    if (!(index in slots)) slots[index] = initial
    return index
  }
  let creates = 0
  let createdRequest: any
  let savedReferences: any
  let active: any
  let conversationLoader: (id: string) => Promise<any> = async () => active
  let listLoader: () => Promise<any[]> = async () => []
  let runtimeStreamOptions: any
  const env = {
    useState: (initial: any) => { const i = slot(initial); return [slots[i], (value: any) => { slots[i] = typeof value === 'function' ? value(slots[i]) : value }] },
    useRef: (initial: any) => slots[slot({ current: initial })],
    useCallback: (callback: any) => callback,
    useMemo: (callback: any) => callback(),
    useEffect: () => {},
    useLayoutEffect: () => {},
    useRuntimeEventStream: (options: any) => { runtimeStreamOptions = options },
    useKernelStateStream: () => {},
    desktopRuntimeAvailable: false,
    desktopClient: {
      initialize: async () => ({ defaultAgentId: 'fox-general' }),
      createConversation: async (request: any) => {
        creates++
        createdRequest = request
        active = { conversation: { ...request, id: 'conversation-1' }, messages: [], attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null }
        return active.conversation
      },
      loadConversation: (id: string) => conversationLoader(id),
      setKnowledgeReferences: async (_id: string, references: any, names: any) => { savedReferences = { references, names }; active.knowledgeReferences = references },
      listConversations: () => listLoader(),
      listArchivedConversations: async () => [],
      listTrashedConversations: async () => [],
      runtimeStatus: async () => ({}),
      startRun: async () => ({ run: { id: 'run-1' }, userMessage: { id: 'message-1' }, attachments: [] }),
    },
    optimisticRun: () => ({ id: 'pending-run-1' }),
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
    desktopErrorDetails: (error: Error) => ({ message: error.message }),
    // Sidebar activity: the hook evaluates these eagerly for its derived map, so
    // they must exist in the sandbox even though its effects never run.
    EMPTY_CONVERSATION_RUN_OVERLAY,
    conversationRunIndicators,
    pruneRunOverlay,
    applyRunEventToOverlay,
    applyApprovalToOverlay,
  }
  const hook = new Function(...Object.keys(env), `${body}; return useDesktopConversation`)(...Object.values(env))
  return {
    render: () => { cursor = 0; return hook() },
    setConversationLoader: (loader: (id: string) => Promise<any>) => { conversationLoader = loader },
    setListLoader: (loader: () => Promise<any[]>) => { listLoader = loader },
    deliverDetail: (detail: any) => runtimeStreamOptions.setDetail(() => detail),
    creates: () => creates,
    submitted: () => ({ createdRequest, savedReferences }),
  }
}

describe('conversation navigation ownership', () => {
  const detail = (id: string) => ({
    conversation: { id, agentId: 'fox-general', permissionMode: 'ask', projectRoot: `D:/work/${id}` },
    messages: [], attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null,
  })

  test('late A load cannot replace a later B selection, including an A background update', async () => {
    const h = harness()
    const pending = new Map<string, (value: any) => void>()
    h.setConversationLoader((id) => new Promise((resolve) => pending.set(id, resolve)))
    const openA = h.render().openConversation('A')
    expect(h.render().selectedConversationId).toBe('A')
    const openB = h.render().openConversation('B')
    expect(h.render().selectedConversationId).toBe('B')
    pending.get('B')!(detail('B'))
    await openB
    pending.get('A')!(detail('A'))
    await openA
    h.deliverDetail(detail('A'))
    expect(h.render().selectedConversationId).toBe('B')
    expect(h.render().detail?.conversation.id).toBe('B')
    expect(h.render().detail?.conversation.projectRoot).toBe('D:/work/B')
  })

  test('leaving a conversation for a new project draft rejects an old detail response', async () => {
    const h = harness()
    let finishA!: (value: any) => void
    h.setConversationLoader(() => new Promise((resolve) => { finishA = resolve }))
    const openA = h.render().openConversation('A')
    h.render().selectProject('D:/work/new-project', 'ask')
    finishA(detail('A'))
    await openA
    expect(h.render().selectedConversationId).toBeNull()
    expect(h.render().detail).toBeNull()
    expect(h.render().draftProjectRoot).toBe('D:/work/new-project')
  })

  test('an older sidebar list response cannot replace a newer project list or selection', async () => {
    const h = harness()
    h.setConversationLoader(async (id) => detail(id))
    await h.render().openConversation('B')
    const pending: Array<(value: any[]) => void> = []
    h.setListLoader(() => new Promise((resolve) => pending.push(resolve)))
    const oldRefresh = h.render().refreshLifecycleLists()
    const newRefresh = h.render().refreshLifecycleLists()
    pending[1]([{ id: 'B', projectRoot: 'D:/work/B', title: 'B' }])
    await newRefresh
    pending[0]([{ id: 'A', projectRoot: 'D:/work/A', title: 'A' }])
    await oldRefresh
    expect(h.render().selectedConversationId).toBe('B')
    expect(h.render().conversations.map((item: any) => item.id)).toEqual(['B'])
  })
})

describe('starting a fresh draft', () => {
  function beginNewDraftHarness() {
    const workbench = readFileSync(new URL('../src/features/chat/workbench.tsx', import.meta.url), 'utf8')
    const slice = workbench.slice(workbench.indexOf('  const beginNewDraft = ('), workbench.indexOf('  const resetConversation = '))
    const compiled = new Bun.Transpiler({ loader: 'tsx' }).transformSync(slice)
    const calls: Record<string, unknown> = {}
    const setter = (name: string) => (value: unknown) => {
      calls[name] = typeof value === 'function' ? (value as (previous: unknown) => unknown)(5) : value
    }
    const beginNewDraft = new Function(
      'clearWorkflowTimer', 'setChatState', 'setActivePrompt', 'setPendingUserMessage',
      'setEmptyConversation', 'setComposerDraft', 'setComposerResetKey', 'setActiveEntityId',
      'setActiveDocumentId', 'setActiveSourceLocator', 'setAssistantMode', 'setActiveView',
      `${compiled}; return beginNewDraft`,
    )(
      () => { calls.clearWorkflowTimer = true },
      setter('chatState'), setter('activePrompt'), setter('pendingUserMessage'),
      setter('emptyConversation'), setter('composerDraft'), setter('composerResetKey'),
      setter('activeEntityId'), setter('activeDocumentId'), setter('activeSourceLocator'),
      setter('assistantMode'), setter('activeView'),
    )
    return { beginNewDraft, calls }
  }

  test('resets run identity, composer text and temporary state together', () => {
    const { beginNewDraft, calls } = beginNewDraftHarness()
    beginNewDraft()
    // The previous conversation's run state must not survive: a draft that keeps
    // "running" shows the stop button over an empty composer.
    expect(calls.chatState).toBe('complete')
    expect(calls.pendingUserMessage).toBeNull()
    expect(calls.composerDraft).toBe('')
    expect(calls.activePrompt).toBe('')
    expect(calls.emptyConversation).toBe(true)
    // The composer is remounted so its own internal state cannot linger either.
    expect(calls.composerResetKey).toBe(6)
    expect(calls.activeEntityId).toBeNull()
    expect(calls.activeDocumentId).toBeNull()
    expect(calls.activeSourceLocator).toBeNull()
    expect(calls.activeView).toBe('chat')
    // Nothing cancels the conversation that is still running in the background.
    expect('cancel' in calls).toBe(false)
    // The assistant mode is only touched when the caller asks for a change.
    expect('assistantMode' in calls).toBe(false)
  })

  test('keeps a pre-filled suggestion visible instead of forcing an empty composer', () => {
    const { beginNewDraft, calls } = beginNewDraftHarness()
    beginNewDraft({ suggestion: '帮我总结这份文档' })
    expect(calls.composerDraft).toBe('帮我总结这份文档')
    expect(calls.emptyConversation).toBe(false)
    expect(calls.chatState).toBe('complete')
  })

  test('an explicit empty draft wins over the layout heuristic', () => {
    const { beginNewDraft, calls } = beginNewDraftHarness()
    beginNewDraft({ suggestion: 'ignored layout hint', empty: true })
    expect(calls.emptyConversation).toBe(true)
  })

  test('switching assistant mode is applied when requested', () => {
    const { beginNewDraft, calls } = beginNewDraftHarness()
    beginNewDraft({ mode: 'knowledge' })
    expect(calls.assistantMode).toBe('knowledge')
  })
})

describe('composable conversation draft selections', () => {
  test('returning to chat does not create a draft, but New conversation does', () => {
    const workbench = readFileSync(new URL('../src/features/chat/workbench.tsx', import.meta.url), 'utf8')
    const selection = workbench.slice(workbench.indexOf('  const switchAssistantMode ='), workbench.indexOf('  const createProjectDraft ='))
    const compiled = new Bun.Transpiler({ loader: 'ts' }).transformSync(selection)
    const routes: string[] = []
    const drafts: string[] = []
    const change = new Function('setAssistantMode', 'navigate', 'agentResource', 'prepareDraft', `${compiled}; return switchAssistantMode`)(
      () => {}, (view: string) => routes.push(view), { agents: [{ id: 'fox-general' }] }, (id: string) => drafts.push(id),
    )
    change('assistant')
    expect(routes).toEqual(['chat'])
    expect(drafts).toEqual([])
    change('assistant', true)
    expect(drafts).toEqual(['fox-general'])
  })
  for (const order of ['pek', 'pke', 'epk', 'ekp', 'kpe', 'kep']) {
    test(`preserves project, expert and mixed knowledge in order ${order}`, async () => {
      const h = harness()
      const references = [{ source: 'local', id: 'kb-local' }, { source: 'remote', id: 'kb-remote', connectionId: 'yuxi-primary' }]
      const names = Object.fromEntries(references.map((reference, index) => [knowledgeReferenceKey(reference as any), index ? '远程制度库' : '本地经验库']))
      for (const step of order) {
        const state = h.render()
        if (step === 'p') state.selectProject('D:/work/project', 'read_only')
        if (step === 'e') expect((await state.createConversationForExpert('expert-a')).success).toBe(true)
        if (step === 'k') expect(await state.setKnowledgeReferences(references, names)).toBe(true)
      }
      const state = h.render()
      expect(state.draftProjectRoot).toBe('D:/work/project')
      expect(state.selectedExpertId).toBe('expert-a')
      expect(state.knowledgeReferences).toEqual(references)
      expect(state.knowledgeReferenceNames).toEqual(names)
      expect(h.creates()).toBe(0)
      // A re-render caused by navigating a workspace page keeps the draft.
      expect(h.render().knowledgeReferences).toEqual(references)
      expect(h.render().knowledgeReferenceNames).toEqual(names)
      await state.createConversation()
      const cleared = h.render()
      expect(cleared.draftProjectRoot).toBeNull()
      expect(cleared.selectedExpertId).toBeNull()
      expect(cleared.knowledgeReferences).toBeUndefined()
      expect(cleared.knowledgeReferenceNames).toEqual({})
    })
  }
  test('sends the combined draft and knowledge names to the Host only on first message', async () => {
    const h = harness()
    const references = [{ source: 'local', id: 'kb-local' }]
    const names = { [knowledgeReferenceKey(references[0] as any)]: '企业文化' }
    await h.render().setKnowledgeReferences(references, names)
    await h.render().createConversationForExpert('expert-a')
    h.render().selectProject('D:/work/project', 'read_only')
    expect(h.creates()).toBe(0)
    const state = h.render()
    expect(await state.send('测试问题')).toBe(true)
    expect(h.submitted()).toEqual({
      createdRequest: { agentId: 'fox-general', expertId: 'expert-a', projectRoot: 'D:/work/project', permissionMode: 'read_only' },
      savedReferences: { references, names },
    })
  })
})
