// 首条消息提交后切到已有对话的输入框。

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, fireEvent, render, waitFor, within } = await import('@testing-library/react')
const { FoxDecisionCard } = await import('../src/features/chat/thread/composer/fox-decision-card')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { MemoizedComposer, composerEntryFrozen, composerUsesNewConversation, conversationFirstReplyCompleted, conversationHasModelReply } = await import('../src/features/chat/composer/fox-composer')
const { ComposerEntryContext } = await import('../src/features/chat/composer/composer-entry-context')
import type { ApprovalRecord, ProjectRecord } from '../src/features/conversations/model/types'
import type { AgentBackendId } from '../src/features/conversations/model/agent-backend'

afterEach(cleanup)

const projects = [{
  id: 'project-1',
  name: '项目甲',
  rootPath: 'D:/测试/项目甲',
  status: 'active',
  permissionMode: 'ask',
}] as ProjectRecord[]

function Harness({ newConversation, entryFrozen = false, boundAgentBackend = null, projectRoot = null, questionRequest, onSubmitQuestion, runtimeApprovals, onResolveApproval }: {
  newConversation: boolean
  entryFrozen?: boolean
  boundAgentBackend?: AgentBackendId | null
  projectRoot?: string | null
  questionRequest?: React.ComponentProps<typeof MemoizedComposer>['questionRequest']
  onSubmitQuestion?: React.ComponentProps<typeof MemoizedComposer>['onSubmitQuestion']
  runtimeApprovals?: ApprovalRecord[]
  onResolveApproval?: React.ComponentProps<typeof MemoizedComposer>['onResolveApproval']
}) {
  const [draft, setDraft] = React.useState('')
  return (
    <TooltipProvider>
      <ComposerEntryContext.Provider value={{
        projects,
        onSelectProject: () => {},
        onAddProject: () => {},
        draftAgentBackend: 'fox',
        onSelectAgentBackend: () => {},
      }}>
        <MemoizedComposer
          resetKey={0}
          draft={draft}
          setDraft={setDraft}
          chatState="idle"
          centered={newConversation}
          newConversation={newConversation}
          runtimeControlled
          entryFrozen={entryFrozen}
          boundAgentBackend={boundAgentBackend}
          projectRoot={projectRoot}
          questionRequest={questionRequest}
          onSubmitQuestion={onSubmitQuestion}
          runtimeApprovals={runtimeApprovals}
          onResolveApproval={onResolveApproval}
        />
      </ComposerEntryContext.Provider>
    </TooltipProvider>
  )
}

describe('新对话输入框形态', () => {
  test('未选中会话使用新对话高输入框和托盘，项目与 Agent 仍可切换', () => {
    expect(conversationHasModelReply(null)).toBe(false)
    const view = render(<Harness newConversation={!conversationHasModelReply(null)} />)
    const wrap = view.container.querySelector('.fox-composer-wrap')
    expect(wrap?.classList.contains('is-new-conversation')).toBe(true)
    expect(wrap?.classList.contains('is-history')).toBe(false)
    expect(view.container.querySelector('.fox-composer-draft-footer')).not.toBeNull()
    expect(view.getByRole('button', { name: '选择 Agent，当前 Fox' })).toBeTruthy()
    expect(view.getByRole('button', { name: '选择项目' })).toBeTruthy()
    expect(view.container.querySelector('[data-agent-frozen]')).toBeNull()
    expect(view.container.querySelector('[data-project-frozen]')).toBeNull()
  })

  test('已选中但未发送消息的空会话保留新对话托盘', () => {
    const detail = {
      conversation: { id: 'conv-empty' },
      messages: [],
      toolCalls: [],
    }
    expect(conversationHasModelReply(detail)).toBe(false)
    const newConversation = composerUsesNewConversation({ desktopEnabled: true, timelineEmpty: true, selectedConversationId: 'conv-empty', detail })
    expect(newConversation).toBe(true)
    const frozen = composerEntryFrozen({ desktopEnabled: true, selectedConversationId: 'conv-empty', newConversation })
    expect(frozen).toBe(false)
    const view = render(
      <Harness
        newConversation={newConversation}
        entryFrozen={frozen}
        boundAgentBackend="deepseek"
        projectRoot="D:/测试/项目甲"
      />,
    )
    const wrap = view.container.querySelector('.fox-composer-wrap')
    expect(wrap?.classList.contains('is-new-conversation')).toBe(true)
    expect(wrap?.classList.contains('is-history')).toBe(false)
    expect(view.container.querySelector('.fox-composer-draft-footer')).not.toBeNull()
    // The picker shows the conversation's own Agent, and opens.
    const agent = view.getByRole('button', { name: /选择 Agent，当前 DeepSeek/ })
    expect(agent.getAttribute('data-agent-backend')).toBe('deepseek')
    expect(view.container.querySelector('[data-agent-frozen]')).toBeNull()
    expect(view.container.querySelector('[data-project-frozen]')).toBeNull()
    const project = view.getByRole('button', { name: /项目甲/ })
    expect(project.getAttribute('aria-haspopup')).toBe('dialog')
  })

  test('已有对话和草稿的项目、Agent 托盘使用各自状态', () => {
    expect(composerEntryFrozen({ desktopEnabled: true, selectedConversationId: 'conv', newConversation: false })).toBe(true)
    expect(composerEntryFrozen({ desktopEnabled: true, selectedConversationId: 'conv', newConversation: true })).toBe(false)
    expect(composerEntryFrozen({ desktopEnabled: true, selectedConversationId: null, newConversation: true })).toBe(false)
    expect(composerEntryFrozen({ desktopEnabled: false, selectedConversationId: 'conv', newConversation: false })).toBe(false)
    // A replied conversation that somehow renders the tray still shows its values read-only.
    const view = render(<Harness newConversation entryFrozen boundAgentBackend="pi" projectRoot="D:/测试/项目甲" />)
    expect(view.getByTitle('对话创建后不能更换 Agent').getAttribute('data-agent-backend')).toBe('pi')
    expect(view.getByTitle('对话创建后不能更换项目').textContent).toContain('项目甲')
  })

  test('只有工具调用时收成窄输入框且没有托盘', () => {
    const detail = {
      conversation: { id: 'conv-tools' },
      messages: [
        { role: 'user', content: '帮我看一下' },
        { role: 'assistant', content: '' },
      ],
      toolCalls: [{ id: 'call-1' }],
    }
    expect(conversationHasModelReply(detail)).toBe(true)
    const view = render(<Harness newConversation={!conversationHasModelReply(detail)} />)
    const wrap = view.container.querySelector('.fox-composer-wrap')
    expect(wrap?.classList.contains('is-history')).toBe(true)
    expect(wrap?.classList.contains('is-new-conversation')).toBe(false)
    expect(view.container.querySelector('.fox-composer-draft-footer')).toBeNull()
  })

  test('选中会话但 detail 还没加载时不显示托盘', () => {
    const stale = { conversation: { id: 'conv-previous' }, messages: [], toolCalls: [] }
    expect(composerUsesNewConversation({
      desktopEnabled: true,
      timelineEmpty: true,
      selectedConversationId: 'conv-next',
      detail: null,
    })).toBe(false)
    expect(composerUsesNewConversation({
      desktopEnabled: true,
      timelineEmpty: true,
      selectedConversationId: 'conv-next',
      detail: stale,
    })).toBe(false)
    expect(composerUsesNewConversation({
      desktopEnabled: true,
      timelineEmpty: true,
      selectedConversationId: null,
      detail: null,
    })).toBe(true)
    const view = render(<Harness newConversation={composerUsesNewConversation({
      desktopEnabled: true,
      timelineEmpty: true,
      selectedConversationId: 'conv-next',
      detail: null,
    })} />)
    const wrap = view.container.querySelector('.fox-composer-wrap')
    expect(wrap?.classList.contains('is-history')).toBe(true)
    expect(view.container.querySelector('.fox-composer-draft-footer')).toBeNull()
  })

  test('已有模型回复时使用窄输入框且没有托盘', () => {
    const detail = {
      messages: [
        { role: 'user', content: '你好' },
        { role: 'assistant', content: '第一段回复' },
      ],
    }
    expect(conversationHasModelReply(detail)).toBe(true)
    const view = render(
      <Harness
        newConversation={!conversationHasModelReply(detail)}
        entryFrozen
        boundAgentBackend="deepseek"
        projectRoot="D:/测试/项目甲"
      />,
    )
    const wrap = view.container.querySelector('.fox-composer-wrap')
    expect(wrap?.classList.contains('is-history')).toBe(true)
    expect(wrap?.classList.contains('is-new-conversation')).toBe(false)
    expect(view.container.querySelector('.fox-composer-draft-footer')).toBeNull()
    expect(view.queryByTitle('对话创建后不能更换 Agent')).toBeNull()
    expect(view.queryByRole('button', { name: /选择 Agent/ })).toBeNull()
  })
})

describe('conversationHasModelReply', () => {
  test('空 assistant 占位不算回复；正文、tool 消息或 toolCalls 算已开始', () => {
    expect(conversationHasModelReply({ messages: [] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'user', content: '在吗' }] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'assistant', content: '   \n' }] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'assistant', content: null }], toolCalls: [] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'tool', content: '已写入' }] })).toBe(true)
    expect(conversationHasModelReply({ messages: [{ role: 'tool', content: '' }] })).toBe(true)
    expect(conversationHasModelReply({ messages: [{ role: 'assistant', content: '' }], toolCalls: [{ id: 'call-1' }] })).toBe(true)
    expect(conversationHasModelReply({ messages: [{ role: 'assistant', content: '好' }] })).toBe(true)
  })
})

describe('首条消息提交后的输入栏', () => {
  const cancelledFirstRound = {
    conversation: { id: 'conv-cancelled' },
    messages: [
      { role: 'user', content: 'ping -n 60 127.0.0.1' },
      { role: 'assistant', content: '我先运行这条命令' },
    ],
    toolCalls: [{ id: 'call-ping' }],
    lastRun: { status: 'cancelled' },
    runs: [{ status: 'cancelled' }],
  }

  test('首条消息刚提交、会话尚未创建时就收窄', () => {
    const view = render(<Harness newConversation />)
    expect(view.container.querySelector('.fox-composer-draft-footer')).not.toBeNull()
    const pending = composerUsesNewConversation({ desktopEnabled: true, timelineEmpty: false, detail: null })
    view.rerender(<Harness newConversation={pending} />)
    expect(view.container.querySelector('.fox-composer-wrap.is-history')).not.toBeNull()
    expect(view.container.querySelector('.fox-composer-draft-footer')).toBeNull()
  })

  test('等待首响应、失败、中断或取消后仍保留窄样式', () => {
    const state = { desktopEnabled: true, timelineEmpty: false, selectedConversationId: 'conv-cancelled' }
    for (const status of ['failed', 'interrupted', 'cancelled']) {
      const detail = { ...cancelledFirstRound, lastRun: { status }, runs: [{ status }] }
      expect(conversationFirstReplyCompleted(detail)).toBe(false)
      expect(composerUsesNewConversation({ ...state, detail })).toBe(false)
    }
    const running = { ...cancelledFirstRound, lastRun: { status: 'running' }, runs: [{ status: 'cancelled' }, { status: 'running' }] }
    const waiting = { ...running, toolCalls: [], messages: [{ role: 'user', content: '在吗' }] }
    expect(composerUsesNewConversation({ ...state, detail: waiting })).toBe(false)
    const completed = { ...cancelledFirstRound, lastRun: { status: 'completed' }, runs: [{ status: 'cancelled' }, { status: 'completed' }] }
    expect(conversationFirstReplyCompleted(completed)).toBe(true)
    expect(composerUsesNewConversation({ ...state, detail: completed })).toBe(false)
  })

  test('没有 Run 记录的老会话按「模型是否回复过」；更早的消息没加载时按已完成', () => {
    expect(conversationFirstReplyCompleted({ messages: [{ role: 'assistant', content: '好' }] })).toBe(true)
    expect(conversationFirstReplyCompleted({ messages: [{ role: 'user', content: '在吗' }] })).toBe(false)
    expect(conversationFirstReplyCompleted({ messages: [], runs: [{ status: 'cancelled' }], hasEarlierMessages: true })).toBe(true)
    expect(conversationFirstReplyCompleted(null)).toBe(false)
  })
})

test.each(['输入框', '对话卡'])('%s的问题分页保留各题的选项和自定义答案', async surface => {
  const submitted: Array<Record<string, string | string[]>> = []
  const questionRequest = { runId: 'run-question', questions: [
    { questionId: 'grouping', question: '按什么维度汇总？', options: [{ label: '按地区', value: 'region' }, { label: '按商品', value: 'product' }], multiSelect: false, allowOther: true },
    { questionId: 'format', question: '需要什么交付格式？', options: [{ label: 'CSV', value: 'csv' }, { label: 'Excel', value: 'xlsx' }], multiSelect: true, allowOther: true },
  ] }
  const onSubmitQuestion = async (_text: string, answers: Record<string, string | string[]>) => { submitted.push(answers); return true }
  const view = render(surface === '输入框' ? <Harness newConversation={false} questionRequest={questionRequest} onSubmitQuestion={onSubmitQuestion} />
    : <FoxDecisionCard questionRequest={questionRequest} onSubmitQuestion={onSubmitQuestion} />)
  const pages = within(view.getByRole('navigation', { name: '问题分页' }))
  expect(pages.getByText('01/02')).toBeDefined()
  expect((pages.getByRole('button', { name: '上一题' }) as HTMLButtonElement).disabled).toBe(true)
  expect(view.queryByRole('heading', { name: '需要什么交付格式？' })).toBeNull()
  const region = view.getByRole('button', { name: '按地区' })
  fireEvent.click(region)
  expect(region.getAttribute('aria-pressed')).toBe('true')
  fireEvent.change(view.getByRole('textbox', { name: '按什么维度汇总？ 自定义答案' }), { target: { value: '按月份' } })
  expect(region.getAttribute('aria-pressed')).toBe('false')
  fireEvent.click(pages.getByRole('button', { name: '下一题' }))
  expect(pages.getByText('02/02')).toBeDefined()
  expect((pages.getByRole('button', { name: '下一题' }) as HTMLButtonElement).disabled).toBe(true)
  expect(view.queryByRole('heading', { name: '按什么维度汇总？' })).toBeNull()
  fireEvent.click(view.getByRole('button', { name: 'CSV' }))
  fireEvent.click(view.getByRole('button', { name: 'Excel' }))
  fireEvent.click(pages.getByRole('button', { name: '上一题' }))
  expect((view.getByRole('textbox', { name: '按什么维度汇总？ 自定义答案' }) as HTMLInputElement).value).toBe('按月份')
  fireEvent.click(pages.getByRole('button', { name: '下一题' }))
  expect(view.getByRole('button', { name: 'CSV' }).getAttribute('aria-pressed')).toBe('true')
  fireEvent.click(view.getByRole('button', { name: '确认并继续' }))
  await waitFor(() => expect(submitted).toEqual([{ grouping: '按月份', format: ['csv', 'xlsx'] }]))
})

test('问题作为标题，三个预设选项和自定义输入占四行', async () => {
  const submitted: Array<Record<string, string | string[]>> = []
  const view = render(<Harness newConversation={false} questionRequest={{ runId: 'missing-file', questions: [
    { questionId: 'file', question: '怎样处理缺失台账？', options: [{ label: '新建台账', value: 'create' }, { label: '提供文件', value: 'upload' }, { label: '先列字段', value: 'fields' }], multiSelect: false, allowOther: true },
  ] }} onSubmitQuestion={async (_text, answers) => { submitted.push(answers); return true }} />)
  expect(view.getByRole('heading', { name: '怎样处理缺失台账？' }).closest('.fox-question-field')?.firstElementChild?.tagName).toBe('HEADER')
  expect(view.queryByText('需要你补充一点信息')).toBeNull()
  expect(view.queryByRole('navigation', { name: '问题分页' })).toBeNull()
  const rows = view.getAllByRole('listitem')
  expect(rows.map(row => row.textContent)).toEqual(['1新建台账', '2提供文件', '3先列字段'])
  const other = view.getByRole('textbox', { name: '怎样处理缺失台账？ 自定义答案' })
  expect(other.closest('label')?.textContent).toBe('4')
  expect(other.closest('.fox-question-decision-actions')).toBe(view.getByRole('button', { name: '确认并继续' }).parentElement)
  expect(other.getAttribute('placeholder')).toBe('请输入其他选择')
  fireEvent.change(other, { target: { value: '先检查归档目录' } })
  fireEvent.click(view.getByRole('button', { name: '确认并继续' }))
  await waitFor(() => expect(submitted).toEqual([{ file: '先检查归档目录' }]))
})

test('待审批卡在输入框位置按顺序显示，处理后恢复草稿', async () => {
  const approval = (id: string, requestedAt: number) => ({
    id, requestedAt, conversationId: 'chat', runId: 'run', toolCallId: id, toolName: 'write_file', status: 'pending',
    request: { tool: 'write_file', input: { path: `D:/project/${id}.txt` }, availableDecisions: ['deny', 'allow_once'] },
  } as ApprovalRecord)
  const first = approval('first', 1)
  const second = approval('second', 2)
  const resolved: string[] = []
  const resolve = async (id: string) => { resolved.push(id); return true }
  const view = render(<Harness newConversation={false} />)
  fireEvent.change(view.getByRole('textbox'), { target: { value: '还没发出的内容' } })
  view.rerender(<Harness newConversation={false} runtimeApprovals={[second, first]} onResolveApproval={resolve} />)
  const approve = await view.findByRole('button', { name: /允许.*一次/ })
  expect(approve.closest('.fox-prompt-shell.is-approval')).not.toBeNull()
  expect(view.queryByRole('textbox')).toBeNull()
  expect(view.getByText('01/02')).toBeDefined()
  expect(view.container.textContent).not.toContain('second.txt')
  fireEvent.click(view.getByRole('button', { name: '下一项审批' }))
  expect(view.getByText('02/02')).toBeDefined()
  expect(view.container.textContent).not.toContain('first.txt')
  fireEvent.click(view.getByRole('button', { name: '上一项审批' }))
  fireEvent.click(view.getByRole('button', { name: /允许.*一次/ }))
  await waitFor(() => expect(resolved).toEqual(['first']))
  view.rerender(<Harness newConversation={false} runtimeApprovals={[second]} onResolveApproval={resolve} />)
  fireEvent.click(await view.findByRole('button', { name: /允许.*一次/ }))
  await waitFor(() => expect(resolved).toEqual(['first', 'second']))
  view.rerender(<Harness newConversation={false} />)
  expect((view.getByRole('textbox') as HTMLTextAreaElement).value).toBe('还没发出的内容')
})
