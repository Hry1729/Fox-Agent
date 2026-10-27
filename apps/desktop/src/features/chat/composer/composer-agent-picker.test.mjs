import { afterEach, expect, spyOn, test } from 'bun:test'
import { createElement } from 'react'
import { act, cleanup, fireEvent, render, renderHook, waitFor } from '@testing-library/react'
import { ComposerAgentPicker } from './composer-agent-picker'
import { ExpertBindingChip } from '../components/ExpertBindingChip'
import { desktopClient } from '../../conversations/api/desktop-client'
import { useDesktopConversation } from '../../conversations/hooks/use-desktop-conversation'
import { DEFAULT_AGENT_BACKEND_PREFERENCES } from '../../conversations/model/agent-backend'

const spies = []
afterEach(() => { cleanup(); spies.splice(0).forEach(spy => spy.mockRestore()) })
function fakeHost(defaultBackend = 'deepseek') {
  const requests = []
  let conversation
  spies.push(spyOn(desktopClient, 'agentBackendPreferences').mockResolvedValue({ ...DEFAULT_AGENT_BACKEND_PREFERENCES, defaultBackend, effectiveBackend: defaultBackend }))
  spies.push(spyOn(desktopClient, 'createConversation').mockImplementation(async request => {
    requests.push(request)
    conversation = { ...request, id: 'new', agentId: 'fox-general', title: 'test', agentBackend: request.agentBackend ?? 'fox' }
    return conversation
  }))
  spies.push(spyOn(desktopClient, 'loadConversation').mockImplementation(async () => ({ conversation, messages: [], runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [], expertBindings: [], goals: [], tasks: [], evidence: [], planRevisions: [], reviewFindings: [], acceptances: [], lastRun: null, hasEarlierMessages: false })))
  spies.push(spyOn(desktopClient, 'listConversations').mockImplementation(async () => conversation ? [conversation] : []))
  spies.push(spyOn(desktopClient, 'startRun').mockRejectedValue(Object.assign(new Error('fixture refuses execution'), { code: 'agent_backend_invalid' })))
  return requests
}

for (const backend of ['fox', 'pi', 'codex', 'deepseek']) {
  test(`explicit ${backend} selection reaches creation and survives choosing a project`, async () => {
    const requests = fakeHost()
    const { result } = renderHook(() => useDesktopConversation())
    await act(async () => { await result.current.createConversationForAgent('fox-general') })
    await act(async () => result.current.setDraftAgentBackend(backend))
    await act(async () => result.current.selectProject('D:/test-project', 'ask'))
    expect(result.current.draftAgentBackend).toBe(backend)
    await act(async () => { await result.current.send('测试消息') })
    expect(requests).toHaveLength(1)
    expect(requests[0].agentBackend ?? 'fox').toBe(backend)
    expect(requests[0].projectRoot).toBe('D:/test-project')
    expect(requests[0].permissionMode).toBe('ask')
    expect(requests[0].externalPermissionMode).toBeUndefined()
  })
}

test('starting another draft clears its override and never applies a retired saved default', async () => {
  // The Host still reports the saved `deepseek` value, but a new conversation
  // uses the built-in Pi route: a retired preference must not be applied
  // behind the user's back, and neither may the previous draft's override.
  const requests = fakeHost('deepseek')
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => result.current.setDraftAgentBackend('pi'))
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => { await result.current.send('使用保存的默认值') })
  // The default route travels as an absent field; anything else would be a
  // retired driver applied without the user choosing it.
  expect(requests[0].agentBackend ?? 'fox').toBe('fox')
})

test('project selected before switching Agent stays bound through repeated switches and send', async () => {
  const requests = fakeHost('fox')
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => result.current.selectProject('D:/preview-project', 'ask'))
  for (const backend of ['pi', 'deepseek', 'fox', 'pi']) {
    await act(async () => result.current.setDraftAgentBackend(backend))
    expect(result.current.draftProjectRoot).toBe('D:/preview-project')
  }
  await act(async () => { await result.current.send('测试项目绑定') })
  expect(requests[0].projectRoot).toBe('D:/preview-project')
  expect(requests[0].agentBackend).toBe('pi')
})

test('a saved codex default is not used for a new conversation', async () => {
  const requests = fakeHost('codex')
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => { await result.current.send('保存过 codex') })
  expect(requests[0].agentBackend ?? 'fox').toBe('fox')
})

test('external creation does not submit a Fox-only expert binding', async () => {
  const requests = fakeHost()
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => { await result.current.changeExpert('expert-review') })
  await act(async () => result.current.setDraftAgentBackend('pi'))
  await act(async () => { await result.current.send('使用外部 Agent') })
  expect(requests[0].agentBackend).toBe('pi')
  expect(requests[0].expertId).toBeUndefined()
})

test('a single supported driver renders no Agent menu at all', async () => {
  // One option is not a choice: the composer shows no picker even when an
  // older Host still reports four rows. The retired rows are never offered.
  const view = render(createElement(ComposerAgentPicker, { value: 'fox', status: { switchEnv: 'FOX_EXTERNAL_BACKENDS', anyExternalEnabled: true, unknownEntries: [], options: [
    { backend: 'fox', enabled: true, wired: true, experimental: false, reason: null },
    { backend: 'pi', enabled: true, wired: true, experimental: false, reason: null },
    { backend: 'codex', enabled: false, wired: false, experimental: false, hidden: false, retired: true, reason: '本版本已退休' },
    { backend: 'deepseek', enabled: false, wired: false, experimental: false, retired: true, reason: '本版本已退休' },
  ] }, onSelect: () => { throw new Error('nothing can be selected') } }))
  expect(view.queryByRole('button', { name: /选择 Agent/ })).toBeNull()
  expect(view.queryByRole('menu')).toBeNull()
  expect(view.container.childElementCount).toBe(0)
})

test('only the new-conversation expert entry opens selection directly', () => {
  const expert = { id: 'fox', name: 'Fox 通用助手', resources: { skills: [], knowledges: [], mcps: [], tools: [] } }
  let changed = 0
  const props = { expert, defaultLabel: true, readOnly: false, onView: () => {}, onChange: () => { changed++ } }
  const view = render(createElement(ExpertBindingChip, { ...props, directSelection: true }))
  fireEvent.click(view.getByRole('button', { name: '专家' }))
  expect(changed).toBe(1)
  view.rerender(createElement(ExpertBindingChip, props))
  expect(view.getByRole('button', { name: '默认专家' }).getAttribute('aria-haspopup')).toBe('menu')
})
