import { afterEach, describe, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { Command, CommandList } from '@/components/ui/command'
import { ProjectNoteChoices } from '../src/features/chat/composer/fox-composer'
import { projectNoteConversationFor, type ComposerRuntimeContextValue } from '../src/features/chat/fox-shared'
import { shouldRefreshContacts } from '../src/features/companions/context'
import type { ConversationDetail } from '../src/features/conversations/model/types'

afterEach(cleanup)

function choices(patch: Partial<ComposerRuntimeContextValue> = {}) {
  return {
    knowledgeBindings: [],
    projectNoteOptions: { selectedCompanionId: null, options: [{ id: 'fox-a', displayName: 'Alpha' }, { id: 'fox-b', displayName: 'Beta' }] },
    ...patch,
  } satisfies ComposerRuntimeContextValue
}

function mount(runtimeContext: ComposerRuntimeContextValue) {
  return render(<Command><CommandList><ProjectNoteChoices runtimeContext={runtimeContext} /></CommandList></Command>)
}

describe('当前项目对话的小 Fox 笔记选择', () => {
  test('默认不使用，点击只提交所选 companion ID', async () => {
    const selected: Array<string | null> = []
    const view = mount(choices({ onProjectNoteSelect: async id => { selected.push(id) } }))
    expect(screen.getByText('不使用').closest('[cmdk-item]')?.querySelector('.fox-command-selected-check')).not.toBeNull()
    expect(screen.getByText('Alpha').closest('[cmdk-item]')?.querySelector('.fox-command-selected-check')).toBeNull()
    fireEvent.click(screen.getByText('Alpha'))
    await Promise.resolve()
    expect(selected).toEqual(['fox-a'])
    view.rerender(<Command><CommandList><ProjectNoteChoices runtimeContext={choices({ projectNoteOptions: { selectedCompanionId: 'fox-a', options: [{ id: 'fox-a', displayName: 'Alpha' }, { id: 'fox-b', displayName: 'Beta' }] }, onProjectNoteSelect: async id => { selected.push(id) } })} /></CommandList></Command>)
    expect(screen.getByText('Alpha').closest('[cmdk-item]')?.querySelector('.fox-command-selected-check')).not.toBeNull()
    fireEvent.click(screen.getByText('不使用'))
    await Promise.resolve()
    expect(selected).toEqual(['fox-a', null])
  })

  test('保存错误显示且不触发额外选择', () => {
    let called = false
    mount(choices({ projectNoteError: '保存失败：授权已撤销', onProjectNoteSelect: async () => { called = true } }))
    expect(screen.getByText('保存失败：授权已撤销')).toBeTruthy()
    fireEvent.click(screen.getByText('加载或保存失败'))
    expect(called).toBe(false)
  })

  test('普通项目顶层对话之外的会话没有这个控制', () => {
    const detail = (companionId: string | null, parentConversationId: string | null) => ({
      companionId,
      conversation: { id: 'chat-current', projectId: 'project-a', parentConversationId },
    }) as ConversationDetail
    expect(projectNoteConversationFor(detail(null, null))).toBe('chat-current')
    expect(projectNoteConversationFor(detail('fox-private', null))).toBeUndefined()
    expect(projectNoteConversationFor(detail(null, 'parent-chat'))).toBeUndefined()
    expect(projectNoteConversationFor(null)).toBeUndefined()
  })
})

describe('资料工具事件刷新', () => {
  test('仅成功的 companion_profile_update tool.completed 额外刷新', () => {
    const notice = (type: string, tool?: string, isError?: boolean) => ({ event: { type, tool, isError } })
    expect(shouldRefreshContacts(notice('tool.completed', 'companion_profile_update', false))).toBe(true)
    expect(shouldRefreshContacts(notice('tool.completed', 'companion_profile_update'))).toBe(true)
    expect(shouldRefreshContacts(notice('tool.completed', 'companion_profile_update', true))).toBe(false)
    expect(shouldRefreshContacts(notice('tool.completed', 'other_tool', false))).toBe(false)
    expect(shouldRefreshContacts(notice('message.completed'))).toBe(true)
  })
})
