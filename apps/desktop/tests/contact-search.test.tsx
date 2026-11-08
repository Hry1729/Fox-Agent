import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks'
import AssistantUiChat from '../src/features/chat/thread/fox-assistant-ui-chat'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import { ContactTopbar } from '../src/features/companions/chat-controls'
import { formatSearchTime } from '../src/features/companions/chat-search'
import type { ContactBubbleSearchHit, ConversationDetail, ConversationMessage } from '../src/features/conversations/model/types'

const context: ContactsContextValue = {
  current: { id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
    duty: '', personaOverride: null, workspaceDir: 'D:/contact', workspaceState: 'ready', workspaceError: null,
    isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null },
  privateChat: true, list: { companions: [], pendingDeletions: [] }, projects: [], model: 'test', permission: 'ask', error: '',
  refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
  leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
}
const message = (id: string, role: 'user' | 'assistant', content: string, patch: Partial<ConversationMessage> = {}): ConversationMessage => ({
  id, runId: 'run', conversationId: 'chat', role, kind: 'text',
  content, status: 'completed', ordinal: 2, createdAt: 1700000000000, updatedAt: 1700000000000, ...patch,
})
const detail = (patch: Partial<ConversationDetail> = {}): ConversationDetail => ({
  conversation: { id: 'chat', agentId: 'fox-general', agentName: 'Fox', title: '联系人', projectId: null, projectRoot: null,
    permissionMode: 'ask', status: 'active', createdAt: 0, updatedAt: 0, lastMessageAt: null },
  companionId: 'contact', messages: [], runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [],
  knowledgeBindings: [], expertBindings: [], hasEarlierMessages: false, goals: [], tasks: [], evidence: [],
  planRevisions: [], reviewFindings: [], acceptances: [], childRuns: [], expertWorkflow: null, expertTeam: null,
  lastRun: { id: 'run', conversationId: 'chat', runtimeSessionId: null, model: 'test', status: 'completed', lastSeq: 1,
    errorCode: null, errorMessage: null, startedAt: 1700000000000, finishedAt: 1700000001000 }, ...patch,
})
const bubble = (id: string, content: string, order: number) => ({
  id, conversationId: 'chat', runId: 'run', messageId: 'kernel-message:run:2', sourceOrdinal: 2,
  author: 'companion' as const, projection: 'segmented' as const, segmentStart: order, segmentEnd: order,
  deliveryOrder: order, deliveredAt: 1700000000000 + order, content, fingerprint: '',
})
const hit = (id: string, author: 'user' | 'companion', snippet: string, matchStart: number, matchEnd: number): ContactBubbleSearchHit => ({
  id, author, snippet, matchStart, matchEnd, deliveredAt: Date.now(), sourceOrdinal: 3,
  runId: 'run', messageId: id.startsWith('run:') ? 'kernel-message:run:2' : id,
})

afterEach(() => { cleanup(); clearMocks() })

test('搜索时间用本地时区：今天带时刻，同年省略年份，跨年补全年份', () => {
  const now = new Date('2026-11-07T15:00:00').getTime()
  expect(formatSearchTime(new Date('2026-11-07T14:33:00').getTime(), now)).toBe('今天 14:33')
  expect(formatSearchTime(new Date('2026-10-12T09:05:00').getTime(), now)).toBe('10 月 12 日')
  expect(formatSearchTime(new Date('2025-10-12T09:05:00').getTime(), now)).toBe('2025 年 10 月 12 日')
})

describe('联系人私聊搜索', () => {
  const settled = () => detail({
    messages: [message('user-1', 'user', '帮我核对资料'), message('kernel-message:run:2', 'assistant', '帮我核对资料')],
    bubbles: [bubble('run:response:3', '帮我核对资料', 3)],
  })
  const renderTopbar = (snapshot: ConversationDetail, loader: () => Promise<void> = async () => {}, ctx = context) =>
    render(<ContactsContext.Provider value={ctx}>
      <ContactTopbar detail={snapshot} onReset={() => {}} loadEarlierMessages={loader} />
      <AssistantUiChat conversation={{ enabled: true, detail: snapshot, streamingText: '' } as any} callbacks={{ onSubmit: () => {} }} />
    </ContactsContext.Provider>)
  const mockSearch = (results: ContactBubbleSearchHit[], calls: Record<string, unknown>[] = []) => {
    mockIPC((command, args) => {
      if (command === 'companion_chat_search') {
        calls.push((args as { request: Record<string, unknown> }).request)
        return { ok: true, data: { total: results.length, results } }
      }
      throw new Error('Unexpected command: ' + command)
    })
    return calls
  }
  const openAndType = async (view: ReturnType<typeof renderTopbar>, query: string) => {
    fireEvent.click(view.getByRole('button', { name: '搜索私聊' }))
    fireEvent.change(view.getByRole('textbox', { name: '搜索私聊' }), { target: { value: query } })
  }

  test('关键词命中后按条定位：气泡加圈、计数 chip、上下切换、关闭清除', async () => {
    const calls = mockSearch([
      hit('run:response:3', 'companion', '帮我核对资料', 2, 4),
      hit('user-1', 'user', '帮我核对资料', 2, 4),
    ])
    const view = renderTopbar(settled())
    await waitFor(() => expect(view.container.querySelector('[data-contact-message-id="run:response:3"]')).toBeTruthy())
    await openAndType(view, '核对')
    await waitFor(() => expect(view.getByText('2 条消息')).toBeDefined())
    expect(calls[0]).toMatchObject({ conversationId: 'chat', query: '核对', limit: 100 })
    expect(view.getByText('只搜索和资料伙伴的私聊')).toBeDefined()
    expect(view.container.querySelector('.fox-contact-search-result mark')?.textContent).toBe('核对')

    fireEvent.click(view.container.querySelectorAll('.fox-contact-search-result')[0])
    await waitFor(() => expect(view.getByText('1 / 2')).toBeDefined())
    expect(view.container.querySelector('[data-contact-message-id="run:response:3"]')?.classList.contains('is-search-hit')).toBe(true)

    fireEvent.click(view.getByRole('button', { name: '下一条结果' }))
    await waitFor(() => expect(view.getByText('2 / 2')).toBeDefined())
    expect(view.container.querySelector('[data-turn-message-id="user-1"]')?.classList.contains('is-search-hit')).toBe(true)
    expect(view.container.querySelector('[data-contact-message-id="run:response:3"]')?.classList.contains('is-search-hit')).toBe(false)

    fireEvent.click(view.getByRole('button', { name: '关闭搜索' }))
    await waitFor(() => expect(view.getByRole('button', { name: '搜索私聊' })).toBeDefined())
    expect(view.container.querySelector('.is-search-hit')).toBeNull()
  })

  test('时间范围与关键词组合查询，自定义日期按本地日起止', async () => {
    const calls = mockSearch([hit('run:response:3', 'companion', '帮我核对资料', 2, 4)])
    const view = renderTopbar(settled())
    await openAndType(view, '核对')
    await waitFor(() => expect(calls).toHaveLength(1))
    expect(calls[0].since).toBeUndefined()

    fireEvent.click(view.getByRole('button', { name: '最近 7 天' }))
    await waitFor(() => expect(calls).toHaveLength(2))
    expect(typeof calls[1].since).toBe('number')
    expect(calls[1].query).toBe('核对')

    fireEvent.click(view.getByRole('button', { name: '自定义' }))
    fireEvent.change(view.getByLabelText('开始日期'), { target: { value: '2026-10-01' } })
    fireEvent.change(view.getByLabelText('结束日期'), { target: { value: '2026-10-31' } })
    await waitFor(() => expect(calls.length).toBeGreaterThanOrEqual(3))
    const last = calls.at(-1)!
    expect(last.since).toBe(new Date('2026-10-01T00:00:00').getTime())
    expect(last.until).toBe(new Date('2026-10-31T23:59:59.999').getTime())
  })

  test('命中未加载的历史时先翻页再定位，翻页后仍是同一气泡身份', async () => {
    mockSearch([hit('run:response:9', 'companion', '上周核对的资料', 2, 4)])
    let loads = 0
    let snapshot = detail({ hasEarlierMessages: true })
    const view = renderTopbar(snapshot, async () => {
      loads += 1
      snapshot = detail({ messages: [message('kernel-message:run:2', 'assistant', '上周核对的资料')], bubbles: [bubble('run:response:9', '上周核对的资料', 9)] })
      await act(async () => { view.rerender(<ContactsContext.Provider value={context}>
        <ContactTopbar detail={snapshot} onReset={() => {}} loadEarlierMessages={async () => { loads += 1 }} />
        <AssistantUiChat conversation={{ enabled: true, detail: snapshot, streamingText: '' } as any} callbacks={{ onSubmit: () => {} }} />
      </ContactsContext.Provider>) })
    })
    await openAndType(view, '核对')
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-search-result')).toHaveLength(1))
    fireEvent.click(view.container.querySelector('.fox-contact-search-result')!)
    await waitFor(() => expect(loads).toBe(1))
    await waitFor(() => expect(view.getByText('1 / 1')).toBeDefined())
    expect(view.container.querySelector('[data-contact-message-id="run:response:9"]')?.classList.contains('is-search-hit')).toBe(true)
  })

  test('切换联系人清空结果与高亮，空关键词和无结果各有说明', async () => {
    mockSearch([hit('run:response:3', 'companion', '帮我核对资料', 2, 4)])
    const view = renderTopbar(settled())
    await openAndType(view, '核对')
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-search-result')).toHaveLength(1))
    fireEvent.click(view.container.querySelector('.fox-contact-search-result')!)
    await waitFor(() => expect(view.getByText('1 / 1')).toBeDefined())

    const nextContext = { ...context, current: { ...context.current!, id: 'other', conversationId: 'other-chat' } }
    view.rerender(<ContactsContext.Provider value={nextContext}>
      <ContactTopbar detail={detail({ conversation: { ...detail().conversation, id: 'other-chat' } })} onReset={() => {}} loadEarlierMessages={async () => {}} />
    </ContactsContext.Provider>)
    await waitFor(() => expect(view.getByRole('button', { name: '搜索私聊' })).toBeDefined())
    expect(view.container.querySelector('.is-search-hit')).toBeNull()
    expect(view.container.querySelector('.fox-contact-search-chip')).toBeNull()

    fireEvent.click(view.getByRole('button', { name: '搜索私聊' }))
    expect(view.getByText('输入关键词，搜索和资料伙伴的私聊')).toBeDefined()
    mockSearch([])
    fireEvent.change(view.getByRole('textbox', { name: '搜索私聊' }), { target: { value: '不存在的词' } })
    await waitFor(() => expect(view.getByText('没有找到相关消息')).toBeDefined())
  })

  test('Esc 关闭面板并保留聊天记录原样', async () => {
    mockSearch([hit('run:response:3', 'companion', '帮我核对资料', 2, 4)])
    const view = renderTopbar(settled())
    await openAndType(view, '核对')
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-search-result')).toHaveLength(1))
    fireEvent.keyDown(view.getByRole('textbox', { name: '搜索私聊' }), { key: 'Escape' })
    await waitFor(() => expect(view.getByRole('button', { name: '搜索私聊' })).toBeDefined())
    expect(view.container.querySelector('.fox-contact-search-panel')).toBeNull()
    expect(view.container.querySelector('[data-contact-message-id="run:response:3"]')?.textContent).toContain('帮我核对资料')
  })
})