import { afterAll, afterEach, expect, mock, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { TooltipProvider } from '../src/components/ui/tooltip'
import type { ConversationDetail } from '../src/features/conversations/model/types'

// mock.module is process-wide: spread the real client so other files keep working, and put it back afterwards.
const actualDesktopClient = { ...await import('../src/features/conversations/api/desktop-client') }
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => actualDesktopClient) })

let inspect: () => Promise<unknown> = async () => { throw new Error('not set') }
let calls: string[] = []

mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    inspectArtifact: async () => { calls.push('inspectArtifact'); return inspect() },
    artifactAction: async (_conversationId: string, _artifactId: string, action: string) => {
      calls.push(action)
      return { ...await inspect() as object, action, completed: true }
    },
    readProjectFile: async () => {
      calls.push('readProjectFile')
      return { path: 'docs/项目介绍.md', name: '项目介绍.md', content: '现在的内容', byteSize: 34407, truncated: false }
    },
    projectFileAction: async () => ({ action: 'inspect', completed: true, applications: [] }),
  },
}))

const { RightPanel, createOpenFileTab } = await import('../src/features/chat/right-panel/fox-right-panel')

afterEach(() => { cleanup(); calls = [] })

const artifact = { id: 'a1', displayName: '项目介绍.md', storagePath: 'D:/项目/docs/项目介绍.md', artifactType: 'created_file', byteSize: 13011, declaredDescription: '项目可视化介绍页' }
const detail = {
  conversation: { id: 'preview-test', projectRoot: 'D:/项目' }, tasks: [], artifacts: [artifact], toolCalls: [], childRuns: [], runtimeEvents: [], knowledgeBindings: [], approvals: [],
} as unknown as ConversationDetail

function failWith(code: string, message: string) {
  return async () => { throw new actualDesktopClient.DesktopCommandError({ code, message, retryable: false }) }
}

function renderPreview() {
  const tab = createOpenFileTab('docs/项目介绍.md', 'a1')
  return render(<TooltipProvider><RightPanel mode="files" tabs={['files']} detail={detail} activity={{ goal: null, focusedEvidence: null }} width={320} fileTabs={[tab]} activeFileTabId={tab.id} activeChildAgent={null} onMode={() => {}} onCloseMode={() => {}} onOpenFile={() => {}} onActivateFile={() => {}} onCloseFile={() => {}} onOpenChildAgent={() => {}} onBackChildAgent={() => {}} onToggleMaximized={() => {}} onCollapse={() => {}} /></TooltipProvider>)
}

test('交付后又被改过的文件显示现在的内容，且没有说明行', async () => {
  inspect = failWith('artifact.integrity_mismatch', '产物文件大小已发生变化')
  const view = renderPreview()
  await waitFor(() => expect(view.getByText('现在的内容')).toBeDefined())
  expect(view.queryByText('回答之后这个文件又被修改过，下面是现在的内容。')).toBeNull()
  expect(view.container.querySelector('.fox-file-head-main > strong')?.getAttribute('title')).toMatch(/^\/docs\/项目介绍.md · \S+/)
  expect(view.queryByText(/请重试|操作未能完成/)).toBeNull()
  expect(calls).toEqual(['inspectArtifact', 'readProjectFile'])
})

test('找不到交付文件时说明原因，不提供无效的重试', async () => {
  inspect = failWith('artifact.file_missing', '产物文件不存在')
  const view = renderPreview()
  await waitFor(() => expect(view.getByText('找不到这个文件')).toBeDefined())
  expect(view.queryByRole('button', { name: '重试' })).toBeNull()
  expect(view.container.querySelector('.fox-file-head-main > strong')?.textContent).toBe('项目介绍.md')
  expect(calls).not.toContain('readProjectFile')
})

test('正常文件使用单行标题和图标操作，无版本记录时隐藏历史按钮', async () => {
  inspect = async () => ({ artifact, capabilities: { preview: true, open: true, reveal: true, copyPath: true, openWith: false }, applications: [], preview: { kind: 'text', available: true, content: '交付时的内容', truncated: false, byteSize: 13011, reason: null } })
  const view = renderPreview()
  await waitFor(() => expect(view.getByText('交付时的内容')).toBeDefined())
  expect(view.container.querySelector('.fox-file-head-main > strong')?.getAttribute('title')).toMatch(/^\/docs\/项目介绍.md · \S+/)
  expect(view.container.querySelector('.fox-file-head')).not.toBeNull()
  expect(view.container.querySelector('.fox-file-body')?.textContent).toContain('交付时的内容')
  expect(view.getByRole('button', { name: '打开', exact: true })).toBeDefined()
  expect(view.getByRole('button', { name: '打开所在文件夹' })).toBeDefined()
  expect(view.getByRole('button', { name: '更多文件操作' })).toBeDefined()
  expect(view.queryByRole('button', { name: '历史版本' })).toBeNull()
  fireEvent.click(view.getByRole('button', { name: '打开', exact: true }))
  await waitFor(() => expect(view.getByRole('button', { name: '打开所在文件夹' })).not.toHaveProperty('disabled', true))
  fireEvent.click(view.getByRole('button', { name: '打开所在文件夹' }))
  await waitFor(() => expect(calls).toEqual(['inspectArtifact', 'open', 'reveal']))
  expect(view.container.querySelector('.fox-file-head')?.textContent).not.toMatch(/已用系统默认方式打开|已在文件夹中定位/)
})

test('截断提示位于文件头部下方，不占用预览内容区', async () => {
  inspect = async () => ({ artifact, capabilities: { preview: true, open: true, reveal: true, copyPath: true, openWith: false }, applications: [], preview: { kind: 'text', available: true, content: '截断的内容', truncated: true, byteSize: 13011, reason: null } })
  const view = renderPreview()
  await waitFor(() => expect(view.getByText('截断的内容')).toBeDefined())
  expect(view.getByText('预览已截断，只显示前 512 KB。').closest('.fox-file-head')).not.toBeNull()
  expect(view.container.querySelector('.fox-file-body')?.textContent).toBe('截断的内容')
})
