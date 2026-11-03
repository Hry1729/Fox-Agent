// 新对话输入框：模型还没开始回复时保持高内框和托盘；正文、工具消息或 toolCalls 出现后收成窄输入框。

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, render } = await import('@testing-library/react')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { MemoizedComposer, composerEntryFrozen, composerUsesNewConversation, conversationFirstReplyCompleted, conversationHasModelReply } = await import('../src/features/chat/composer/fox-composer')
const { ComposerEntryContext } = await import('../src/features/chat/composer/composer-entry-context')
import type { ProjectRecord } from '../src/features/conversations/model/types'
import type { AgentBackendId } from '../src/features/conversations/model/agent-backend'

afterEach(cleanup)

const projects = [{
  id: 'project-1',
  name: '项目甲',
  rootPath: 'D:/测试/项目甲',
  status: 'active',
  permissionMode: 'ask',
}] as ProjectRecord[]

function Harness({ newConversation, entryFrozen = false, boundAgentBackend = null, projectRoot = null }: {
  newConversation: boolean
  entryFrozen?: boolean
  boundAgentBackend?: AgentBackendId | null
  projectRoot?: string | null
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

  test('13.37：已选中但没有模型回复时仍是新对话托盘，项目和 Agent 都还能换（1103 的只读托盘已撤回）', () => {
    const detail = {
      conversation: { id: 'conv-empty' },
      messages: [
        { role: 'user', content: '你好' },
        { role: 'assistant', content: '' },
      ],
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

  test('13.37：模型回复以后才冻结项目和 Agent；未选中的草稿从不冻结', () => {
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

describe('13.38 R2-4：第一轮回复完成之前保留托盘', () => {
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

  test('第一轮被取消（已经调过工具、出过半截正文）仍是新对话托盘，项目和 Agent 能换', () => {
    expect(conversationHasModelReply(cancelledFirstRound)).toBe(true)
    expect(conversationFirstReplyCompleted(cancelledFirstRound)).toBe(false)
    const newConversation = composerUsesNewConversation({ desktopEnabled: true, timelineEmpty: false, selectedConversationId: 'conv-cancelled', detail: cancelledFirstRound, running: false })
    expect(newConversation).toBe(true)
    const frozen = composerEntryFrozen({ desktopEnabled: true, selectedConversationId: 'conv-cancelled', newConversation })
    expect(frozen).toBe(false)
    const view = render(<Harness newConversation={newConversation} entryFrozen={frozen} boundAgentBackend="pi" projectRoot="D:/测试/项目甲" />)
    expect(view.container.querySelector('.fox-composer-draft-footer')).not.toBeNull()
    expect(view.getByRole('button', { name: /选择 Agent，当前 Pi/ })).toBeTruthy()
    expect(view.getByRole('button', { name: /项目甲/ })).toBeTruthy()
    expect(view.container.querySelector('[data-agent-frozen]')).toBeNull()
  })

  test('失败、中断的第一轮同样不算完成；正在回复时收窄，完成后锁定', () => {
    for (const status of ['failed', 'interrupted', 'cancelled']) {
      expect(conversationFirstReplyCompleted({ ...cancelledFirstRound, lastRun: { status }, runs: [{ status }] })).toBe(false)
    }
    const running = { ...cancelledFirstRound, lastRun: { status: 'running' }, runs: [{ status: 'cancelled' }, { status: 'running' }] }
    expect(composerUsesNewConversation({ desktopEnabled: true, timelineEmpty: false, selectedConversationId: 'conv-cancelled', detail: running, running: true })).toBe(false)
    const waiting = { ...running, toolCalls: [], messages: [{ role: 'user', content: '在吗' }] }
    expect(composerUsesNewConversation({ desktopEnabled: true, timelineEmpty: false, selectedConversationId: 'conv-cancelled', detail: waiting, running: true })).toBe(true)
    const completed = { ...cancelledFirstRound, lastRun: { status: 'completed' }, runs: [{ status: 'cancelled' }, { status: 'completed' }] }
    expect(conversationFirstReplyCompleted(completed)).toBe(true)
    const newConversation = composerUsesNewConversation({ desktopEnabled: true, timelineEmpty: false, selectedConversationId: 'conv-cancelled', detail: completed, running: false })
    expect(newConversation).toBe(false)
    expect(composerEntryFrozen({ desktopEnabled: true, selectedConversationId: 'conv-cancelled', newConversation })).toBe(true)
  })

  test('没有 Run 记录的老会话按「模型是否回复过」；更早的消息没加载时按已完成', () => {
    expect(conversationFirstReplyCompleted({ messages: [{ role: 'assistant', content: '好' }] })).toBe(true)
    expect(conversationFirstReplyCompleted({ messages: [{ role: 'user', content: '在吗' }] })).toBe(false)
    expect(conversationFirstReplyCompleted({ messages: [], runs: [{ status: 'cancelled' }], hasEarlierMessages: true })).toBe(true)
    expect(conversationFirstReplyCompleted(null)).toBe(false)
  })
})
