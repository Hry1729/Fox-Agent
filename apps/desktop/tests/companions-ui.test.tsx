import { describe, expect, test } from 'bun:test'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { SharedDecision } from '../src/features/companions/shared-decision'
import { renderToStaticMarkup } from 'react-dom/server'
import { ContactSidebar, ContactTopbar, ContactRepairBanner, AddContactAction } from '../src/features/companions/chat-controls'
import { ContactWriteCards, WriteCallsContext } from '../src/features/companions/chat-cards'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import type { Companion } from '../src/features/companions/api'
import type { AgentRecord, ConversationDetail, ToolCallRecord } from '../src/features/conversations/model/types'

const contact: Companion = {
  id: 'cmp_one', conversationId: 'chat', templateAgentId: 'expert', displayName: '当前联系人',
  avatarRef: '', duty: '', personaOverride: null, workspaceDir: 'C:/fox/workspace', workspaceState: 'ready',
  workspaceError: null, isDefault: false, pinnedAt: 1, lastReadOrdinal: 0, modelProviderId: null, modelRef: null, unreadCount: 2, activeRunStatus: null,
  archivedAt: null, deletingAt: null,
}
const archived = { ...contact, id: 'cmp_archived', conversationId: 'old', displayName: '已归档角色', archivedAt: 2 }
const noop = () => { }
const ctx: ContactsContextValue = {
  current: contact, privateChat: true, projects: [], model: 'model', permission: 'ask', setPermission: noop, openSharedMemory: noop, askToOrganize: noop,
  list: { companions: [contact, archived], pendingDeletions: [] }, refresh: async () => { }, error: '',
  open: noop, add: noop, showProfile: noop, leave: noop, refreshConversation: async () => { },
}
const render = (children: React.ReactNode) => renderToStaticMarkup(<ContactsContext.Provider value={ctx}>{children}</ContactsContext.Provider>)

describe('联系人桌面组件', () => {
  test('侧栏展示活跃身份、未读与选中会话，归档身份不占位置', () => {
    const html = render(<ContactSidebar activeId="chat" />)
    expect(html).toContain('当前联系人')
    expect(html).toContain('2 条未读')
    expect(html).toContain('aria-current="page"')
    expect(html).not.toContain('已归档角色')
  })
  test('本机专家按已有身份打开，并隐藏远端专家入口', () => {
    const expert = { id: 'expert', runtimeType: 'pi', agentKind: 'expert' } as AgentRecord
    expect(render(<AddContactAction agent={expert} />)).toContain('打开私聊')
    expect(render(<AddContactAction agent={{ ...expert, runtimeType: 'yuxi' }} />)).toBe('')
  })
  test('损坏判定来自 Host，仅受影响私聊显示修复', () => {
    const detail = { conversation: { id: 'chat' }, contextRepair: { available: false } } as ConversationDetail
    expect(render(<ContactRepairBanner detail={detail} />)).toBe('')
    expect(render(<ContactRepairBanner detail={{ ...detail, contextRepair: { available: true } }} />)).toContain('修复后继续')
  })
  test('主动重置只在 Host 开放时显示，运行期间禁用', () => {
    const detail = { contextReset: { supported: true, reason: null, code: null } } as ConversationDetail
    expect(render(<ContactTopbar detail={null} onReset={noop} />)).not.toContain('重置上下文')
    expect(render(<ContactTopbar detail={detail} onReset={noop} />)).toContain('重置上下文')
    const blocked = { ...detail, contextResetBlockedReason: '请先停止当前生成' }
    expect(render(<ContactTopbar detail={blocked} onReset={noop} />)).toContain('disabled')
  })
  test('写入卡等待持久化状态，忽略其他 Run 的工具', () => {
    const call = { id: 'a', toolName: 'memory_write', runId: 'run', result: { status: 'created', memoryId: 'new' } } as ToolCallRecord
    const cards = (items: ToolCallRecord[]) => render(<WriteCallsContext.Provider value={items}><ContactWriteCards runId="run" /></WriteCallsContext.Provider>)
    expect(cards([call])).toContain('已记住')
    expect(cards([call])).not.toContain('撤销')
    const existing = cards([{ ...call, result: { status: 'existing', memoryId: 'old' } }, { ...call, id: 'b', runId: 'other' }])
    expect(existing).toBe('')
    expect(existing).not.toContain('撤销')
  })
})
test('共享偏好默认新增，显式选择旧条目后才可替换', async () => {
  const element = document.createElement('div')
  document.body.appendChild(element)
  const root = createRoot(element)
  const choices: unknown[][] = []
  const request = { id: 'q', content: '我喜欢先看结论', evidenceExcerpt: '我喜欢先看结论', matches: [{ id: 'before', content: '我习惯阅读完整报告' }] }
  const button = (text: string) => Array.from(element.querySelectorAll('button')).find(b => b.textContent === text)!
  try {
    await act(async () => { root.render(<SharedDecision request={request} busy={false} onChoose={(...args) => choices.push(args)} />) })
    await act(async () => { button('记住这条').click() })
    expect(choices).toEqual([['add']])
    await act(async () => { button('替换…').click() })
    expect(button('确认替换').disabled).toBe(true)
    await act(async () => { element.querySelector<HTMLInputElement>('input[type=radio]')!.click() })
    await act(async () => { button('确认替换').click() })
    expect(choices).toEqual([['add'], ['replace', 'before']])
  } finally {
    await act(async () => { root.unmount() })
    element.remove()
  }
})
test('专题冲突保留编辑中的正文，删除请求按实际动作显示', async () => {
  const { mockIPC, clearMocks } = await import('@tauri-apps/api/mocks')
  const { ContactNotes } = await import('../src/features/companions/notes')
  const element = document.createElement('div')
  document.body.appendChild(element)
  const root = createRoot(element)
  const topic = { id: 'topic', title: '定价', contentSha256: 'first', revision: 1, aliases: [], content: '我的编辑' }
  mockIPC(cmd => {
    if (cmd === 'companion_notes_get') return { ok: true, data: { notes: [], usedChars: 0, limit: 4000, draft: '超额草稿', topics: [topic], moves: [], sharedRequests: [], requests: [{ id: 'q', action: 'remove', content: '原笔记', payload: { content: '原笔记' } }] } }
    if (cmd === 'companion_topic_read') return { ok: true, data: topic }
    if (cmd === 'companion_topic_save') return { ok: true, data: { status: 'conflict', current: { ...topic, content: '磁盘上的新内容', contentSha256: 'second', revision: 2 } } }
    throw Error('unexpected command: ' + cmd)
  })
  const button = (text: string) => Array.from(element.querySelectorAll('button')).find(b => b.textContent === text)!
  try {
    await act(async () => { root.render(<ContactNotes contact={contact} />) })
    expect(element.textContent).toContain('保存后 0 / 4,000 字')
    expect(button('请当前联系人帮我整理')).toBeDefined()
    expect(element.textContent).toContain('确认删除笔记')
    expect(element.textContent).not.toContain('确认改写笔记')
    await act(async () => { button('定价').click() })
    await act(async () => { button('保存专题').closest('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })) })
    expect(Array.from(element.querySelectorAll('textarea')).map(input => input.value)).toContain('我的编辑')
    expect(element.textContent).toContain('磁盘上的新内容')
    expect(element.textContent).toContain('second')
    await act(async () => { button('载入当前版本').click() })
    expect(Array.from(element.querySelectorAll('textarea')).map(input => input.value)).toContain('磁盘上的新内容')
  } finally {
    await act(async () => { root.unmount() })
    element.remove()
    clearMocks()
  }
})
