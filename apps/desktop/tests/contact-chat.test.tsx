import { afterEach, describe, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { renderToStaticMarkup } from 'react-dom/server'
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks'
import AssistantUiChat from '../src/features/chat/thread/fox-assistant-ui-chat'
import { deriveFoxThreadMessages } from '../src/features/chat/thread/convert-detail'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import { ContactWorkStatus } from '../src/features/companions/chat-status'
import { ContactQuestionCard } from '../src/features/companions/chat-question'
import { ContactWriteCards, WriteCallsContext } from '../src/features/companions/chat-cards'
import { contactReplies } from '../src/features/companions/chat-message-model'
import { RuntimeApprovalPrompt } from '../src/features/chat/components/RuntimeApprovalPrompt'
import { runtimeQuestionHistory } from '../src/features/conversations/model/pending-interactions'
import type { ApprovalRecord, ConversationDetail, ConversationMessage, RunEventRecord, ToolCallRecord } from '../src/features/conversations/model/types'

const context: ContactsContextValue = {
  current: { id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
    duty: '', personaOverride: null, workspaceDir: 'D:/contact', workspaceState: 'ready', workspaceError: null,
    isDefault: false, pinnedAt: null, lastReadOrdinal: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null },
  privateChat: true, list: { companions: [], pendingDeletions: [] }, projects: [], model: 'test', permission: 'ask', error: '',
  refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
  leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
}
const message = (checkpoint: number, content: string, patch: Partial<ConversationMessage> = {}): ConversationMessage => ({
  id: `kernel-message:run:${checkpoint}`, runId: 'run', conversationId: 'chat', role: 'assistant', kind: 'text',
  content, status: 'completed', ordinal: checkpoint, createdAt: 1700000000000 + checkpoint, updatedAt: 1700000000000 + checkpoint, ...patch,
})
const event = (seq: number, eventType: string, payload: Record<string, unknown> = {}): RunEventRecord => ({
  runId: 'run', seq, eventType, event: { type: eventType, ...payload }, createdAt: 1700000000000 + seq,
})
const detail = (patch: Partial<ConversationDetail> = {}): ConversationDetail => ({
  conversation: { id: 'chat', agentId: 'fox-general', agentName: 'Fox', title: '联系人', projectId: null, projectRoot: null,
    permissionMode: 'ask', status: 'active', createdAt: 0, updatedAt: 0, lastMessageAt: null },
  companionId: 'contact', messages: [], runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [],
  knowledgeBindings: [], expertBindings: [], hasEarlierMessages: false, goals: [], tasks: [], evidence: [],
  planRevisions: [], reviewFindings: [], acceptances: [], childRuns: [], expertWorkflow: null, expertTeam: null,
  lastRun: { id: 'run', conversationId: 'chat', runtimeSessionId: null, model: 'test', status: 'running', lastSeq: 1,
    errorCode: null, errorMessage: null, startedAt: 1700000000000, finishedAt: null }, ...patch,
})
const withContact = (children: React.ReactNode) => <ContactsContext.Provider value={context}>{children}</ContactsContext.Provider>
const renderChat = (snapshot: ConversationDetail) => withContact(<AssistantUiChat
  conversation={{ enabled: true, detail: snapshot, streamingText: snapshot.messages.at(-1)?.content ?? '' } as any}
  callbacks={{ onSubmit: () => {} }} />)

afterEach(() => { cleanup(); clearMocks() })

describe('联系人按条送达', () => {
  test('三条中途说明与最终回复各成气泡，流式预览和工具过程不进入聊天，历史分段相同', async () => {
    const texts = ['我先看一下资料。', '发现了两处问题。', '正在核对最后一项。', '已经处理好了。\n\n```ts\nconst ready = true\n```']
    let snapshot = detail({ runtimeEvents: [event(1, 'tool.started', { tool: 'read_file', toolCallId: 'tool', input: { path: 'secret-process.txt' }, kernelCheckpointSeq: '2' })] })
    const view = render(renderChat(snapshot))
    for (let index = 0; index < texts.length; index++) {
      const checkpoint = index * 2 + 2
      const preview = message(checkpoint, texts[index], { status: 'streaming', kernelPreview: { checkpointSeq: checkpoint, revision: 1 } })
      snapshot = { ...snapshot, messages: [...snapshot.messages, preview] }
      view.rerender(renderChat(snapshot))
      await act(async () => {})
      expect(view.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(index)
      snapshot = { ...snapshot, messages: snapshot.messages.map(item => item.id === preview.id ? { ...item, status: 'completed', kernelPreview: undefined } : item) }
      view.rerender(renderChat(snapshot))
      await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(index + 1))
      expect(view.container.querySelector('[data-streaming="true"]')).toBeNull()
      expect(view.container.textContent).not.toContain('secret-process.txt')
      expect(view.container.textContent).not.toContain('处理过程')
    }
    const ids = [...view.container.querySelectorAll('[data-contact-message-id]')].map(node => node.getAttribute('data-contact-message-id'))
    view.unmount()
    const reopened = render(renderChat({ ...snapshot, lastRun: { ...snapshot.lastRun!, status: 'completed' } }))
    await waitFor(() => expect(reopened.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(4))
    expect([...reopened.container.querySelectorAll('[data-contact-message-id]')].map(node => node.getAttribute('data-contact-message-id'))).toEqual(ids)
    expect(reopened.container.querySelectorAll('.fox-contact-bubble pre')).toHaveLength(1)
    expect(reopened.container.textContent).not.toContain('查看过程')
    expect(reopened.container.textContent).not.toContain('过程文件')
  })

  test('不按标点切分，不把内部思考变成气泡，重复内容保留真实消息数', () => {
    const saved = detail({ messages: [message(2, '<think>内部分析</think>正文。第二句。'), message(4, '正文。第二句。')] })
    const row = deriveFoxThreadMessages({ detail: saved })[0]
    expect(contactReplies(row, [])).toEqual([
      { id: 'kernel-message:run:2', text: '正文。第二句。', createdAt: 1700000000002 },
      { id: 'kernel-message:run:4', text: '正文。第二句。', createdAt: 1700000000004 },
    ])
  })

  test('切换联系人不会留下前一个联系人的气泡', async () => {
    const view = render(renderChat(detail({ messages: [message(2, '第一位的消息')] })))
    await waitFor(() => expect(view.container.textContent).toContain('第一位的消息'))
    const next = detail({ conversation: { ...detail().conversation, id: 'other-chat' }, messages: [message(2, '第二位的消息', { conversationId: 'other-chat', id: 'other-reply', runId: 'other-run' })], lastRun: null })
    view.rerender(<ContactsContext.Provider value={{ ...context, current: { ...context.current!, id: 'other', conversationId: 'other-chat' } }}>
      <AssistantUiChat key="other-chat" conversation={{ enabled: true, detail: next, streamingText: '' } as any} callbacks={{ onSubmit: () => {} }} />
    </ContactsContext.Provider>)
    await waitFor(() => expect(view.container.textContent).toContain('第二位的消息'))
    expect(view.container.textContent).not.toContain('第一位的消息')
    expect(view.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(1)
  })
})

test('工作条只显示当前联系人，终止后隐藏，等确认时头像不动', () => {
  const renderStatus = (snapshot: ConversationDetail | null, id = 'chat') => renderToStaticMarkup(withContact(<ContactWorkStatus detail={snapshot} conversationId={id} />))
  expect(renderStatus(detail())).toContain('📚')
  expect(renderStatus(detail())).toContain('data-working="true"')
  expect(renderStatus(detail(), 'other')).toBe('')
  for (const state of ['completed', 'failed', 'cancelled', 'interrupted']) {
    const snapshot = detail()
    expect(renderStatus({ ...snapshot, lastRun: { ...snapshot.lastRun!, status: state as any } })).toBe('')
  }
  const waiting = detail()
  const html = renderStatus({ ...waiting, lastRun: { ...waiting.lastRun!, status: 'waiting_approval' } })
  expect(html).toContain('等你确认')
  expect(html).toContain('data-working="false"')
})

test('聊天审批只呈现实际支持的选择，失败后可重试，历史保留授权范围', async () => {
  const approval: ApprovalRecord = { id: 'ticket', conversationId: 'chat', runId: 'run', toolCallId: 't', toolName: 'read_file', status: 'pending',
    requestedAction: '', request: { input: { path: 'D:/notes.md' }, availableDecisions: ['allow_once', 'deny'] }, decision: null, requestedAt: 1, resolvedAt: null }
  const calls: unknown[] = []
  let accepted = false
  const onResolve = async (id: string, decision: string) => { calls.push([id, decision]); return accepted }
  const view = render(withContact(<RuntimeApprovalPrompt approval={approval} onResolve={onResolve} />))
  expect([...view.container.querySelectorAll('.fox-contact-choice')].map(node => node.textContent)).toEqual(['A仅允许这一次', 'B拒绝'])
  fireEvent.click(view.getByText('仅允许这一次'))
  await waitFor(() => expect(view.getByRole('alert').textContent).toContain('重试'))
  accepted = true
  fireEvent.click(view.getByText('仅允许这一次'))
  await waitFor(() => expect(calls).toEqual([['ticket', 'allow_once'], ['ticket', 'allow_once']]))
  view.unmount()
  const reopened = render(withContact(<RuntimeApprovalPrompt approval={{ ...approval, status: 'approved', decision: { approved: true, scope: 'once' } }} onResolve={onResolve} />))
  expect(reopened.container.textContent).toContain('已选择：仅允许这一次')
  expect(reopened.container.querySelectorAll('.fox-contact-choice')).toHaveLength(0)
})

test('提问选项与自由回答互斥，真实历史事件恢复回答和所选项', async () => {
  const requested = event(4, 'user.question.requested', { questions: [{ question_id: 'q', question: '先整理哪一部分？', options: ['资料', '代码'], allow_other: true }] })
  const snapshot = detail({ runtimeEvents: [requested] })
  const request = runtimeQuestionHistory(snapshot)[0]
  const answers: unknown[] = []
  const view = render(<ContactQuestionCard request={request} onSubmit={async (...args) => { answers.push(args); return false }} />)
  fireEvent.click(view.getByText('资料'))
  fireEvent.change(view.getByRole('textbox'), { target: { value: '先核对附件' } })
  fireEvent.click(view.getByText('发送回答'))
  await waitFor(() => expect(answers).toEqual([['先整理哪一部分？：先核对附件', { q: '先核对附件' }]]))
  view.unmount()
  const saved = { ...snapshot, runtimeEvents: [requested, event(5, 'user.question.responded', { childRunId: 'answer-run' })],
    messages: [message(6, '先整理哪一部分？：代码', { id: 'user-answer', role: 'user', runId: 'answer-run' })] }
  const reopened = render(<ContactQuestionCard request={runtimeQuestionHistory(saved)[0]} />)
  expect(reopened.container.textContent).toContain('已收到你的回答')
  expect(reopened.getByText('代码').closest('button')?.getAttribute('aria-pressed')).toBe('true')
  expect(reopened.queryByRole('textbox')).toBeNull()
})

test('笔记确认重新挂载后使用持久化结果，不恢复已经处理的按钮', async () => {
  const call = { id: 'note-call', toolName: 'note_organize', runId: 'run', result: { status: 'pending', requestId: 'note-request' } } as ToolCallRecord
  const request = { id: 'note-request', memoryId: 'note', action: 'move', content: '每周核对资料', payload: { topic: '工作习惯' }, status: 'open', resolvedAt: null as number | null }
  mockIPC(command => {
    if (command === 'companion_notes_get') return { ok: true, data: { notes: [], topics: [], moves: [], usedChars: 0, limit: 4000, draft: null, sharedRequests: [],
      requests: request.status === 'open' ? [request] : [], requestHistory: [request], sharedRequestHistory: [] } }
    if (command === 'companion_note_request_resolve') { request.status = 'approved'; request.resolvedAt = 1; return { ok: true, data: { status: 'applied' } } }
    throw new Error('Unexpected command: ' + command)
  })
  const ui = () => withContact(<WriteCallsContext.Provider value={[call]}><ContactWriteCards runId="run" /></WriteCallsContext.Provider>)
  const view = render(ui())
  await waitFor(() => expect(view.getByText('确认移动')).toBeDefined())
  fireEvent.click(view.getByText('确认移动'))
  await waitFor(() => expect(view.getByText('已确认移动')).toBeDefined())
  view.unmount()
  const reopened = render(ui())
  await waitFor(() => expect(reopened.getByText('已确认移动')).toBeDefined())
  expect(reopened.getByText('确认移动').closest('button')?.disabled).toBe(true)
  expect(reopened.getByText('确认移动').closest('button')?.getAttribute('aria-pressed')).toBe('true')
})

test('撤销自动笔记后重开聊天仍显示已撤销', async () => {
  const call = { id: 'auto-note', toolName: 'memory_write', runId: 'run', result: { status: 'created', memoryId: 'note' } } as ToolCallRecord
  let status = 'created'
  mockIPC(command => {
    if (command === 'companion_notes_get') return { ok: true, data: { notes: [], topics: [], moves: [], requests: [], sharedRequests: [],
      writeReceipts: [{ memoryId: 'note', runId: 'run', status }] } }
    if (command === 'companion_memory_undo') { status = 'undone'; return { ok: true, data: { status: 'removed' } } }
    throw new Error('Unexpected command: ' + command)
  })
  const ui = () => withContact(<WriteCallsContext.Provider value={[call]}><ContactWriteCards runId="run" /></WriteCallsContext.Provider>)
  const view = render(ui())
  await waitFor(() => expect(view.getByRole('button', { name: '撤销' })).toBeDefined())
  fireEvent.click(view.getByRole('button', { name: '撤销' }))
  await waitFor(() => expect(view.getByText('已撤销')).toBeDefined())
  view.unmount()
  const reopened = render(ui())
  await waitFor(() => expect(reopened.getByText('已撤销')).toBeDefined())
  expect(reopened.queryByRole('button', { name: '撤销' })).toBeNull()
})
