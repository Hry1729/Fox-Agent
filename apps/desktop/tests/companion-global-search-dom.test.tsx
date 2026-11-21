import { afterEach, describe, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
import { Sidebar } from '../src/features/chat/shell/fox-sidebar'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import { TooltipProvider } from '../src/components/ui/tooltip'
import type { Companion } from '../src/features/companions/api'
import type { GlobalSearchRecord } from '../src/features/conversations/model/types'

const contact: Companion = {
  id: 'c1', conversationId: 'chat-c1', templateAgentId: null, displayName: '资料伙伴', avatarRef: '📚', duty: '',
  personaOverride: null, workspaceDir: '/tmp/c1', workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, workShortcutOrder: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}
const context = {
  current: contact, privateChat: true, list: { companions: [contact], pendingDeletions: [] }, projects: [], model: 'test', permission: 'ask', error: '',
  refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
  leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
} as ContactsContextValue
const result = (kind: GlobalSearchRecord['kind'], id: string, title: string, subtitle: string): GlobalSearchRecord => ({
  id, kind, title, subtitle, workspaceView: kind === 'companion' ? 'companions' : kind === 'project' ? 'settings-projects' : 'chat',
  entityId: id, documentId: null, updatedAt: 1700000000000,
})
const noop = () => {}

afterEach(() => { cleanup(); window.localStorage.clear() })

describe('伙伴全局搜索', () => {
  test('伙伴分类固定第一，展示头像职责与命中高亮，点击打开指定私聊', async () => {
    const navigations: unknown[][] = []
    const view = render(<TooltipProvider><ContactsContext.Provider value={context}>
      <Sidebar
        collapsed={false} assistantMode="work" onModeChange={noop} onNewChat={noop}
        onNewProjectChat={noop} onAddProject={noop} onOpenConversation={noop}
        onRenameConversation={noop} onPinConversation={noop} onArchiveConversation={noop}
        onUnarchiveConversation={noop} onTrashConversation={noop} onRestoreConversation={noop}
        onPurgeConversation={noop} onDeleteProject={noop} onExitManagement={noop} onExpand={noop}
        onTheme={noop} onSettings={noop} newChatActive={false} activeView="chat"
        runtimeConversations={[]} onNavigate={(...args) => navigations.push(args)}
        onGlobalSearch={async () => [
          result('conversation', 'conversation', '资料整理对话', '普通对话'),
          result('project', 'project', '资料项目', '项目'),
          result('companion', 'c1', '资料伙伴', ''),
          result('companion', 'missing', '未加载的资料伙伴', '未加载职责'),
        ]}
      />
    </ContactsContext.Provider></TooltipProvider>)

    fireEvent.click(view.getByRole('button', { name: '搜索' }))
    fireEvent.change(view.getByPlaceholderText('搜索对话、伙伴、项目、资料、文件或任务'), { target: { value: '资料' } })
    const region = await waitFor(() => {
      const node = view.getByRole('region', { name: '全局搜索结果' })
      if (node.querySelectorAll(':scope > section').length !== 3) throw new Error('search results not ready')
      return node
    })
    expect([...region.querySelectorAll(':scope > section > header')].map(node => node.textContent)).toEqual(['伙伴2', '对话1', '项目1'])
    const companionSection = region.querySelector(':scope > section') as HTMLElement
    const [loaded, fallback] = within(companionSection).getAllByRole('button')
    expect(loaded.querySelector('.fox-contact-avatar')?.textContent).toBe('📚')
    expect(loaded.querySelector('small')?.textContent).toBe('伙伴')
    expect(fallback.querySelector('.fox-contact-avatar')?.textContent).toBe('未')
    expect(within(fallback).getByText('未加载职责')).toBeDefined()
    expect(region.querySelectorAll('mark').length).toBeGreaterThanOrEqual(3)
    expect([...region.querySelectorAll('mark')].every(node => node.textContent === '资料')).toBe(true)

    fireEvent.click(loaded)
    expect(navigations).toEqual([['companions', 'c1', undefined]])
    await waitFor(() => expect(view.queryByRole('dialog')).toBeNull())
  })
})
