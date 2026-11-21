import { afterAll, afterEach, expect, mock, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { useState } from 'react'
import type { ConversationDetail, ProjectFileEntry } from '../src/features/conversations/model/types'
import { persistDeveloperUi } from '../src/features/chat/developer-ui'

const actual = { ...await import('../src/features/conversations/api/desktop-client') }
let childrenCalls: string[] = []
let searches: string[] = []
const file = (path: string, isDirectory = false): ProjectFileEntry => ({ path, name: path.split('/').at(-1)!, parent: path.split('/').slice(0, -1).join('/'), isDirectory, hasChildren: isDirectory, byteSize: 0, modifiedAt: null })
mock.module('../src/features/conversations/api/desktop-client', () => ({ ...actual, desktopClient: {
  ...actual.desktopClient,
  projectFilesChildren: async (_id: string, path: string) => { childrenCalls.push(path); return { entries: path ? [file(`${path}/deep.txt`)] : [file('docs', true), file('Fox', true), file('readme.md')], truncated: false } },
  projectFilesSearch: async (_id: string, query: string) => { searches.push(query); return { entries: [file('docs/a/b/deep.txt')], truncated: false } },
  permissionAuditList: async () => [{ id: 'audit', tool: 'write_file', mode: 'auto', reason: 'mode', ruleIds: [], risk: 'high', findings: [], summary: 'docs/result.md', createdAt: 1 }],
} }))
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => actual) })
afterEach(() => { cleanup(); childrenCalls = []; searches = []; persistDeveloperUi(false) })

const { useProjectFiles } = await import('../src/features/chat/right-panel/use-project-files')
const { ProjectFileTree } = await import('../src/features/chat/right-panel/fox-right-panel')
const { ActivityDetailPanel } = await import('../src/features/chat/components/ActivityDetailPanel')

function Tree({ refresh = '1' }: { refresh?: string }) {
  const [query, setQuery] = useState('')
  const files = useProjectFiles('test', true, refresh, query)
  return <><input aria-label="搜索" value={query} onChange={event => setQuery(event.target.value)} /><ProjectFileTree entries={files.entries} workspaceName="project" selectedFile="" onSelect={() => {}} onDirectoriesChange={files.setDirectories} />{files.results.map(file => <span key={file.path}>{file.path}</span>)}</>
}

test('文件树先请求根层，展开后才请求下一层，刷新只读取已展开目录', async () => {
  const view = render(<Tree />)
  await view.findByText('docs')
  expect(childrenCalls).toEqual([''])
  fireEvent.click(view.getByText('docs'))
  await view.findByText('deep.txt')
  expect(childrenCalls).toEqual(['', 'docs'])
  view.rerender(<Tree refresh="2" />)
  await waitFor(() => expect(childrenCalls).toEqual(['', 'docs', '', 'docs']))
  expect(childrenCalls).not.toContain('Fox')
})

test('搜索等待 200ms，使用全项目搜索而不是已加载目录', async () => {
  const view = render(<Tree />)
  await view.findByText('docs')
  fireEvent.change(view.getByLabelText('搜索'), { target: { value: 'dee' } })
  fireEvent.change(view.getByLabelText('搜索'), { target: { value: 'deep' } })
  expect(searches).toEqual([])
  await view.findByText('docs/a/b/deep.txt')
  expect(searches).toEqual(['deep'])
  expect(childrenCalls).toEqual([''])
})

test('运行详情显示共享动作名、目标和单行运行摘要，自动放行没有原始工具名', async () => {
  persistDeveloperUi(false)
  const detail = { lastRun: { id: 'run', status: 'completed', startedAt: 1, finishedAt: 1001 }, toolCalls: [
    { id: 'read', toolName: 'read_file', input: { path: 'docs/readme.md' }, status: 'completed' },
    { id: 'cmd', toolName: 'run_command', input: { command: 'bun test' }, status: 'completed' },
  ], runtimeEvents: [], approvals: [] } as unknown as ConversationDetail
  const view = render(<ActivityDetailPanel detail={detail} goal={null} focusedEvidence={null} />)
  await view.findByText('自动放行')
  await view.findByText('已编辑 · 高风险')
  expect(view.getByText('读取文件')).toBeDefined()
  expect(view.getByText('执行命令')).toBeDefined()
  expect(view.getByText('docs/readme.md')).toBeDefined()
  expect(view.getByText('bun test')).toBeDefined()
  expect(view.getByText('已完成 · 2 步').closest('.fox-record-row')).not.toBeNull()
  expect(view.queryByText('工具调用')).toBeNull()
  expect(view.queryByText('write_file')).toBeNull()
})

test('运行详情的空状态只显示一个图标和一句话', () => {
  const view = render(<ActivityDetailPanel detail={null} goal={null} focusedEvidence={null} />)
  const empty = view.getByText('这段对话还没有运行记录。').parentElement!
  expect(empty.querySelectorAll('svg[aria-hidden="true"]')).toHaveLength(1)
  expect(empty.children).toHaveLength(2)
})
