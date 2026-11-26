import { afterEach, describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { act, cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
import { renderToStaticMarkup } from 'react-dom/server'
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks'
import AssistantUiChat from '../src/features/chat/thread/fox-assistant-ui-chat'
import { deriveFoxThreadMessages } from '../src/features/chat/thread/convert-detail'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import { ContactWorkStatus } from '../src/features/companions/chat-status'
import { ContactQuestionCard } from '../src/features/companions/chat-question'
import { ContactWriteCards, WriteCallsContext } from '../src/features/companions/chat-cards'
import { contactBubbleGuide, contactReplies } from '../src/features/companions/chat-message-model'
import { RuntimeApprovalPrompt } from '../src/features/chat/components/RuntimeApprovalPrompt'
import { runtimeQuestionHistory } from '../src/features/conversations/model/pending-interactions'
import { applyKernelModelPreview } from '../src/features/conversations/model/kernel-model-preview'
import { answerDeltaFingerprint } from '../src/features/conversations/model/runtime-delta-fingerprint'
import { withMonotonicInvalidation } from '../src/features/conversations/model/approval-labels'
import type { CompanionTeamAction } from '../src/features/companions/api'
import type { ApprovalRecord, ArtifactRecord, ConversationDetail, ConversationMessage, RunEventRecord, ToolCallRecord } from '../src/features/conversations/model/types'

const context: ContactsContextValue = {
  current: { id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
    duty: '', personaOverride: null, workspaceDir: 'D:/contact', workspaceState: 'ready', workspaceError: null,
    isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null },
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
const withContact = (children: React.ReactNode, value: ContactsContextValue = context) => <ContactsContext.Provider value={value}>{children}</ContactsContext.Provider>
const renderChat = (snapshot: ConversationDetail, value: ContactsContextValue = context, onOpenArtifact?: (artifact: ArtifactRecord) => void) => withContact(<AssistantUiChat
  conversation={{ enabled: true, detail: snapshot, streamingText: snapshot.messages.at(-1)?.content ?? '' } as any}
  callbacks={{ onSubmit: () => {} }} onOpenArtifact={onOpenArtifact} />, value)

afterEach(() => { cleanup(); clearMocks() })

describe('聊天边缘渐变', () => {
  const contactCss = readFileSync(new URL('../src/features/companions/contact-chat.css', import.meta.url), 'utf8')
  const workbenchCss = readFileSync(new URL('../src/styles/workbench.css', import.meta.url), 'utf8')
  const rule = (css: string, selector: string) => {
    const start = css.indexOf(`\n${selector} {`)
    expect(start).toBeGreaterThanOrEqual(0)
    return css.slice(start + 1, css.indexOf('}', start) + 1)
  }

  test('名称与搜索悬浮在聊天卡片顶端，渐变从卡片边缘开始', () => {
    const header = rule(contactCss, '.fox-chat-topbar.fox-contact-topbar')
    expect(header).toContain('position: absolute;')
    expect(header).toContain('inset: 0 0 auto;')
    expect(header).toContain('margin: 0;')
    expect(header).toContain('pointer-events: none;')
    expect(rule(contactCss, '.fox-chat-topbar.fox-contact-topbar > *')).toContain('pointer-events: auto;')
    const fade = rule(contactCss, '.fox-chat-topbar.fox-contact-topbar::before')
    expect(fade).toContain('inset: 0 var(--fox-scrollbar-size, 6px) 0 0;')
    expect(fade).toContain('linear-gradient(to bottom, var(--fox-canvas), transparent)')
    expect(contactCss).not.toContain('mask-image:')
    expect(rule(contactCss, '.is-contact-chat .fox-thread-viewport')).toContain('padding-top: calc(var(--fox-contact-header-height, 60px) + var(--fox-space-xl));')
  })

  test('私聊与普通对话底部遮挡留出滚动条，渐变末端之后保持实色', () => {
    const fade = rule(workbenchCss, '.fox-chat-stage:has(> .fox-composer-wrap:not(.is-empty))::after')
    expect(fade).toContain('inset: auto 6px 0 0;')
    expect(fade).toContain('height: calc(var(--fox-composer-occlusion, 180px) - 6px + 42px);')
    expect(fade).toContain('linear-gradient(to bottom, transparent, var(--fox-canvas) 42px)')
    expect(fade).toContain('pointer-events: none;')
    const fadeZ = Number(fade.match(/z-index: (\d+)/)?.[1])
    const dockZ = Number(rule(workbenchCss, '.fox-conversation-bottom-dock').match(/z-index: (\d+)/)?.[1])
    const composerZ = Number(rule(workbenchCss, '.fox-composer-wrap').match(/z-index: (\d+)/)?.[1])
    expect(fadeZ).toBeLessThan(dockZ)
    expect(dockZ).toBeLessThan(composerZ)
    expect(workbenchCss).not.toContain('.fox-composer-wrap:not(.is-empty)::before')
  })
})

describe('联系人按条送达', () => {
  test('Kernel 生成正文时保留已有气泡和审批，提交后才显示下一条', async () => {
    const approval: ApprovalRecord = {
      id: 'approval-order', conversationId: 'chat', runId: 'run', toolCallId: 'write',
      toolName: 'write_file', status: 'pending', requestedAction: '写入测试文档',
      request: { summary: '写入测试文档' } as ApprovalRecord['request'],
      decision: null, requestedAt: 1700000000003, resolvedAt: null,
    }
    const before = message(2, '我会先核对材料。')
    let snapshot = detail({
      messages: [before], approvals: [approval],
      kernelSnapshot: { schemaVersion: 1, runId: 'run', turnId: 'turn', state: 'running', lastEventSeq: '6', terminalWritten: false },
    })
    const view = render(renderChat(snapshot))
    const first = await view.findByText(before.content)
    const firstBubble = first.closest('.fox-contact-message')
    await waitFor(() => expect(view.container.querySelector('.fox-contact-decision-entry')).not.toBeNull())
    const card = view.container.querySelector('.fox-contact-decision-entry')!
    const approved = { ...approval, status: 'approved' as const, resolvedAt: 1700000000004 }
    snapshot = { ...snapshot, approvals: [approved] }
    const preview = (revision: number, text: string) => applyKernelModelPreview(snapshot, {
      schemaVersion: 1, runId: 'run', conversationId: 'chat', turnId: 'turn',
      checkpointSeq: 5, revision, text,
    }, 1700000000005 + revision)!
    for (const [revision, text] of ['已写好', '已写好测试文档。'].entries()) {
      snapshot = preview(revision + 1, text)
      view.rerender(renderChat(snapshot))
      expect(view.queryByText(text)).toBeNull()
      expect(view.getByText(before.content).closest('.fox-contact-message') === firstBubble).toBe(true)
      expect(view.container.querySelector('.fox-contact-decision-entry') === card).toBe(true)
    }
    const settled = detail({
      lastRun: { ...detail().lastRun!, status: 'completed' },
      approvals: [approved],
      messages: [before, message(5, '已写好测试文档。')],
      bubbles: [before, message(5, '已写好测试文档。')].map((reply, index) => ({
        id: `run:kernel:${reply.ordinal}:response`, conversationId: 'chat', runId: 'run',
        messageId: reply.id, sourceOrdinal: reply.ordinal, author: 'companion', projection: 'kernel',
        segmentStart: reply.ordinal, segmentEnd: reply.ordinal, deliveryOrder: index + 1,
        deliveredAt: reply.createdAt, content: reply.content, fingerprint: '', metadata: {},
      })),
    })
    view.rerender(renderChat(settled))
    await waitFor(() => expect(view.getByText('已写好测试文档。').closest('.fox-contact-message')?.getAttribute('data-delivery-order')).toBe('2'))
    expect(view.getByText(before.content).closest('.fox-contact-message') === firstBubble).toBe(true)
    expect(view.container.querySelector('.fox-contact-decision-entry') === card).toBe(true)
    expect(card.compareDocumentPosition(view.getByText('已写好测试文档。')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    view.unmount()
    const reopened = render(renderChat(settled))
    const reopenedLast = await reopened.findByText('已写好测试文档。')
    expect(reopened.container.querySelector('.fox-contact-decision-entry')!.compareDocumentPosition(reopenedLast)
      & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  test('小狐的操作卡出现在对应回合，无助手回复时仍显示待确认操作', async () => {
    const item: CompanionTeamAction = {
      id: 'create', companionId: 'contact', conversationId: 'chat', runId: 'run', targetId: 'new',
      targetName: '周报伙伴', kind: 'create', status: 'applied', createdAt: 1, updatedAt: 1,
    }
    mockIPC(command => {
      if (command === 'companion_team_actions_list') return { ok: true, data: [
        item,
        { ...item, id: 'archive', runId: 'other-run', kind: 'archive', status: 'pending' },
        { ...item, id: 'background', runId: 'background-run', targetName: '定时报表伙伴', createdAt: 1700000000002 },
        { ...item, id: 'history', runId: 'old-run', targetName: '旧报表伙伴' },
      ] }
      throw new Error(`Unexpected command: ${command}`)
    }, { shouldMockEvents: true })
    const ctx = { ...context, current: { ...context.current!, isDefault: true } }
    const view = render(renderChat(detail({
      messages: [message(1, '已创建周报伙伴。')],
      hasEarlierMessages: true,
    }), ctx))
    const creation = await view.findByRole('region', { name: '已新建「周报伙伴」' })
    expect(creation.closest('[data-turn-message-id]')?.getAttribute('data-turn-message-id')).toBe('kernel-message:run:1')
    const archive = await view.findByRole('region', { name: '要归档「周报伙伴」吗？' })
    expect(archive.closest('[data-turn-message-id]')).toBeNull()
    const background = await view.findByRole('region', { name: '已新建「定时报表伙伴」' })
    expect(background.closest('[data-turn-message-id]')).toBeNull()
    expect(view.queryByRole('region', { name: '已新建「旧报表伙伴」' })).toBeNull()
    expect(view.getAllByRole('region', { name: '已新建「周报伙伴」' })).toHaveLength(1)
  })

  test('创建回合从流式检查点落库后仍保留消息和操作卡，撤销结果重开可见', async () => {
    let records: CompanionTeamAction[] = [{
      id: 'm3-create', companionId: 'contact', conversationId: 'chat', runId: 'run', targetId: 'new-peer',
      targetName: '复测核对员', kind: 'create', status: 'applied', createdAt: 1700000000003, updatedAt: 1700000000003,
      after: { name: '复测核对员', duty: '核对数字' },
    }]
    mockIPC((command, args) => {
      if (command === 'companion_team_actions_list') return { ok: true, data: records }
      if (command === 'companion_team_action_resolve') {
        expect(args).toEqual({ request: { actionId: 'm3-create', decision: 'undo' } })
        records = records.map(record => ({ ...record, status: 'undone' }))
        return { ok: true, data: records[0] }
      }
      throw new Error(`Unexpected command: ${command}`)
    }, { shouldMockEvents: true })
    const ctx = { ...context, current: { ...context.current!, isDefault: true } }
    const user = message(1, '新建一个叫“复测核对员”的伙伴，职责是核对数字。', { id: 'm3-user', role: 'user' })
    const streaming = detail({
      messages: [user, message(2, '我来创建这位伙伴。')],
      kernelSnapshot: { schemaVersion: 1, runId: 'run', turnId: 'turn', state: 'running', lastEventSeq: '6', terminalWritten: false },
    })
    const view = render(renderChat(streaming, ctx))
    expect(await view.findByText(user.content)).toBeDefined()
    const creation = await view.findByRole('region', { name: '已新建「复测核对员」' })
    const settled = detail({
      lastRun: { ...detail().lastRun!, status: 'completed' },
      messages: [user, message(3, '我来创建这位伙伴。已创建成功。', { id: 'assistant-run' })],
    })
    view.rerender(renderChat(settled, ctx))
    expect(await view.findByText('我来创建这位伙伴。已创建成功。')).toBeDefined()
    expect(view.getByText(user.content)).toBeDefined()
    expect(view.getByRole('region', { name: '已新建「复测核对员」' })).toBe(creation)
    fireEvent.click(within(creation).getByRole('button', { name: '撤销' }))
    expect((await within(creation).findByRole('status')).textContent).toBe('已撤销')
    view.unmount()
    const reopened = render(renderChat(settled, ctx))
    expect(await reopened.findByText(user.content)).toBeDefined()
    const restored = within(await reopened.findByRole('region', { name: '已新建「复测核对员」' }))
    expect((await restored.findByRole('status')).textContent).toBe('已撤销')
    expect(restored.queryByRole('button', { name: '撤销' })).toBeNull()
  })

  test('没有运行记录的问候使用持久气泡身份', async () => {
    const greeting = message(1, '你好，我是资料伙伴。', { id: 'greeting', runId: null })
    const snapshot = detail({ messages: [greeting], lastRun: null, bubbles: [{
      id: 'greeting', conversationId: 'chat', runId: null, messageId: 'greeting', sourceOrdinal: 1,
      author: 'companion', projection: 'message', segmentStart: null, segmentEnd: null,
      deliveryOrder: 1000, deliveredAt: greeting.createdAt, content: greeting.content, fingerprint: 'f', metadata: {},
    }] })
    const view = render(renderChat(snapshot))
    await act(async () => {})
    expect(view.container.querySelector('[data-delivery-order="1000"]')?.textContent).toContain('你好，我是资料伙伴。')
  })

  test('问候引导提交后保留题目和原选项，重开对话仍能看到', async () => {
    const guide = {
      title: '先给我定个方向',
      options: [
        { label: '整理文件和资料', message: '以后主要帮我整理文件和资料' },
        { label: '写材料和汇报', message: '以后主要帮我写材料和汇报' },
        { label: '定期提醒和跟进', message: '以后主要帮我做定期提醒和跟进' },
        { label: '处理表格和数据', message: '以后主要帮我处理表格和数据' },
      ],
    }
    const greeting = message(1, '你好，我是资料伙伴。', { id: 'greeting', runId: null })
    const greetingBubble = {
      id: 'greeting', conversationId: 'chat', runId: null, messageId: 'greeting', sourceOrdinal: 1,
      author: 'companion' as const, projection: 'message', segmentStart: null, segmentEnd: null,
      deliveryOrder: 1000, deliveredAt: greeting.createdAt, content: greeting.content, fingerprint: 'f', metadata: { companionGuide: guide },
    }
    const sent: string[] = []
    const ctx = { ...context, sendPrompt: async (text: string) => { sent.push(text) } }
    const view = render(renderChat(detail({ messages: [greeting], lastRun: null, bubbles: [greetingBubble] }), ctx))
    const option = await view.findByRole('button', { name: '写材料和汇报' })
    expect(view.container.querySelectorAll('.fox-companion-guide button')).toHaveLength(4)
    fireEvent.click(option)
    await waitFor(() => expect(sent).toEqual(['以后主要帮我写材料和汇报']))
    const user = message(2, sent[0], { id: 'user-guide', runId: null, role: 'user' })
    const settled = detail({ messages: [greeting, user], lastRun: null, bubbles: [greetingBubble] })
    view.rerender(renderChat(settled, ctx))
    const receipt = within(await view.findByRole('region', { name: guide.title }))
    expect(receipt.getByText('写材料和汇报')).toBeDefined()
    expect(receipt.getByText('B')).toBeDefined()
    expect(receipt.queryByRole('button')).toBeNull()
    expect(receipt.queryByText('整理文件和资料')).toBeNull()

    view.unmount()
    const laterUser = message(3, guide.options[0].message, { id: 'later-user', runId: null, role: 'user' })
    const reopened = render(renderChat({ ...settled, messages: [...settled.messages, laterUser] }, ctx))
    const restored = within(await reopened.findByRole('region', { name: guide.title }))
    expect(restored.getByText('写材料和汇报')).toBeDefined()
    expect(restored.queryByText('整理文件和资料')).toBeNull()
  })

  test('引导元数据拒绝畸形选项并限制为四项', () => {
    const options = Array.from({ length: 5 }, (_, index) => ({ label: `方向 ${index}`, message: `消息 ${index}` }))
    const bubble = { metadata: { companionGuide: { title: '方向', options: [...options, { label: ' ', message: '忽略' }] } } } as Parameters<typeof contactBubbleGuide>[0]
    expect(contactBubbleGuide(bubble)?.options).toHaveLength(4)
    expect(contactBubbleGuide({ metadata: { companionGuide: { title: '方向', options: [{ label: '空', message: '' }] } } } as Parameters<typeof contactBubbleGuide>[0])).toBeUndefined()
    expect(contactBubbleGuide({ metadata: {} } as Parameters<typeof contactBubbleGuide>[0])).toBeUndefined()
  })

  test('三条已提交的中途说明和最终回复各显示一次，流式预览与工具过程不进入聊天', async () => {
    const texts = ['我先看一下资料。', '发现了两处问题。', '正在核对最后一项。', '已经处理好了。\n\n```ts\nconst ready = true\n```']
    let snapshot = detail({ runtimeEvents: [event(1, 'tool.started', { tool: 'read_file', toolCallId: 'tool', input: { path: 'secret-process.txt' }, kernelCheckpointSeq: '2' })] })
    const view = render(renderChat(snapshot))
    for (let index = 0; index < texts.length; index++) {
      const checkpoint = index * 2 + 2
      const preview = message(checkpoint, texts[index], { status: 'streaming', kernelPreview: { checkpointSeq: checkpoint, revision: 1 } })
      snapshot = { ...snapshot, messages: [...snapshot.messages, preview] }
      view.rerender(renderChat(snapshot))
      await act(async () => {})
      expect(view.container.querySelectorAll('[data-contact-message-id]')).toHaveLength(index)
      snapshot = { ...snapshot, messages: snapshot.messages.map(item => item.id === preview.id ? { ...item, status: 'completed', kernelPreview: undefined } : item) }
      view.rerender(renderChat(snapshot))
      await waitFor(() => expect(view.container.querySelectorAll('[data-contact-message-id]')).toHaveLength(index + 1))
      await view.findByText(texts[index].split('\n')[0])
      expect([...view.container.querySelectorAll('[data-contact-message-id]')].at(-1)?.getAttribute('data-contact-message-id')).toBe(`run:kernel:${checkpoint}:response`)
      expect(view.container.querySelector('[data-streaming="true"]')).toBeNull()
      expect(view.container.textContent).not.toContain('secret-process.txt')
      expect(view.container.textContent).not.toContain('处理过程')
    }
    const ids = [...view.container.querySelectorAll('[data-contact-message-id]')].map(node => node.getAttribute('data-contact-message-id'))
    view.unmount()
    const reopened = render(renderChat({ ...snapshot, lastRun: { ...snapshot.lastRun!, status: 'completed' } }))
    await waitFor(() => expect(reopened.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(5))
    expect([...reopened.container.querySelectorAll('[data-contact-message-id]')].map(node => node.getAttribute('data-contact-message-id'))).toEqual(ids)
    expect(reopened.container.querySelectorAll('.fox-contact-bubble pre')).toHaveLength(1)
    expect(reopened.container.textContent).not.toContain('查看过程')
    expect(reopened.container.textContent).not.toContain('过程文件')
  })

  test('已落库的气泡身份优先于按页重算，重开后仍是同一批气泡', async () => {
    const bubble = (id: string, content: string, order: number) => ({
      id, conversationId: 'chat', runId: 'run', messageId: 'kernel-message:run:2', sourceOrdinal: 2,
      author: 'companion' as const, projection: 'segmented' as const, segmentStart: order, segmentEnd: order,
      deliveryOrder: order, deliveredAt: 1700000000000 + order, content, fingerprint: '',
    })
    const settled = detail({
      lastRun: { ...detail().lastRun!, status: 'completed' },
      messages: [message(2, '第一条第二条')],
      bubbles: [bubble('run:response:3', '第一条', 3), bubble('run:response:5', '第二条', 5)],
    })
    const view = render(renderChat(settled))
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(2))
    expect([...view.container.querySelectorAll('[data-contact-message-id]')].map(node => node.getAttribute('data-contact-message-id')))
      .toEqual(['run:response:3', 'run:response:5'])
    expect(view.container.textContent).toContain('第一条')
    view.unmount()
    const reopened = render(renderChat(settled))
    await waitFor(() => expect(reopened.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(2))
    expect([...reopened.container.querySelectorAll('[data-contact-message-id]')].map(node => node.getAttribute('data-contact-message-id')))
      .toEqual(['run:response:3', 'run:response:5'])
  })

  test('运行中只显示已落库气泡并保留其阅读序号', async () => {
    const committed = {
      id: 'run:kernel:2:response', conversationId: 'chat', runId: 'run', messageId: 'kernel-message:run:2', sourceOrdinal: 2,
      author: 'companion' as const, projection: 'message' as const, segmentStart: null, segmentEnd: null,
      deliveryOrder: 3000, deliveredAt: 1700000000002, content: '第一条已落库', fingerprint: '',
    }
    const snapshot = detail({
      messages: [
        message(2, '第一条已落库'),
        message(4, '正在输入…', { status: 'streaming', kernelPreview: { checkpointSeq: 4, revision: 1 } }),
      ],
      bubbles: [committed],
    })
    const view = render(renderChat(snapshot))
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-bubble')).toHaveLength(1))
    const sections = [...view.container.querySelectorAll('[data-contact-message-id]')]
    expect(sections[0].getAttribute('data-contact-message-id')).toBe('run:kernel:2:response')
    expect(sections[0].getAttribute('data-delivery-order')).toBe('3000')
    expect(sections[0].getAttribute('data-bubble-id')).toBe('run:kernel:2:response')
    expect(view.queryByText('正在输入…')).toBeNull()
  })

  test('Pi 在生成期间隐藏所有正文，提交后一次显示并保留既有回复', async () => {
    const answer = (seq: number, text: string) => event(seq, 'message.delta', {
      deltaLength: text.length, deltaFingerprint: answerDeltaFingerprint(text),
    })
    const run = detail().lastRun!
    const first = message(2, '先核对订单', { id: 'assistant-run', status: 'streaming' })
    const events = [event(1, 'run.started'), answer(2, first.content)]
    const view = render(renderChat(detail({ messages: [first], runtimeEvents: events })))
    expect(view.queryByText(first.content)).toBeNull()
    const second = { ...first, content: first.content + '并汇总金额', updatedAt: first.updatedAt + 1 }
    events.push(answer(3, '并汇总金额'))
    view.rerender(renderChat(detail({ messages: [second], runtimeEvents: [...events] })))
    expect(view.queryByText(second.content)).toBeNull()
    events.push(event(4, 'message.completed'))
    view.rerender(renderChat(detail({ messages: [{ ...second, status: 'completed' }], runtimeEvents: [...events] })))
    await view.findByText(second.content)
    const settled = view.container.querySelector('[data-contact-message-id="run:response:2"]')
    expect(settled).not.toBeNull()
    view.rerender(renderChat(detail({
      messages: [{ ...second, status: 'completed' }],
      runtimeEvents: [...events, event(5, 'run.failed', { code: 'provider.request_failed' })],
      lastRun: { ...run, status: 'failed', errorCode: 'provider.request_failed', finishedAt: 1700000000005 },
    })))
    await waitFor(() => expect(view.container.querySelector('.fox-contact-ended')?.textContent).toContain('模型'))
    expect(view.container.querySelector('[data-contact-message-id="run:response:2"]') === settled).toBe(true)
    expect(view.container.querySelector('[data-streaming="true"]')).toBeNull()
  })

  test('长回复默认折叠，展开后显示完整内容', async () => {
    const text = `已核对 5,000 行数据。\n\n${'这里是分析明细。'.repeat(180)}\n\n完整报告末尾。`
    const view = render(renderChat(detail({ messages: [message(2, text)] })))
    const expand = await view.findByRole('button', { name: '展开全文' })
    expect(view.queryByText('完整报告末尾。')).toBeNull()
    fireEvent.click(expand)
    await view.findByText('完整报告末尾。')
    fireEvent.click(view.getByRole('button', { name: '收起全文' }))
    expect(view.queryByText('完整报告末尾。')).toBeNull()
  })

  test('一条已完成的回复按段落显示，代码块保留在同一个气泡内', async () => {
    const text = '先看结论。\n\n```text\n第一行\n\n第二行\n```\n\n再说下一步。'
    const view = render(renderChat(detail({ messages: [message(2, text)] })))
    await view.findByText('再说下一步。')
    const reply = view.container.querySelector('[data-contact-message-id="run:kernel:2:response"]')!
    const bubbles = reply.querySelectorAll('.fox-contact-bubble')
    expect(bubbles).toHaveLength(3)
    expect(bubbles[1].querySelector('pre')?.textContent).toContain('第二行')
    expect(reply.querySelectorAll('.fox-thread-copy-button')).toHaveLength(1)
  })

  test('长回复有完整报告时只显示短段，附件按钮打开该报告', async () => {
    const report: ArtifactRecord = {
      id: 'companion-report:run', conversationId: 'chat', runId: 'run', displayName: '完整回复.md',
      artifactType: 'companion_report', artifactClass: 'deliverable', artifactOrigin: 'host_private',
      storagePath: 'D:/data/report.md', mediaType: 'text/markdown', byteSize: 5000, sha256: null,
      status: 'ready', createdAt: 1, updatedAt: 1, declaredAt: 1,
    }
    const text = `已核对 5,000 行订单。\n\n${'这里是分析明细。'.repeat(180)}\n\n完整报告末尾。`
    const opened: ArtifactRecord[] = []
    const view = render(renderChat(detail({ messages: [message(2, text)], artifacts: [report] }), context, artifact => opened.push(artifact)))
    await view.findByText('已核对 5,000 行订单。')
    expect(view.queryByText('完整报告末尾。')).toBeNull()
    expect(view.queryByRole('button', { name: '展开全文' })).toBeNull()
    fireEvent.click(view.getByRole('button', { name: /完整回复.md/ }))
    expect(opened.map(artifact => artifact.id)).toEqual([report.id])
  })

  test('不按标点切分，不把内部思考变成气泡，重复内容保留真实消息数', () => {
    const saved = detail({ messages: [message(2, '<think>内部分析</think>正文。第二句。'), message(4, '正文。第二句。')] })
    const row = deriveFoxThreadMessages({ detail: saved })[0]
    expect(contactReplies(row, [])).toEqual([
      { id: 'run:kernel:2:response', text: '正文。第二句。', createdAt: 1700000000002 },
      { id: 'run:kernel:4:response', text: '正文。第二句。', createdAt: 1700000000004 },
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

test('restored_invalidated_approval_shows_reason_and_cannot_be_reapproved', async () => {
  // D1-6: load an approved+invalidation record from the persistent API shape.
  const approval: ApprovalRecord = { id: 'ticket', conversationId: 'chat', runId: 'run', toolCallId: 't', toolName: 'call_mcp_tool', status: 'approved',
    requestedAction: '发送消息', request: { input: { serverId: 'feishu', tool: 'send_message', arguments: {} }, availableDecisions: ['allow_once', 'allow_conversation', 'deny'] },
    decision: { approved: true, scope: 'conversation', invalidation: { code: 'companion.tool_not_available', message: '联系人已撤销对工具 send_message 的授权', at: 2 } }, requestedAt: 1, resolvedAt: 2 }
  const assertInvalidated = (container: HTMLElement) => {
    expect(container.textContent).toContain('本次操作未执行，旧的批准不会恢复')
    expect(container.textContent).toContain('联系人已撤销对工具 send_message 的授权')
    expect(container.textContent).toContain('重新授权后请重新发起')
    expect(container.textContent).not.toContain('用户拒绝')
    expect(container.querySelectorAll('.fox-contact-choice')).toHaveLength(0)
    // The original scope survives only as secondary history, not the main state.
    expect(container.textContent).toContain('原选择（历史）：本次对话内允许')
  }
  const view = render(withContact(<RuntimeApprovalPrompt approval={approval} onResolve={async () => true} />))
  assertInvalidated(view.container)
  view.unmount()
  const reopened = render(withContact(<RuntimeApprovalPrompt approval={approval} onResolve={async () => true} />))
  assertInvalidated(reopened.container)
  reopened.unmount()
  // A late approved event without the barrier must not erase the seen reason.
  const merged = withMonotonicInvalidation(approval, { ...approval, decision: { approved: true, scope: 'conversation' } })
  const stale = render(withContact(<RuntimeApprovalPrompt approval={merged} onResolve={async () => true} />))
  assertInvalidated(stale.container)
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
  expect(reopened.container.querySelector('.is-resolved')).not.toBeNull()
  expect(reopened.getByText('代码').closest('.fox-contact-answer-choice')?.textContent).toBe('B代码')
  expect(reopened.queryByRole('button')).toBeNull()
  expect(reopened.queryByRole('textbox')).toBeNull()
  expect(deriveFoxThreadMessages({ detail: saved, contactChat: true }).some(item => item.id === 'user-answer')).toBe(false)
  expect(deriveFoxThreadMessages({ detail: saved }).some(item => item.id === 'user-answer')).toBe(true)
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
  expect(reopened.queryByRole('button', { name: '确认移动' })).toBeNull()
  expect(reopened.queryByRole('button', { name: '暂不移动' })).toBeNull()
  expect(reopened.container.querySelector('.is-resolved')).not.toBeNull()
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
