// 13.38 实测修复（界面部分）：会话头部「更换项目」、失败工具卡的原文只在开发者模式显示。

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, render, fireEvent, act } = await import('@testing-library/react')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { ChatTopbar, changeProjectItemState } = await import('../src/features/chat/shell/fox-chat-topbar')
const { toolFailurePresentation, toolFailureRawText } = await import('../src/features/chat/run-status-facts')
import type { ConversationSummary, ProjectRecord } from '../src/features/conversations/model/types'

afterEach(cleanup)

const conversation = {
  id: 'conv-1', agentId: 'fox-general', agentName: 'Fox', title: '列出目录', projectId: 'p1', projectRoot: 'D:/测试/项目甲',
  permissionMode: 'ask', agentBackend: 'pi', status: 'active', createdAt: 0, updatedAt: 0, lastMessageAt: 0,
} as unknown as ConversationSummary
const projects = [
  { id: 'p1', name: '项目甲', rootPath: 'D:/测试/项目甲', status: 'active', permissionMode: 'ask' },
  { id: 'p2', name: '项目乙', rootPath: 'D:/测试/项目乙', status: 'active', permissionMode: 'allow' },
] as ProjectRecord[]

function Topbar({ state = 'idle', onChangeProject }: { state?: 'idle' | 'running'; onChangeProject: (conversation: ConversationSummary, root: string | null, mode?: string) => void }) {
  return (
    <TooltipProvider>
      <ChatTopbar rightSidebarCollapsed onRightSidebarExpand={() => {}} onRightSidebarCollapse={() => {}} onOpenRightMode={() => {}}
        conversation={conversation} detail={null} modelName="deepseek-chat" state={state as never}
        onPinConversation={() => {}} onRenameConversation={() => {}} onResetContext={() => {}} onArchiveConversation={() => {}}
        projects={projects} onChangeProject={onChangeProject} onPickProjectFolder={() => {}} />
    </TooltipProvider>
  )
}

async function openMenu(view: ReturnType<typeof render>) {
  const trigger = view.getByTestId('conversation-menu')
  await act(async () => {
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: 'mouse' })
    fireEvent.keyDown(trigger, { key: 'Enter' })
  })
}

describe('13.38 R2-4：会话头部「更多 → 更换项目」', () => {
  test('运行中不可用并说明原因；空闲时可用', () => {
    expect(changeProjectItemState('running', conversation)).toEqual({ available: false, reason: '对话正在运行，请先停止' })
    expect(changeProjectItemState('idle', conversation)).toEqual({ available: true, reason: null })
    expect(changeProjectItemState('idle', null).available).toBe(false)
  })

  test('菜单里有「更换项目」，子菜单列出项目，当前项目不可选，选别的项目就在原对话里更换', async () => {
    const calls: Array<[string, string | null, string | undefined]> = []
    const view = render(<Topbar onChangeProject={(c, root, mode) => calls.push([c.id, root, mode])} />)
    await openMenu(view)
    const item = await view.findByTestId('change-project')
    expect(item.textContent).toContain('更换项目')
    await act(async () => {
      fireEvent.pointerMove(item, { pointerType: 'mouse' })
      fireEvent.keyDown(item, { key: 'ArrowRight' })
    })
    const other = await view.findByText('项目乙')
    const current = view.getByText('项目甲').closest('[role=menuitem]')
    expect(current?.getAttribute('aria-disabled')).toBe('true')
    await act(async () => { fireEvent.click(other) })
    expect(calls).toEqual([['conv-1', 'D:/测试/项目乙', 'allow']])
    expect(view.getByText('选择文件夹…')).toBeTruthy()
  })

  test('运行中的菜单项是灰的，带原因', async () => {
    const view = render(<Topbar state="running" onChangeProject={() => { throw new Error('must not change') }} />)
    await openMenu(view)
    const item = await view.findByTestId('change-project')
    expect(item.getAttribute('aria-disabled')).toBe('true')
    expect(item.textContent).toContain('对话正在运行，请先停止')
  })
})

describe('13.38 B5-3：失败工具卡普通模式只有中文句子，原文给开发者模式', () => {
  const blocked = {
    content: [{ type: 'text', text: '[tool.permission_denied] 网络保护拦截了这次访问：example.com 解析到 198.18.0.7，这是内网、本机或保留地址，Fox 不会从这类地址读取内容。' }],
    isError: true,
    details: { errorCode: 'tool.permission_denied' },
  }

  test('普通模式的句子没有英文原文；原文能取出来给开发者模式', () => {
    const failure = toolFailurePresentation('web_read', blocked)
    expect(failure.summary).toBe('网络保护拦截了这个地址')
    expect(failure.hint).toContain('内网、本机或保留地址')
    expect(toolFailurePresentation('write_file', { details: { errorCode: 'tool.permission_denied' } }).summary).toBe('本次未能写入文件')
    expect(`${failure.summary}${failure.hint ?? ''}`).not.toMatch(/[A-Za-z]{4,}/)
    expect(toolFailureRawText(blocked)).toContain('网络保护拦截了这次访问')
    expect(toolFailureRawText('web_read blocks private addresses')).toBe('web_read blocks private addresses')
    expect(toolFailureRawText({ error: 'boom' })).toBe('boom')
    expect(toolFailureRawText(null)).toBeNull()
  })
})
