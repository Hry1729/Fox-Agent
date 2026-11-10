// C07：引用回复、收藏与表情。
//
// 被测对象是真实的 AssistantUiChat（assistant-ui 线程）与真实的 marks 组件，
// 挂到 happy-dom 文档上，用真实 DOM 事件驱动；Tauri IPC 边界用 mockIPC 替换，
// 记录每一次永久标记命令。未落库的气泡（直播预览、乐观发送、纯附件消息）
// 不提供任何永久入口；已落库气泡的标记只接受 K1 稳定身份。

import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { useState } from 'react'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks'
import AssistantUiChat from '../src/features/chat/thread/fox-assistant-ui-chat'
import { ContactsContext, showContactError, type ContactLocateRequest, type ContactsContextValue } from '../src/features/companions/context'
import { ContactTopbar } from '../src/features/companions/chat-controls'
import { ContactFavorites, ContactQuoteDraft } from '../src/features/companions/marks'
import { contacts, type Companion, type QuotePreview } from '../src/features/companions/api'
import type { ContactBubbleMarkRecord, ContactBubbleRecord, ContactMessageQuoteRecord, ConversationDetail, ConversationMessage } from '../src/features/conversations/model/types'

const contact: Companion = {
  id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
  duty: '', personaOverride: null, workspaceDir: '/tmp/contact', workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}

const bubble = (id: string, author: 'user' | 'companion', order: number, content: string): ContactBubbleRecord => ({
  id, conversationId: 'chat', runId: 'run', messageId: author === 'user' ? id : 'answer', sourceOrdinal: order,
  author, projection: author === 'user' ? 'message' : 'segmented', segmentStart: null, segmentEnd: null,
  deliveryOrder: order, deliveredAt: 1700000000000 + order, content, fingerprint: 'f',
})

const userMessage: ConversationMessage = {
  id: 'user-1', runId: 'run', conversationId: 'chat', role: 'user', kind: 'text',
  content: '帮我把报价整理成对比表', status: 'completed', ordinal: 1, createdAt: 1700000000000, updatedAt: 1700000000000,
}
const answerMessage: ConversationMessage = {
  id: 'answer', runId: 'run', conversationId: 'chat', role: 'assistant', kind: 'text',
  content: '华东机电的报价单没写税率，我先按 13% 估算。', status: 'completed', ordinal: 2, createdAt: 1700000001000, updatedAt: 1700000001000,
}

const detail = (patch: Partial<ConversationDetail> = {}): ConversationDetail => ({
  conversation: { id: 'chat', agentId: 'fox-general', agentName: 'Fox', title: '联系人', projectId: null, projectRoot: null,
    permissionMode: 'ask', status: 'active', createdAt: 0, updatedAt: 0, lastMessageAt: null },
  companionId: 'contact', messages: [userMessage, answerMessage], runtimeEvents: [], toolCalls: [], approvals: [],
  attachments: [], artifacts: [], knowledgeBindings: [], expertBindings: [], hasEarlierMessages: false, goals: [], tasks: [],
  evidence: [], planRevisions: [], reviewFindings: [], acceptances: [], childRuns: [], expertWorkflow: null, expertTeam: null,
  bubbles: [bubble('user-1', 'user', 1, userMessage.content), bubble('run:response:2', 'companion', 2, answerMessage.content)],
  lastRun: { id: 'run', conversationId: 'chat', runtimeSessionId: null, model: 'test', status: 'completed', lastSeq: 1,
    errorCode: null, errorMessage: null, startedAt: 1700000000000, finishedAt: 1700000002000 },
  ...patch,
})

const quote = (patch: Partial<ContactMessageQuoteRecord> = {}): ContactMessageQuoteRecord => ({
  messageId: 'user-1', bubbleId: 'run:response:2', author: 'companion',
  summary: '华东机电的报价单没写税率，我先按 13% 估算。', excerpt: '华东机电的报价单没写税率，我先按 13% 估算。',
  deliveredAt: 1700000002000, sourceOrdinal: 2, available: true, ...patch,
})

// 同一进程里别的测试可能已经把 @tauri-apps/api/core 的 invoke 换掉（且没还回来），
// 那样 mockIPC 就收不到调用。这里自己接管 invoke 边界，并在本文件结束后原样还回去。
const tauriCoreAsFound = { ...(await import('@tauri-apps/api/core')) }
let answer: ((command: string, args: unknown) => unknown) | null = null
mock.module('@tauri-apps/api/core', () => ({
  ...tauriCoreAsFound,
  invoke: async (command: string, args: unknown) => answer
    ? answer(command, args)
    : (tauriCoreAsFound.invoke as (c: string, a: unknown) => Promise<unknown>)(command, args),
}))
afterAll(() => { answer = null; mock.module('@tauri-apps/api/core', () => tauriCoreAsFound) })

// C06：永久标记失败时 showContactError 走 notify.error。这里接管它，断言原始
// Host 错误确实展示出来，而不是被前端吞掉冒充成功。
const actualNotifications = await import('../src/features/notifications')
const toasts: string[] = []
mock.module('../src/features/notifications', () => ({
  ...actualNotifications,
  notify: Object.assign(
    (message: unknown, options?: unknown) => actualNotifications.notify(message, options as never),
    { ...actualNotifications.notify, error: (message: unknown) => { toasts.push(String(message)); return 'toast' } },
  ),
}))
afterAll(() => { mock.module('../src/features/notifications', () => actualNotifications) })

type Call = { command: string; request: Record<string, unknown> }
let calls: Call[] = []
let marks: Record<string, ContactBubbleMarkRecord> = {}
let favorites: Array<Record<string, unknown>> = []

function host() {
  calls = []
  marks = {}
  favorites = []
  const handler = (command: string, args: unknown) => {
    const request = ((args as { request?: Record<string, unknown> } | undefined)?.request ?? {}) as Record<string, unknown>
    calls.push({ command, request })
    if (command === 'companion_quote_preview') {
      return { ok: true, data: { bubbleId: request.bubbleId, author: 'companion', summary: '华东机电的报价单没写税率。', deliveredAt: 1700000002000 } satisfies QuotePreview }
    }
    if (command === 'companion_bubble_favorite') {
      const id = String(request.bubbleId)
      const current = marks[id]
      const next: ContactBubbleMarkRecord = { bubbleId: id, favoritedAt: request.favorited ? (current?.favoritedAt ?? 1700000009000) : null, reaction: current?.reaction ?? null, reactionAt: current?.reactionAt ?? null }
      if (!next.favoritedAt && !next.reaction) delete marks[id]
      else marks[id] = next
      if (request.favorited === false) favorites = favorites.filter(entry => entry.bubbleId !== id)
      return { ok: true, data: next }
    }
    if (command === 'companion_bubble_reaction') {
      const id = String(request.bubbleId)
      const current = marks[id]
      const same = current?.reaction === request.reaction
      const next: ContactBubbleMarkRecord = { bubbleId: id, favoritedAt: current?.favoritedAt ?? null, reaction: same ? null : String(request.reaction), reactionAt: same ? null : 1700000009000 }
      if (!next.favoritedAt && !next.reaction) delete marks[id]
      else marks[id] = next
      return { ok: true, data: next }
    }
    if (command === 'companion_favorites_list') return { ok: true, data: favorites }
    if (command === 'companion_mark_unread') return { ok: true, data: contact }
    throw new Error('Unexpected command: ' + command)
  }
  answer = handler
  mockIPC(handler)
}

const context = (patch: Partial<ContactsContextValue> = {}): ContactsContextValue => ({
  current: contact, privateChat: true, list: { companions: [contact], pendingDeletions: [] }, projects: [], model: 'test',
  permission: 'ask', error: '', refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {},
  showProfile: () => {}, leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
  ...patch,
})

const renderChat = (snapshot: ConversationDetail, ctx = context()) => render(
  <ContactsContext.Provider value={ctx}>
    <AssistantUiChat conversation={{ enabled: true, detail: snapshot, streamingText: '' } as never} callbacks={{ onSubmit: () => {} }} />
  </ContactsContext.Provider>)

/** 气泡旁的「更多」菜单（原生 details）。用户气泡的身份是它的消息 id。 */
const bubbleSection = (container: HTMLElement, bubbleId: string) => {
  const section = container.querySelector(`[data-bubble-id="${bubbleId}"], [data-turn-message-id="${bubbleId}"]`)
  if (!section) throw new Error(`missing bubble ${bubbleId}`)
  return section as HTMLElement
}
const openMenu = (container: HTMLElement, bubbleId: string) => {
  const section = bubbleSection(container, bubbleId)
  fireEvent.click(section.querySelector('.fox-contact-bubble-more > summary') as HTMLElement)
  return section
}
const openReactions = (container: HTMLElement, bubbleId: string) => {
  const section = bubbleSection(container, bubbleId)
  fireEvent.click(section.querySelector('.fox-contact-reactions > summary') as HTMLElement)
  return section
}

afterEach(() => { cleanup(); clearMocks(); toasts.length = 0 })

describe('C07 永久标记只接受已落库气泡', () => {
  test('直播预览与乐观用户消息没有引用、收藏、表情入口，已落库气泡才有', async () => {
    host()
    // 运行中：助手回复仍是可替换预览（没有 contact_bubbles 行），用户消息是乐观发送。
    const live = detail({
      bubbles: [],
      messages: [{ ...userMessage, id: 'pending-message-1', status: 'sending' }, { ...answerMessage, status: 'streaming', kernelPreview: { checkpointSeq: 2, revision: 1 } }],
      lastRun: { ...detail().lastRun!, status: 'running' },
    })
    const view = renderChat(live)
    await waitFor(() => expect(view.container.querySelector('[data-contact-message-id]')).toBeTruthy())
    expect(view.container.querySelector('[data-bubble-id]')).toBeNull()
    expect(view.container.querySelector('.fox-contact-bubble-more')).toBeNull()
    expect(view.container.querySelector('.fox-contact-reactions')).toBeNull()
    expect(view.container.querySelectorAll('.fox-thread-copy-button').length).toBeGreaterThan(0)
    view.unmount()

    // 落库后：双方气泡都拿到稳定身份与三个入口。
    const settled = renderChat(detail())
    await waitFor(() => expect(settled.container.querySelector('[data-bubble-id="run:response:2"]')).toBeTruthy())
    expect(settled.container.querySelector('[data-bubble-id="run:response:2"]')!.getAttribute('data-delivery-order')).toBe('2')
    for (const id of ['user-1', 'run:response:2']) {
      const section = bubbleSection(settled.container, id)
      expect(section.querySelector('.fox-contact-bubble-more')).toBeTruthy()
      expect(section.querySelector('.fox-contact-reactions')).toBeTruthy()
    }
    // 纯附件消息没有气泡，也就没有永久入口。
    settled.unmount()
    const filesOnly = renderChat(detail({
      bubbles: [bubble('run:response:2', 'companion', 2, answerMessage.content)],
      messages: [{ ...userMessage, content: '  ' }, answerMessage],
    }))
    await waitFor(() => expect(filesOnly.container.querySelector('[data-bubble-id="run:response:2"]')).toBeTruthy())
    const plainUser = filesOnly.container.querySelector('[data-turn-message-id="user-1"]')
    expect(plainUser?.querySelector('.fox-contact-bubble-more') ?? null).toBeNull()
    expect(plainUser?.querySelector('.fox-contact-reactions') ?? null).toBeNull()
    expect(calls).toHaveLength(0)
  })

  test('引用、收藏、表情都发到 Fox 持久化，表情点同一个取消、点别的替换', async () => {
    host()
    const jumps: string[] = []
    const quoted: string[] = []
    const view = renderChat(detail(), context({ jumpToMessage: id => jumps.push(id), quoteBubble: async id => { quoted.push(id) } }))
    await waitFor(() => expect(view.container.querySelector('[data-bubble-id="run:response:2"]')).toBeTruthy())

    let section = openMenu(view.container, 'run:response:2')
    fireEvent.click(section.querySelector('.fox-contact-bubble-menu > button')!)
    expect(quoted).toEqual(['run:response:2'])

    section = openMenu(view.container, 'run:response:2')
    fireEvent.click([...section.querySelectorAll('.fox-contact-bubble-menu > button')].find(node => node.textContent?.includes('收藏'))!)
    await waitFor(() => expect(calls.some(call => call.command === 'companion_bubble_favorite')).toBe(true))
    expect(calls.at(-1)).toEqual({ command: 'companion_bubble_favorite', request: { companionId: 'contact', bubbleId: 'run:response:2', favorited: true } })
    await act(async () => {})
    section = openMenu(view.container, 'run:response:2')
    expect([...section.querySelectorAll('.fox-contact-bubble-menu > button')].some(node => node.textContent?.includes('取消收藏'))).toBe(true)

    section = openReactions(view.container, 'run:response:2')
    fireEvent.click(section.querySelector('.fox-contact-reaction-panel > button')!)
    await waitFor(() => expect(view.container.querySelector('.fox-contact-reaction-chip')?.textContent).toBe('👍'))
    expect(calls.at(-1)).toEqual({ command: 'companion_bubble_reaction', request: { companionId: 'contact', bubbleId: 'run:response:2', reaction: '👍' } })
    expect(view.container.querySelectorAll('.fox-contact-reaction-chip')).toHaveLength(1)

    // 换一个表情是替换，不是累加；收藏不受影响。
    section = openReactions(view.container, 'run:response:2')
    fireEvent.click([...section.querySelectorAll('.fox-contact-reaction-panel > button')].find(node => node.textContent === '😂')!)
    await waitFor(() => expect(view.container.querySelector('.fox-contact-reaction-chip')?.textContent).toBe('😂'))
    expect(view.container.querySelectorAll('.fox-contact-reaction-chip')).toHaveLength(1)
    expect(marks['run:response:2'].favoritedAt).toBeTruthy()

    // 再点同一个取消，芯片消失，收藏还在。
    section = openReactions(view.container, 'run:response:2')
    fireEvent.click([...section.querySelectorAll('.fox-contact-reaction-panel > button')].find(node => node.textContent === '😂')!)
    await waitFor(() => expect(view.container.querySelector('.fox-contact-reaction-chip')).toBeNull())
    expect(marks['run:response:2'].reaction).toBeNull()
    expect(marks['run:response:2'].favoritedAt).toBeTruthy()
    expect(jumps).toEqual([])
  })

  test('用户自己的消息同样可以引用、收藏与加表情', async () => {
    host()
    const quoted: string[] = []
    const view = renderChat(detail(), context({ quoteBubble: async id => { quoted.push(id) } }))
    await waitFor(() => expect(view.container.querySelector('[data-turn-message-id="user-1"]')).toBeTruthy())
    const section = openMenu(view.container, 'user-1')
    fireEvent.click(section.querySelector('.fox-contact-bubble-menu > button')!)
    expect(quoted).toEqual(['user-1'])
    fireEvent.click(openReactions(view.container, 'user-1').querySelector('.fox-contact-reaction-panel > button')!)
    await waitFor(() => expect(calls.some(call => call.command === 'companion_bubble_reaction' && call.request.bubbleId === 'user-1')).toBe(true))
    // 用户气泡不提供「标为未读」。
    expect(bubbleSection(view.container, 'user-1').querySelector('.fox-thread-unread-button')).toBeNull()
  })
})

describe('C07 引用区与回到原消息', () => {
  test('发出的消息带引用区，点它跳回原气泡；原消息不可用时保留摘要且不跳转', async () => {
    host()
    const jumps: string[] = []
    const ctx = context({ jumpToMessage: id => jumps.push(id) })
    const view = renderChat(detail({ messageQuotes: [quote()] }), ctx)
    await waitFor(() => expect(view.container.querySelector('.fox-contact-qref')).toBeTruthy())
    const ref = view.container.querySelector('.fox-contact-qref') as HTMLElement
    expect(ref.textContent).toContain('资料伙伴')
    expect(ref.textContent).toContain('华东机电的报价单没写税率')
    fireEvent.click(ref)
    expect(jumps).toEqual(['run:response:2'])
    view.unmount()

    const gone = renderChat(detail({ messageQuotes: [quote({ available: false })] }), context({ jumpToMessage: id => jumps.push(id) }))
    await waitFor(() => expect(gone.container.querySelector('.fox-contact-qref.is-gone')).toBeTruthy())
    const stale = gone.container.querySelector('.fox-contact-qref.is-gone') as HTMLElement
    expect(stale.textContent).toContain('原消息不可用')
    expect(stale.textContent).toContain('华东机电的报价单没写税率')
    fireEvent.click(stale)
    expect(jumps).toEqual(['run:response:2'])
    expect(stale.querySelector('button')).toBeNull()
  })

  test('引用草稿显示作者与摘要，取消后不再带入下一条消息', async () => {
    host()
    let draft: QuotePreview | null = { bubbleId: 'run:response:2', author: 'companion', summary: '华东机电的报价单没写税率。', deliveredAt: 1700000002000 }
    const cleared: number[] = []
    const ctx = () => context({ quoteDraft: draft, clearQuote: () => { cleared.push(1); draft = null } })
    const view = render(<ContactsContext.Provider value={ctx()}><ContactQuoteDraft /></ContactsContext.Provider>)
    expect(view.container.querySelector('.fox-contact-quote-draft')?.textContent).toContain('回复资料伙伴')
    expect(view.container.querySelector('.fox-contact-quote-draft')?.textContent).toContain('华东机电的报价单没写税率。')
    fireEvent.click(view.getByLabelText('取消引用'))
    expect(cleared).toEqual([1])
    view.rerender(<ContactsContext.Provider value={ctx()}><ContactQuoteDraft /></ContactsContext.Provider>)
    expect(view.container.querySelector('.fox-contact-quote-draft')).toBeNull()
    // 非私聊不显示引用草稿。
    const plain = render(<ContactsContext.Provider value={context({ privateChat: false, quoteDraft: { bubbleId: 'x', author: 'user', summary: 's', deliveredAt: 1 } })}><ContactQuoteDraft /></ContactsContext.Provider>)
    expect(plain.container.querySelector('.fox-contact-quote-draft')).toBeNull()
  })
})

describe('C07 收藏与重启', () => {
  test('重开后收藏与表情仍在，取消后重开不恢复', async () => {
    host()
    const stored: ContactBubbleMarkRecord[] = [
      { bubbleId: 'run:response:2', favoritedAt: 1700000009000, reaction: '🎉', reactionAt: 1700000009000 },
      { bubbleId: 'user-1', favoritedAt: 1700000008000, reaction: null, reactionAt: null },
    ]
    const view = renderChat(detail({ bubbleMarks: stored }))
    await waitFor(() => expect(view.container.querySelector('.fox-contact-reaction-chip')?.textContent).toBe('🎉'))
    expect(openMenu(view.container, 'run:response:2').textContent).toContain('取消收藏')
    expect(openMenu(view.container, 'user-1').textContent).toContain('取消收藏')
    view.unmount()

    // 取消收藏与表情后，Host 不再返回任何标记：重开也不恢复。
    const cancelled = renderChat(detail({ bubbleMarks: [] }))
    await waitFor(() => expect(cancelled.container.querySelector('[data-bubble-id="run:response:2"]')).toBeTruthy())
    expect(cancelled.container.querySelector('.fox-contact-reaction-chip')).toBeNull()
    expect(openMenu(cancelled.container, 'run:response:2').textContent).toContain('收藏')
    expect(openMenu(cancelled.container, 'run:response:2').textContent).not.toContain('取消收藏')
  })

  test('收藏页列出摘要与时间，可回到原消息或取消收藏，并说明不写笔记', async () => {
    host()
    favorites = [
      { bubbleId: 'run:response:2', author: 'companion', summary: '华东机电的报价单没写税率。', deliveredAt: 1700000002000, sourceOrdinal: 2, favoritedAt: 1700000009000 },
      { bubbleId: 'user-1', author: 'user', summary: '附件：三家报价单.zip（压缩包）', deliveredAt: 1700000000000, sourceOrdinal: 1, favoritedAt: 1700000008000 },
    ]
    const jumps: string[] = []
    const view = render(<ContactsContext.Provider value={context({ jumpToMessage: id => jumps.push(id) })}>
      <ContactFavorites contact={contact} onBack={() => {}} />
    </ContactsContext.Provider>)
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-favorite')).toHaveLength(2))
    expect(view.container.textContent).toContain('不会写进它的笔记')
    expect(view.container.textContent).toContain('附件：三家报价单.zip（压缩包）')
    expect(view.container.querySelector('.fox-contact-favorite time')?.getAttribute('datetime')).toBe(new Date(1700000002000).toISOString())
    fireEvent.click(view.getAllByText('回到消息')[0])
    expect(jumps).toEqual(['run:response:2'])
    fireEvent.click(view.getAllByLabelText('取消收藏')[0])
    await waitFor(() => expect(calls.some(call => call.command === 'companion_bubble_favorite' && call.request.favorited === false)).toBe(true))
    expect(calls.find(call => call.command === 'companion_bubble_favorite')?.request).toEqual({ companionId: 'contact', bubbleId: 'run:response:2', favorited: false })
    expect(calls.some(call => call.command.startsWith('companion_notes'))).toBe(false)
    // 取消的那条不再出现在收藏列表里（重开也不会回来）。
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-favorite')).toHaveLength(1))
    expect(view.container.textContent).not.toContain('华东机电的报价单没写税率。')
  })

  test('收藏页取消后，原气泡的菜单同步变回「收藏」', async () => {
    host()
    favorites = [
      { bubbleId: 'run:response:2', author: 'companion', summary: '华东机电的报价单没写税率。', deliveredAt: 1700000002000, sourceOrdinal: 2, favoritedAt: 1700000009000 },
    ]
    const snapshot = detail({ bubbleMarks: [{ bubbleId: 'run:response:2', favoritedAt: 1700000009000, reaction: null, reactionAt: null }] })
    // 与 workbench 相同的共享标记状态：气泡与收藏页读写同一份 Host 确认记录。
    function Harness() {
      const [updates, setUpdates] = useState<ReadonlyMap<string, ContactBubbleMarkRecord>>(new Map())
      const ctx = context({
        bubbleMarkUpdates: updates,
        publishBubbleMark: mark => setUpdates(current => new Map(current).set(mark.bubbleId, mark)),
      })
      return <ContactsContext.Provider value={ctx}>
        <AssistantUiChat conversation={{ enabled: true, detail: snapshot, streamingText: '' } as never} callbacks={{ onSubmit: () => {} }} />
        <ContactFavorites contact={contact} />
      </ContactsContext.Provider>
    }
    const view = render(<Harness />)
    await waitFor(() => expect(view.container.querySelector('[data-bubble-id="run:response:2"]')).toBeTruthy())
    expect(openMenu(view.container, 'run:response:2').textContent).toContain('取消收藏')
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-favorite')).toHaveLength(1))

    fireEvent.click(view.getByLabelText('取消收藏'))
    await waitFor(() => expect(calls.some(call => call.command === 'companion_bubble_favorite' && call.request.favorited === false)).toBe(true))
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-favorite')).toHaveLength(0))
    // 不重载会话，气泡菜单也立刻同步。
    await waitFor(() => expect(openMenu(view.container, 'run:response:2').textContent).not.toContain('取消收藏'))
    expect(openMenu(view.container, 'run:response:2').textContent).toContain('收藏')
  })
})

describe('C07 引用目标在分页后仍可定位', () => {
  test('原气泡还在更早的历史里：点引用区先翻页，再圈住同一条气泡', async () => {
    host()
    // 已发消息带着引用，但被引用的气泡不在当前窗口里。
    const window_ = detail({
      hasEarlierMessages: true,
      bubbles: [bubble('user-1', 'user', 5, userMessage.content)],
      messageQuotes: [quote({ bubbleId: 'run:response:2', sourceOrdinal: 2 })],
    })
    const paged = detail({
      hasEarlierMessages: false,
      messages: [{ ...answerMessage, ordinal: 2 }, userMessage],
      bubbles: [bubble('run:response:2', 'companion', 2, answerMessage.content), bubble('user-1', 'user', 5, userMessage.content)],
      messageQuotes: [quote({ bubbleId: 'run:response:2', sourceOrdinal: 2 })],
    })
    let loads = 0
    function Harness() {
      const [locate, setLocate] = useState<ContactLocateRequest | null>(null)
      const [snapshot, setSnapshot] = useState(window_)
      const ctx = context({ locateRequest: locate, jumpToMessage: id => setLocate({ id, key: Date.now() }) })
      return <ContactsContext.Provider value={ctx}>
        <ContactTopbar detail={snapshot} onReset={() => {}} loadEarlierMessages={async () => { loads += 1; setSnapshot(paged) }} />
        <AssistantUiChat conversation={{ enabled: true, detail: snapshot, streamingText: '' } as never} callbacks={{ onSubmit: () => {} }} />
      </ContactsContext.Provider>
    }
    const view = render(<Harness />)
    await waitFor(() => expect(view.container.querySelector('.fox-contact-qref')).toBeTruthy())
    expect(view.container.querySelector('[data-contact-message-id="run:response:2"]')).toBeNull()
    fireEvent.click(view.container.querySelector('.fox-contact-qref')!)
    await waitFor(() => expect(loads).toBe(1))
    await act(async () => {})
    await waitFor(() => expect(view.container.querySelector('[data-contact-message-id="run:response:2"]')?.classList.contains('is-search-hit')).toBe(true))
    // 定位用的是 K1 身份，不是位置：翻页后同一条气泡被圈住，阅读序号不变。
    expect(view.container.querySelector('[data-contact-message-id="run:response:2"]')?.getAttribute('data-delivery-order')).toBe('2')
  })
})

describe('C06 新建联系人当前会话三操作（无需重启）', () => {
  test('新建联系人完成回复后，标未读/引用/收藏直接成功，引用目标正确，未读位置由 Host 事实保持', async () => {
    host()
    const quoted: string[] = []
    let refreshed = 0
    // 新建联系人：阅读游标已在创建事务内初始化为 0，当前会话回复已落库。
    const fresh: Companion = { ...contact, lastReadDeliveryOrder: 0, readDeliveryOrder: 0 }
    const ctx = context({
      current: fresh, list: { companions: [fresh], pendingDeletions: [] },
      quoteBubble: async id => { quoted.push(id) }, refresh: async () => { refreshed += 1 },
    })
    const view = renderChat(detail(), ctx)
    await waitFor(() => expect(view.container.querySelector('[data-bubble-id="run:response:2"]')).toBeTruthy())

    // 标未读：发到 Host 的是 K1 气泡身份；成功后刷新，未读位置由 Host 持久事实决定。
    fireEvent.click(view.container.querySelector('.fox-thread-unread-button')!)
    await waitFor(() => expect(calls.some(call => call.command === 'companion_mark_unread')).toBe(true))
    expect(calls.find(call => call.command === 'companion_mark_unread')?.request)
      .toEqual({ companionId: 'contact', bubbleId: 'run:response:2' })
    await waitFor(() => expect(refreshed).toBeGreaterThan(0))
    expect(toasts).toHaveLength(0)

    // 引用：目标是同一条已落库气泡，不是位置相近的另一条。
    let section = openMenu(view.container, 'run:response:2')
    fireEvent.click(section.querySelector('.fox-contact-bubble-menu > button')!)
    expect(quoted).toEqual(['run:response:2'])

    // 收藏：Host 确认一次收藏事实，菜单同步为「取消收藏」，没有错误通知。
    section = openMenu(view.container, 'run:response:2')
    fireEvent.click([...section.querySelectorAll('.fox-contact-bubble-menu > button')].find(node => node.textContent?.includes('收藏'))!)
    await waitFor(() => expect(calls.some(call => call.command === 'companion_bubble_favorite')).toBe(true))
    expect(calls.at(-1)).toEqual({ command: 'companion_bubble_favorite', request: { companionId: 'contact', bubbleId: 'run:response:2', favorited: true } })
    await act(async () => {})
    expect(openMenu(view.container, 'run:response:2').textContent).toContain('取消收藏')
    expect(marks['run:response:2']?.favoritedAt).toBeTruthy()
    expect(toasts).toHaveLength(0)
  })

  test('后端历史仍在整理时三操作展示原始 BackfillPending 错误，不吞错冒充成功', async () => {
    const backfillMessage = '这个联系人的历史还在整理，稍后再标记'
    const handler = (command: string, args: unknown) => {
      const request = ((args as { request?: Record<string, unknown> } | undefined)?.request ?? {}) as Record<string, unknown>
      calls.push({ command, request })
      if (command === 'companion_mark_unread' || command === 'companion_bubble_favorite' || command === 'companion_quote_preview') {
        return { ok: false, error: { code: 'companion.backfill_pending', message: backfillMessage, retryable: true } }
      }
      if (command === 'companion_favorites_list') return { ok: true, data: [] }
      throw new Error('Unexpected command: ' + command)
    }
    calls = []; marks = {}; favorites = []; toasts.length = 0
    answer = handler
    mockIPC(handler)

    // quoteBubble 按 workbench 真实契约：失败展示错误且不设置引用草稿。
    const ctx = context({
      quoteBubble: async id => { try { await contacts.quotePreview('contact', id) } catch (error) { showContactError(error) } },
    })
    const view = render(
      <ContactsContext.Provider value={ctx}>
        <AssistantUiChat conversation={{ enabled: true, detail: detail(), streamingText: '' } as never} callbacks={{ onSubmit: () => {} }} />
        <ContactQuoteDraft />
      </ContactsContext.Provider>)
    await waitFor(() => expect(view.container.querySelector('[data-bubble-id="run:response:2"]')).toBeTruthy())

    // 收藏被拒：菜单仍是「收藏」，没有收藏事实，原始错误展示出来。
    let section = openMenu(view.container, 'run:response:2')
    fireEvent.click([...section.querySelectorAll('.fox-contact-bubble-menu > button')].find(node => node.textContent?.includes('收藏'))!)
    await waitFor(() => expect(toasts).toContain(backfillMessage))
    await act(async () => {})
    expect(openMenu(view.container, 'run:response:2').textContent).toContain('收藏')
    expect(openMenu(view.container, 'run:response:2').textContent).not.toContain('取消收藏')
    expect(marks['run:response:2']).toBeUndefined()

    // 标未读被拒：按钮不翻成「已标为未读」（不吞错冒充成功），原始错误展示出来。
    const unreadButton = view.container.querySelector('.fox-thread-unread-button') as HTMLElement
    fireEvent.click(unreadButton)
    await act(async () => {})
    await waitFor(() => expect(calls.some(call => call.command === 'companion_mark_unread')).toBe(true))
    expect(unreadButton.classList.contains('is-marked')).toBe(false)
    expect(unreadButton.getAttribute('aria-label')).toBe('标为未读')

    // 引用被拒：不产生引用草稿，原始错误展示出来。
    section = openMenu(view.container, 'run:response:2')
    fireEvent.click(section.querySelector('.fox-contact-bubble-menu > button')!)
    await waitFor(() => expect(calls.some(call => call.command === 'companion_quote_preview')).toBe(true))
    await act(async () => {})
    expect(view.container.querySelector('.fox-contact-quote-draft')).toBeNull()

    // 三个操作各自展示了 Host 的原始句子，而不是被前端改成「成功」。
    expect(toasts.filter(message => message === backfillMessage).length).toBe(3)
  })
})
