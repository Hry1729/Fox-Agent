import { afterEach, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
import { emit } from '@tauri-apps/api/event'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { COMPANION_TEAM_ACTIONS_CHANGED, type Companion, type CompanionTeamAction, type CompanionTeamDecision } from '../src/features/companions/api'
import { CompanionTeamActionsProvider, CompanionTeamCards } from '../src/features/companions/companion-team-cards'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'

const companion: Companion = {
  id: 'fox', conversationId: 'private-chat', templateAgentId: null, displayName: '小狐', avatarRef: '',
  duty: '', personaOverride: null, workspaceDir: 'D:/fox', workspaceState: 'ready', workspaceError: null,
  isDefault: true, pinnedAt: 1, workShortcutOrder: null, lastReadOrdinal: 0,
  modelProviderId: null, modelRef: null, reminderMode: 'follow_global', lastReadDeliveryOrder: null,
  manualUnreadFromOrder: null, manualUnreadAt: null, readDeliveryOrder: 0, pendingActionCount: 0,
  unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}
const action = (id: string, values: Partial<CompanionTeamAction>): CompanionTeamAction => ({
  id, companionId: 'fox', conversationId: 'private-chat', runId: 'run-1', targetId: 'target',
  targetName: '周报伙伴', kind: 'create', status: 'applied', createdAt: 1, updatedAt: 1, ...values,
})

function setup(
  initial: CompanionTeamAction[],
  resolve: (record: CompanionTeamAction, decision: CompanionTeamDecision) => CompanionTeamAction | { code: string; message: string },
  refreshError?: Error,
) {
  let records = initial
  let refreshed = 0
  const requests: unknown[] = []
  mockIPC((command, args) => {
    if (command === 'companion_team_actions_list') {
      expect(args).toEqual({ request: { companionId: 'fox' } })
      return { ok: true, data: records }
    }
    if (command === 'companion_team_action_resolve') {
      requests.push(args)
      const { actionId, decision } = (args as { request: { actionId: string; decision: CompanionTeamDecision } }).request
      const next = resolve(records.find(record => record.id === actionId)!, decision)
      if ('code' in next) return { ok: false, error: { ...next, retryable: false } }
      records = records.map(record => record.id === actionId ? next : record)
      return { ok: true, data: next }
    }
    throw new Error(`Unexpected command: ${command}`)
  }, { shouldMockEvents: true })
  const context: ContactsContextValue = {
    current: companion, privateChat: true, list: { companions: [companion], pendingDeletions: [] },
    projects: [], model: '', permission: '', error: '',
    refresh: async () => { refreshed++; if (refreshError) throw refreshError }, refreshConversation: async () => {},
    open: () => {}, showProfile: () => {}, leave: () => {}, setPermission: () => {},
    openSharedMemory: () => {}, askToOrganize: () => {},
  }
  const mount = () => render(<ContactsContext.Provider value={context}>
    <CompanionTeamActionsProvider>
      <CompanionTeamCards runId="run-1" />
    </CompanionTeamActionsProvider>
  </ContactsContext.Provider>)
  return {
    mount, requests, refreshCount: () => refreshed,
    publish: async (record: CompanionTeamAction) => {
      records = [...records, record]
      await act(() => emit(COMPANION_TEAM_ACTIONS_CHANGED, { companionId: 'fox' }))
    },
  }
}

afterEach(async () => {
  await act(async () => cleanup())
  clearMocks()
})

test('新建操作完成后列表刷新失败，卡片仍显示已生效状态', async () => {
  const harness = setup([action('profile', { kind: 'profile' })], record => record, new Error('伙伴列表读取失败'))
  const view = harness.mount()
  await view.findByRole('region', { name: '已修改资料「周报伙伴」' })
  await harness.publish(action('create', {}))
  const card = within(await view.findByRole('region', { name: '已新建「周报伙伴」' }))
  expect(card.getByRole('button', { name: '撤销' })).toBeDefined()
  expect(harness.refreshCount()).toBe(1)
})

test('已生效的操作显示前后值，收到新记录后可撤销并重开对话查看结果', async () => {
  const harness = setup([
    action('profile', { kind: 'profile', before: { duty: '整理资料' }, after: { duty: '整理周报' } }),
    action('other-run', { runId: 'run-2', targetName: '次要伙伴' }),
  ], record => ({ ...record, status: 'undone' }))
  const view = harness.mount()
  const profile = within(await view.findByRole('region', { name: '已修改资料「周报伙伴」' }))
  expect(profile.getByLabelText('修改前后的值').textContent).toContain('改前  整理资料\n改后  整理周报')
  expect(view.queryByText(/次要伙伴/)).toBeNull()
  await harness.publish(action('model', { kind: 'model', before: { modelRef: '旧模型' }, after: { modelRef: '新模型' } }))
  const model = within(await view.findByRole('region', { name: '已更换模型「周报伙伴」' }))
  expect(model.getByLabelText('修改前后的值').textContent).toContain('改前  旧模型\n改后  新模型')
  await harness.publish(action('create', { after: { name: '周报伙伴', duty: '整理周报' } }))
  const created = within(await view.findByRole('region', { name: '已新建「周报伙伴」' }))
  expect(created.getByLabelText('新伙伴资料').textContent).toContain('职责  整理周报')
  fireEvent.click(created.getByRole('button', { name: '撤销' }))
  expect((await created.findByRole('status')).textContent).toBe('已撤销')
  expect(harness.requests).toEqual([{ request: { actionId: 'create', decision: 'undo' } }])
  await waitFor(() => expect(harness.refreshCount()).toBeGreaterThan(0))
  view.unmount()
  const reopened = harness.mount()
  expect((await reopened.findByRole('status')).textContent).toBe('已撤销')
})

test('删除展示清单并等待确认，运行中失败后可重试，归档可以取消', async () => {
  let running = true
  const harness = setup([
    action('delete', {
      kind: 'delete', status: 'pending', preview: {
        planHash: 'delete-plan-hash',
        plan: { companionId: 'target', displayName: '周报伙伴', messages: 2, notes: 1, topics: 0,
          workspacePath: 'D:/weekly', workspaceFiles: 3, workspaceBytes: 128, sharedMemories: 1,
          generations: 0, grantedProjects: [], files: [] },
      },
    }),
    action('archive', { kind: 'archive', status: 'pending' }),
  ], (record, decision) => {
    if (record.kind === 'delete' && running) return { code: 'companion.run_active', message: 'busy' }
    return { ...record, status: decision === 'confirm' ? 'confirmed' : 'dismissed' }
  })
  const view = harness.mount()
  const deletion = within(await view.findByRole('region', { name: '要删除「周报伙伴」吗？' }))
  expect(deletion.getByText(/将删除 2 条消息/)).toBeDefined()
  expect(deletion.getByText(/工作目录里的 3 个文件/)).toBeDefined()
  expect(deletion.getByText('delete-plan-hash')).toBeDefined()
  expect(harness.requests).toEqual([])
  fireEvent.click(deletion.getByRole('button', { name: '永久删除' }))
  expect((await deletion.findByRole('alert')).textContent).toContain('等它停下后再试')
  running = false
  fireEvent.click(deletion.getByRole('button', { name: '永久删除' }))
  expect((await deletion.findByRole('status')).textContent).toBe('已确认删除')
  expect(deletion.queryByRole('button', { name: '永久删除' })).toBeNull()
  const archive = within(view.getByRole('region', { name: '要归档「周报伙伴」吗？' }))
  fireEvent.click(archive.getByRole('button', { name: '取消' }))
  expect((await archive.findByRole('status')).textContent).toBe('已取消')
  expect(harness.requests).toEqual([
    { request: { actionId: 'delete', decision: 'confirm' } },
    { request: { actionId: 'delete', decision: 'confirm' } },
    { request: { actionId: 'archive', decision: 'dismiss' } },
  ])
})

test('归档和临时伙伴删除取消后可重新提议，确认结果重开保持', async () => {
  const preview = {
    planHash: 'temporary-delete-plan',
    plan: { companionId: 'temporary', displayName: '临时伙伴', messages: 1, notes: 0, topics: 0,
      workspacePath: 'D:/temporary', workspaceFiles: 1, workspaceBytes: 8, sharedMemories: 0,
      generations: 0, grantedProjects: [], files: [] },
  }
  const harness = setup([
    action('archive-1', { kind: 'archive', status: 'pending' }),
    action('delete-1', { kind: 'delete', status: 'pending', targetId: 'temporary', targetName: '临时伙伴', preview }),
  ], (record, decision) => ({ ...record, status: decision === 'confirm' ? 'confirmed' : 'dismissed' }))
  const view = harness.mount()
  const archive = within(await view.findByRole('region', { name: '要归档「周报伙伴」吗？' }))
  fireEvent.click(archive.getByRole('button', { name: '取消' }))
  expect((await archive.findByRole('status')).textContent).toBe('已取消')
  const deletion = within(view.getByRole('region', { name: '要删除「临时伙伴」吗？' }))
  fireEvent.click(deletion.getByRole('button', { name: '取消' }))
  expect((await deletion.findByRole('status')).textContent).toBe('已取消')

  await harness.publish(action('archive-2', { kind: 'archive', status: 'pending', createdAt: 2, updatedAt: 2 }))
  await harness.publish(action('delete-2', { kind: 'delete', status: 'pending', targetId: 'temporary', targetName: '临时伙伴', preview, createdAt: 2, updatedAt: 2 }))
  const newArchive = within(await view.findByRole('region', { name: '要归档「周报伙伴」吗？' }))
  fireEvent.click(newArchive.getByRole('button', { name: '确认归档' }))
  expect((await newArchive.findByRole('status')).textContent).toBe('已确认归档')
  const newDeletion = within(view.getByRole('region', { name: '要删除「临时伙伴」吗？' }))
  fireEvent.click(newDeletion.getByRole('button', { name: '永久删除' }))
  expect((await newDeletion.findByRole('status')).textContent).toBe('已确认删除')
  expect(harness.requests).toEqual([
    { request: { actionId: 'archive-1', decision: 'dismiss' } },
    { request: { actionId: 'delete-1', decision: 'dismiss' } },
    { request: { actionId: 'archive-2', decision: 'confirm' } },
    { request: { actionId: 'delete-2', decision: 'confirm' } },
  ])

  view.unmount()
  const reopened = harness.mount()
  const restoredArchive = within(await reopened.findByRole('region', { name: '已归档「周报伙伴」' }))
  expect((await restoredArchive.findByRole('status')).textContent).toBe('已确认归档')
  expect(restoredArchive.queryByRole('button')).toBeNull()
  const restoredDeletion = within(reopened.getByRole('region', { name: '已删除「临时伙伴」' }))
  expect((await restoredDeletion.findByRole('status')).textContent).toBe('已确认删除')
  expect(restoredDeletion.queryByRole('button')).toBeNull()
})
