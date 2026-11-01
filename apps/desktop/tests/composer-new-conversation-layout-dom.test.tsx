// 新对话输入框：没有模型正文时保持高内框和托盘；有正文后才变成已有对话的窄输入框。

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, render } = await import('@testing-library/react')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { MemoizedComposer, conversationHasModelReply } = await import('../src/features/chat/composer/fox-composer')
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

  test('已选中但没有模型回复时仍是新对话托盘，项目和 Agent 不可切换', () => {
    const detail = {
      messages: [
        { role: 'user', content: '你好' },
        { role: 'assistant', content: '' },
        { role: 'tool', content: '工具输出' },
      ],
    }
    expect(conversationHasModelReply(detail)).toBe(false)
    const view = render(
      <Harness
        newConversation={!conversationHasModelReply(detail)}
        entryFrozen
        boundAgentBackend="deepseek"
        projectRoot="D:/测试/项目甲"
      />,
    )
    const wrap = view.container.querySelector('.fox-composer-wrap')
    expect(wrap?.classList.contains('is-new-conversation')).toBe(true)
    expect(wrap?.classList.contains('is-history')).toBe(false)
    expect(view.container.querySelector('.fox-composer-draft-footer')).not.toBeNull()
    expect(view.queryByRole('button', { name: /选择 Agent/ })).toBeNull()
    const agent = view.getByTitle('对话创建后不能更换 Agent')
    expect(agent.textContent).toBe('DeepSeek')
    expect(agent.getAttribute('data-agent-backend')).toBe('deepseek')
    expect(view.queryByRole('button', { name: /项目甲|选择项目/ })).toBeNull()
    const project = view.getByTitle('对话创建后不能更换项目')
    expect(project.textContent).toContain('项目甲')
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
  test('只有用户消息、空白 assistant 或工具消息时不算回复，有正文才算', () => {
    expect(conversationHasModelReply({ messages: [] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'user', content: '在吗' }] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'assistant', content: '   \n' }] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'assistant', content: null }] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'tool', content: '已写入' }] })).toBe(false)
    expect(conversationHasModelReply({ messages: [{ role: 'assistant', content: '好' }] })).toBe(true)
  })
})
