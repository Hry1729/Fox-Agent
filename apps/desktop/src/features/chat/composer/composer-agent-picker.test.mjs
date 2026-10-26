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
    // Codex is hidden (HIDDEN_AGENT_BACKENDS): a pick of it stays Fox.
    const expected = backend === 'codex' ? 'fox' : backend
    expect(result.current.draftAgentBackend).toBe(expected)
    await act(async () => { await result.current.send('测试消息') })
    expect(requests).toHaveLength(1)
    expect(requests[0].agentBackend ?? 'fox').toBe(expected)
    expect(requests[0].projectRoot).toBe('D:/test-project')
    expect(requests[0].permissionMode).toBe('ask')
    expect(requests[0].externalPermissionMode).toBeUndefined()
  })
}

test('starting another draft clears its override and reads the saved Agent default', async () => {
  const requests = fakeHost('deepseek')
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => result.current.setDraftAgentBackend('pi'))
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => { await result.current.send('使用保存的默认值') })
  expect(requests[0].agentBackend).toBe('deepseek')
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

test('the Agent menu lists Fox, pi and DeepSeek (Codex hidden) and retains the unavailable reason', async () => {
  let selected = null
  const view = render(createElement(ComposerAgentPicker, { value: 'fox', status: { switchEnv: 'FOX_EXTERNAL_BACKENDS', anyExternalEnabled: true, unknownEntries: [], options: [
    { backend: 'fox', enabled: true, wired: true, experimental: false, reason: null },
    { backend: 'pi', enabled: true, wired: true, experimental: false, reason: null },
    { backend: 'codex', enabled: true, wired: true, experimental: false, hidden: true, reason: null },
    { backend: 'deepseek', enabled: false, wired: true, experimental: false, reason: '测试禁用原因' },
  ] }, onSelect: value => { selected = value } }))
  fireEvent.keyDown(view.getByRole('button', { name: '选择 Agent，当前 Fox' }), { key: 'ArrowDown' })
  await waitFor(() => expect(view.getByRole('menuitem', { name: 'pi' })).toBeTruthy())
  expect(view.queryByRole('menuitem', { name: /Codex/ })).toBeNull()
  expect(view.getByRole('menuitem', { name: /DeepSeek/ }).getAttribute('data-disabled')).not.toBeNull()
  expect(view.getByText('测试禁用原因')).toBeTruthy()
  fireEvent.click(view.getByRole('menuitem', { name: 'pi' }))
  expect(selected).toBe('pi')
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
