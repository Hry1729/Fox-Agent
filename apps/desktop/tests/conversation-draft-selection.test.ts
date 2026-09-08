import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolveConversationAgentId } from '../src/features/conversations/model/agent-initialization'
import { knowledgeReferenceKey, knowledgeReferenceFromLegacyBinding } from '../src/features/conversations/api/desktop-client'
import { conversationRunFingerprint, conversationRunState, snapshotForRun } from '../src/features/conversations/model/kernel-snapshot'

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
  }
  const hook = new Function(...Object.keys(env), `${body}; return useDesktopConversation`)(...Object.values(env))
  return { render: () => { cursor = 0; return hook() }, creates: () => creates, submitted: () => ({ createdRequest, savedReferences }) }
}

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
      for (const step of order) {
        const state = h.render()
        if (step === 'p') state.selectProject('D:/work/project', 'read_only')
        if (step === 'e') expect((await state.createConversationForExpert('expert-a')).success).toBe(true)
        if (step === 'k') expect(await state.setKnowledgeReferences(references, {})).toBe(true)
      }
      const state = h.render()
      expect(state.draftProjectRoot).toBe('D:/work/project')
      expect(state.selectedExpertId).toBe('expert-a')
      expect(state.knowledgeReferences).toEqual(references)
      expect(h.creates()).toBe(0)
      // A re-render caused by navigating a workspace page keeps the draft.
      expect(h.render().knowledgeReferences).toEqual(references)
      await state.createConversation()
      const cleared = h.render()
      expect(cleared.draftProjectRoot).toBeNull()
      expect(cleared.selectedExpertId).toBeNull()
      expect(cleared.knowledgeReferences).toBeUndefined()
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
