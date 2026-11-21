import { afterEach, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { emit } from '@tauri-apps/api/event'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { useState } from 'react'
import { AssistantRuntimeProvider, useExternalStoreRuntime, type ThreadMessageLike } from '@assistant-ui/react'
import { TooltipProvider } from '../src/components/ui/tooltip'
import { MemoizedComposer } from '../src/features/chat/composer/fox-composer'
import { ComposerRuntimeContext } from '../src/features/chat/fox-shared'
import { COMPANION_ASSIGNMENTS_CHANGED, type Companion } from '../src/features/companions/api'
import { CoordinationTopbar, CoordinationClose } from '../src/features/companions/assignment-thread'
import { CompanionCommunicationProvider, CompanionCommunicationBefore } from '../src/features/companions/assignment-timeline'
import { CoordinationTranscript } from '../src/features/companions/coordination-transcript'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import type { CompanionAssignment, ConversationDetail } from '../src/features/conversations/model/types'

const recipient: Companion = {
  id: 'recipient', displayName: '数据伙伴', conversationId: 'recipient-private',
  templateAgentId: null, avatarRef: '数', duty: '分析数据', personaOverride: null, workspaceDir: 'D:/work',
  workspaceState: 'ready', workspaceError: null, isDefault: false, pinnedAt: null, workShortcutOrder: null,
  lastReadOrdinal: 0, modelProviderId: null, modelRef: null, reminderMode: 'follow_global', lastReadDeliveryOrder: null,
  manualUnreadFromOrder: null, manualUnreadAt: null, readDeliveryOrder: 0, pendingActionCount: 0,
  unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}
const record: CompanionAssignment = {
  id: 'assignment', rootRunId: 'root', sourceConversationId: 'work-chat', sourceRunId: 'run',
  sourceName: '月报对话', recipientConversationId: 'coordination-thread', recipientCompanionId: 'recipient',
  recipientName: '数据伙伴', title: '核对月报', doneWhen: '提供核对结果', status: 'queued', deliveryStatus: 'queued',
  createdAt: 1, updatedAt: 1,
}
const noop = () => {}
const ctx: ContactsContextValue = {
  current: undefined, privateChat: false, list: {
    companions: [recipient, { ...recipient, id: 'archived', displayName: '旧伙伴', archivedAt: 1 }],
    pendingDeletions: [],
  }, projects: [], model: '', permission: '', error: '',
  refresh: async () => {}, refreshConversation: async () => {},
  open: noop, showProfile: noop, leave: noop, setPermission: noop, openSharedMemory: noop, askToOrganize: noop,
  openCoordination: noop,
}

const emptyMessages: ThreadMessageLike[] = []
function CommunicationHarness({ conversationId }: { conversationId: string }) {
  const runtime = useExternalStoreRuntime<ThreadMessageLike>({ messages: emptyMessages, isRunning: false, onNew: async () => {}, convertMessage: message => message })
  return <AssistantRuntimeProvider runtime={runtime}><CompanionCommunicationProvider key={conversationId} conversationId={conversationId} oldestMessageAt={null}>
    <CompanionCommunicationBefore />
  </CompanionCommunicationProvider></AssistantRuntimeProvider>
}
const coordinationDetail = {
  conversation: { id: 'coordination-thread' }, coordination: record, messages: [], runtimeEvents: [], toolCalls: [],
  approvals: [], attachments: [], artifacts: [], lastRun: null, runs: [], hasEarlierMessages: false,
} as unknown as ConversationDetail

afterEach(() => { cleanup(); clearMocks() })

test('工作页可用 @ 选伙伴或文件，保留正文供 Host 识别', async () => {
  const sent: string[] = []
  function ComposerHarness() {
    const [draft, setDraft] = useState('')
    return <ContactsContext.Provider value={ctx}><TooltipProvider>
      <ComposerRuntimeContext.Provider value={{ knowledgeBindings: [], mentionFiles: ['月报.csv'] }}>
        <MemoizedComposer draft={draft} setDraft={setDraft} resetKey={0} chatState="idle" noticeBackend="fox"
          onSubmitPrompt={prompt => { sent.push(prompt); return 'complete' }} />
      </ComposerRuntimeContext.Provider>
    </TooltipProvider></ContactsContext.Provider>
  }
  const view = render(<ComposerHarness />)
  const input = view.getByRole('textbox') as HTMLTextAreaElement
  fireEvent.change(input, { target: { value: '@' } })
  expect(view.queryByText('旧伙伴')).toBeNull()
  expect(await view.findByText('月报.csv')).toBeDefined()
  fireEvent.keyDown(input, { key: 'Enter' })
  await waitFor(() => expect(input.value).toBe('@数据伙伴 '))
  expect(sent).toEqual([])
  fireEvent.change(input, { target: { value: '@数据伙伴 核对 @月' } })
  fireEvent.click(await view.findByText('月报.csv'))
  await waitFor(() => expect(input.value).toBe('@数据伙伴 核对 @月报.csv '))
  fireEvent.keyDown(input, { key: 'Enter' })
  await waitFor(() => expect(sent).toEqual(['@数据伙伴 核对 @月报.csv']))
})

test('双方入口打开同一气泡对话，补答留在发起对话，详情关闭后返回', async () => {
  let returned = record
  let submitted: unknown = null
  mockIPC((command, args) => {
    if (command === 'companion_assignments_list') return { ok: true, data: [returned] }
    if (command === 'companion_assignment_answer') {
      submitted = args
      returned = { ...returned, status: 'resume', question: null }
      return { ok: true, data: returned }
    }
    throw new Error(`Unexpected command: ${command}`)
  }, { shouldMockEvents: true })
  const destinations: unknown[][] = []
  const value = { ...ctx, openCoordination: (...args: unknown[]) => { destinations.push(args) } }
  const page = (id: string) => <ContactsContext.Provider value={value}>
    {id === 'coordination-thread' ? <><CoordinationTopbar thread={record} /><CoordinationTranscript detail={coordinationDetail} /><CoordinationClose thread={record} /></>
      : <CommunicationHarness conversationId={id} />}
  </ContactsContext.Provider>
  const view = render(page('work-chat'))
  fireEvent.click(await view.findByRole('button', { name: '等待发送给 数据伙伴，查看对话' }))
  returned = { ...record, status: 'needs_input', deliveryStatus: 'delivered', question: { prompt: '使用哪一份表格？', defaultAnswer: '本月原表' } }
  await act(() => emit(COMPANION_ASSIGNMENTS_CHANGED, { conversationIds: ['work-chat'] }))
  expect(await view.findByRole('textbox', { name: '你的回答' })).toBeDefined()
  view.rerender(page('recipient-private'))
  fireEvent.click(await view.findByRole('button', { name: '2 条消息往来： Fox 用户，查看对话' }))
  expect(destinations).toEqual([['coordination-thread'], ['coordination-thread']])
  expect(view.queryByRole('textbox')).toBeNull()
  view.rerender(page('work-chat'))
  fireEvent.click(await view.findByRole('button', { name: '采用默认回答' }))
  await waitFor(() => expect(submitted).toEqual({ request: { assignmentId: 'assignment', answer: '本月原表' } }))
  await waitFor(() => expect(view.queryByRole('textbox')).toBeNull(), { timeout: 1000 })
  returned = { ...returned, status: 'completed', instructions: '请核对月报中的三处差异。', result: '核对完成。三个差异已记录。' }
  view.rerender(page('coordination-thread'))
  expect(await view.findByText('请核对月报中的三处差异。')).toBeDefined()
  expect(await view.findByText('核对完成。三个差异已记录。')).toBeDefined()
  expect(view.container.querySelectorAll('.fox-coordination-bubble')).toHaveLength(2)
  expect(view.queryByRole('textbox')).toBeNull()
  expect(view.container.querySelector('summary')).toBeNull()
  fireEvent.click(view.getByRole('button', { name: '关闭伙伴对话' }))
  expect(destinations.at(-1)).toEqual(['work-chat', 'chat'])
})

test('发起对话无法自动继续时不展示内部错误', async () => {
  mockIPC(command => {
    if (command === 'companion_assignments_list') return { ok: true, data: [
      { ...record, status: 'completed', deliveryStatus: 'delivered', result: '数据核对完成。',
        resumeError: '已达到根任务执行次数上限。' },
    ] }
    throw new Error(`Unexpected command: ${command}`)
  }, { shouldMockEvents: true })
  const view = render(<ContactsContext.Provider value={ctx}>
    <CommunicationHarness conversationId="work-chat" />
  </ContactsContext.Provider>)
  expect(await view.findByRole('button', { name: '2 条消息往来： 数据伙伴，查看对话' })).toBeDefined()
  expect(view.queryByText('数据核对完成。')).toBeNull()
  expect(view.queryByText('完成标准 提供核对结果')).toBeNull()
  expect(view.queryByText(/已达到根任务执行次数上限/)).toBeNull()
})

test('工作页补答失败保留草稿，重试后继续任务', async () => {
  let returned: CompanionAssignment = { ...record, status: 'needs_input', question: { prompt: '使用哪种语气？' } }
  let reject: ((error: Error) => void) | undefined
  let attempts = 0
  mockIPC((command, args) => {
    if (command === 'companion_assignments_list') return { ok: true, data: [returned] }
    if (command === 'companion_assignment_answer') {
      ++attempts
      expect(args).toEqual({ request: { assignmentId: 'assignment', answer: '正式' } })
      if (attempts === 1) return new Promise((_resolve, fail) => { reject = fail })
      returned = { ...returned, status: 'resume', question: null }
      return { ok: true, data: returned }
    }
    throw new Error(`Unexpected command: ${command}`)
  }, { shouldMockEvents: true })
  const view = render(<ContactsContext.Provider value={ctx}>
    <CommunicationHarness conversationId="work-chat" />
  </ContactsContext.Provider>)
  const input = await view.findByRole('textbox', { name: '你的回答' })
  fireEvent.change(input, { target: { value: '正式' } })
  fireEvent.click(view.getByRole('button', { name: '提交回答' }))
  expect(view.queryByRole('button', { name: '取消协作' })).toBeNull()
  expect((input as HTMLTextAreaElement).disabled).toBe(true)
  reject?.(new Error('连接暂时中断'))
  await waitFor(() => expect((input as HTMLTextAreaElement).disabled).toBe(false))
  expect(view.queryByRole('alert')).toBeNull()
  expect((input as HTMLTextAreaElement).value).toBe('正式')
  await act(async () => { fireEvent.click(view.getByRole('button', { name: '提交回答' })) })
  await waitFor(() => expect(view.queryByRole('textbox')).toBeNull(), { timeout: 1000 })
  expect(view.queryByRole('textbox')).toBeNull()
  expect(view.queryByRole('button', { name: '取消协作' })).toBeNull()
})

for (const status of ['queued', 'needs_input'] as const) {
  test(`工作页 ${status} 协作不显示取消按钮`, async () => {
    mockIPC(command => {
      if (command === 'companion_assignments_list') return { ok: true, data: [{
        ...record, status, question: status === 'needs_input' ? { prompt: '使用哪种语气？', defaultAnswer: '正式' } : null,
      }] }
      throw new Error(`Unexpected command: ${command}`)
    }, { shouldMockEvents: true })
    const view = render(<ContactsContext.Provider value={ctx}>
      <CommunicationHarness conversationId="work-chat" />
    </ContactsContext.Provider>)
    await view.findByRole('button', { name: '等待发送给 数据伙伴，查看对话' })
    expect(view.queryByRole('button', { name: '取消协作' })).toBeNull()
    expect(Boolean(view.queryByRole('textbox', { name: '你的回答' }))).toBe(status === 'needs_input')
  })
}

test('协作详情可取消等待补答的任务，工作页仍只保留补答入口', async () => {
  let returned: CompanionAssignment = { ...record, status: 'needs_input', question: { prompt: '使用哪种语气？' } }
  let submitted: unknown = null
  mockIPC((command, args) => {
    if (command === 'companion_assignments_list') return { ok: true, data: [returned] }
    if (command === 'companion_assignment_cancel') {
      submitted = args
      returned = { ...returned, status: 'cancelled', question: null, result: '用户已取消本次协作' }
      return { ok: true, data: returned }
    }
    throw new Error(`Unexpected command: ${command}`)
  }, { shouldMockEvents: true })
  const view = render(<ContactsContext.Provider value={ctx}>
    <CoordinationTranscript detail={coordinationDetail} />
  </ContactsContext.Provider>)
  fireEvent.click(await view.findByRole('button', { name: '取消协作' }))
  await waitFor(() => expect(submitted).toEqual({ request: { assignmentId: 'assignment' } }))
  expect((await view.findByRole('status')).textContent).toBe('已取消')
  expect(view.queryByRole('textbox')).toBeNull()
  expect(view.queryByRole('button', { name: '取消协作' })).toBeNull()
})

test('已领取的运行和原生审批不显示补答或取消按钮', async () => {
  mockIPC(command => {
    if (command === 'companion_assignments_list') return { ok: true, data: [
      { ...record, recipientRunId: 'claimed-run' },
      { ...record, id: 'approval', status: 'needs_input', recipientRunId: 'approval-run', question: null },
    ] }
    throw new Error(`Unexpected command: ${command}`)
  }, { shouldMockEvents: true })
  const view = render(<ContactsContext.Provider value={ctx}>
    <CommunicationHarness conversationId="work-chat" />
  </ContactsContext.Provider>)
  await view.findAllByText('等待发送给')
  expect(view.queryByRole('textbox')).toBeNull()
  expect(view.queryByRole('button', { name: '取消协作' })).toBeNull()
})
