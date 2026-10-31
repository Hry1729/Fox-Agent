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
function fakeHost(defaultBackend = 'deepseek', retiredBackends = []) {
  const requests = []
  let conversation
  spies.push(spyOn(desktopClient, 'agentBackendPreferences').mockResolvedValue({ ...DEFAULT_AGENT_BACKEND_PREFERENCES, defaultBackend, effectiveBackend: defaultBackend, retiredBackends }))
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

test('starting another draft clears its override and uses the saved default (plan C1)', async () => {
  // 设置 →「新会话使用的 Agent」saved `deepseek` and the Host answers it as
  // effective: the next draft uses it, not the previous draft's `pi` override.
  const requests = fakeHost('deepseek')
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => result.current.setDraftAgentBackend('pi'))
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => { await result.current.send('使用保存的默认值') })
  expect(requests[0].agentBackend).toBe('deepseek')
})

test('a Host-retired saved default is never applied (retirement mechanism kept)', async () => {
  const requests = fakeHost('deepseek', ['deepseek'])
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => { await result.current.send('退休的默认值') })
  // The default route travels as an absent field.
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

test('a saved codex default is used for a new conversation (plan C1)', async () => {
  const requests = fakeHost('codex')
  const { result } = renderHook(() => useDesktopConversation())
  await act(async () => { await result.current.createConversationForAgent('fox-general') })
  await act(async () => { await result.current.send('保存过 codex') })
  expect(requests[0].agentBackend).toBe('codex')
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

test('the four choices render an Agent menu; only the built-in route left renders none', async () => {
  // Plan C1: Fox / Pi / Codex / DeepSeek are offered.
  const option = (backend, patch = {}) => ({ backend, enabled: true, wired: true, experimental: backend === 'deepseek', hidden: false, retired: false, reason: null, ...patch })
  const c1 = { switchEnv: 'FOX_EXTERNAL_BACKENDS', anyExternalEnabled: true, unknownEntries: [], retiredBackends: [], options: ['fox', 'pi', 'codex', 'deepseek'].map(backend => option(backend)) }
  const menu = render(createElement(ComposerAgentPicker, { value: 'fox', status: c1, onSelect: () => {} }))
  expect(menu.getByRole('button', { name: '选择 Agent，当前 Fox' })).toBeTruthy()
  menu.unmount()
  // A Host that retired every external driver (mechanism kept): one option is
  // not a choice, so the composer shows the label without a menu.
  const retired = { ...c1, anyExternalEnabled: false, retiredBackends: ['pi', 'codex', 'deepseek'], options: ['fox', 'pi', 'codex', 'deepseek'].map(backend => option(backend, backend === 'fox' ? {} : { enabled: false, wired: false, retired: true, reason: '本版本已退休' })) }
  const view = render(createElement(ComposerAgentPicker, { value: 'fox', status: retired, onSelect: () => { throw new Error('nothing can be selected') } }))
  expect(view.queryByRole('button', { name: /选择 Agent/ })).toBeNull()
  expect(view.getByText('Fox')).toBeTruthy()
  expect(view.queryByRole('menu')).toBeNull()
  expect(view.getByLabelText('当前 Agent：Fox').tagName).toBe('SPAN')
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
