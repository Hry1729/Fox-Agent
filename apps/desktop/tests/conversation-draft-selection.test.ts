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
        creates++
        createdRequest = request
        active = { conversation: { ...request, id: 'conversation-1' }, messages: [], attachments: [], knowledgeBindings: [], expertBindings: [], lastRun: null }
        return active.conversation
      },
      loadConversation: async () => active,
      setKnowledgeReferences: async (_id: string, references: any, names: any) => { savedReferences = { references, names }; active.knowledgeReferences = references },
      listConversations: async () => [],
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
  return { render: () => { cursor = 0; return hook() }, creates: () => creates, submitted: () => ({ createdRequest, savedReferences }) }
}

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
