import { afterEach, describe, expect, spyOn, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const { act, cleanup, renderHook } = await import('@testing-library/react')
const { desktopClient } = await import('../src/features/conversations/api/desktop-client')
const { useDesktopConversation } = await import('../src/features/conversations/hooks/use-desktop-conversation')
import type { ConversationDetail, ConversationExpertBinding } from '../src/features/conversations/model/types'

function binding(expertId: string): ConversationExpertBinding {
  return { id: expertId, conversationId: 'existing', expertId, state: 'active', activationSource: 'draft', displaySnapshot: { name: expertId, description: '', icon: null, category: null }, packageSnapshot: {}, activatedAt: 1 }
}
function detail(): ConversationDetail {
  return {
    conversation: { id: 'existing', agentId: 'fox-general', agentName: 'Fox', title: '保留当前对话', projectId: null, projectRoot: 'D:/project', status: 'active', archived: false, createdAt: 1, updatedAt: 1, lastMessageAt: null },
    expertBindings: [binding('old')], messages: [], runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [], lastRun: null, hasEarlierMessages: false, goals: [], tasks: [], evidence: [], planRevisions: [], reviewFindings: [], acceptances: [],
  }
}
const spies: Array<{ mockRestore(): void }> = []
afterEach(() => { cleanup(); spies.splice(0).forEach((spy) => spy.mockRestore()) })

describe('change expert without leaving the current conversation', () => {
  test('updates an empty existing conversation in place', async () => {
    const current = detail()
    spies.push(spyOn(desktopClient, 'loadConversation').mockImplementation(async () => structuredClone(current)))
    const bind = spyOn(desktopClient, 'bindConversationExpert').mockImplementation(async (_, id) => {
      current.expertBindings = [binding(id)]
      return current.expertBindings[0]
    })
    spies.push(bind)
    const { result } = renderHook(() => useDesktopConversation())
    await act(async () => { await result.current.openConversation('existing') })
    await act(async () => { expect((await result.current.changeExpert('new')).success).toBe(true) })
    expect(bind).toHaveBeenCalledWith('existing', 'new')
    expect(result.current.selectedConversationId).toBe('existing')
    expect(result.current.detail?.conversation.title).toBe('保留当前对话')
    expect(result.current.selectedExpertId).toBe('new')
  })

  test('changes experts after a message while preserving messages and selection', async () => {
    const current = detail()
    current.messages.push({ id: 'message', conversationId: 'existing', runId: null, role: 'user', kind: 'text', content: '保留这条消息', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1 })
    spies.push(spyOn(desktopClient, 'loadConversation').mockImplementation(async () => structuredClone(current)))
    const bind = spyOn(desktopClient, 'bindConversationExpert').mockImplementation(async (_, id) => {
      current.expertBindings = [binding(id)]
      return current.expertBindings[0]
    })
    spies.push(bind)
    const { result } = renderHook(() => useDesktopConversation())
    await act(async () => { await result.current.openConversation('existing') })
    await act(async () => { expect((await result.current.changeExpert('new')).success).toBe(true) })
    expect(bind).toHaveBeenCalledWith('existing', 'new')
    expect(result.current.selectedConversationId).toBe('existing')
    expect(result.current.selectedExpertId).toBe('new')
    expect(result.current.detail?.messages[0].content).toBe('保留这条消息')
  })

  test('backend failure leaves the existing conversation and expert intact', async () => {
    spies.push(spyOn(desktopClient, 'loadConversation').mockResolvedValue(detail()))
    spies.push(spyOn(desktopClient, 'bindConversationExpert').mockRejectedValue(new Error('service offline')))
    const { result } = renderHook(() => useDesktopConversation())
    await act(async () => { await result.current.openConversation('existing') })
    await act(async () => { expect((await result.current.changeExpert('new')).success).toBe(false) })
    expect(result.current.selectedConversationId).toBe('existing')
    expect(result.current.selectedExpertId).toBe('old')
  })

  test('a late binding response does not return to the old conversation', async () => {
    const other = detail()
    other.conversation.id = 'other'
    spies.push(spyOn(desktopClient, 'loadConversation').mockImplementation(async (id) => id === 'other' ? other : detail()))
    let finish!: (value: ConversationExpertBinding) => void
    spies.push(spyOn(desktopClient, 'bindConversationExpert').mockImplementation(() => new Promise((resolve) => { finish = resolve })))
    const { result } = renderHook(() => useDesktopConversation())
    await act(async () => { await result.current.openConversation('existing') })
    const pending = result.current.changeExpert('new')
    await act(async () => { await result.current.openConversation('other') })
    await act(async () => { finish(binding('new')); expect((await pending).success).toBe(false) })
    expect(result.current.selectedConversationId).toBe('other')
    expect(result.current.detail?.conversation.id).toBe('other')
  })

  test('changing a draft preserves its agent, project, permission and knowledge', async () => {
    const { result } = renderHook(() => useDesktopConversation())
    await act(async () => { await result.current.createConversationForAgent('fox-general', 'D:/draft', 'default') })
    await act(async () => { await result.current.setKnowledgeReferences([{ source: 'local', id: 'kb' }], { 'local:kb': '资料' }) })
    await act(async () => { expect((await result.current.changeExpert('new')).success).toBe(true) })
    expect(result.current.selectedConversationId).toBeNull()
    expect(result.current.selectedExpertId).toBe('new')
    expect(result.current.selectedAgentId).toBe('fox-general')
    expect(result.current.draftProjectRoot).toBe('D:/draft')
    expect(result.current.draftPermissionMode).toBe('default')
    expect(result.current.knowledgeReferences).toEqual([{ source: 'local', id: 'kb' }])
  })
})
