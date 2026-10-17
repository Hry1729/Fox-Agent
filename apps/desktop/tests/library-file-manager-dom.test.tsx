import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
Object.defineProperty(Element.prototype, 'animate', { configurable: true, value: undefined })
const React = await import('react')
const { cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { LibraryFileManager } = await import('../src/features/local-knowledge/library-file-manager')
afterEach(cleanup)

const files = [
  { id: 'keep-id-a', name: '甲.md', path: '甲.md', size: 20, status: 'indexed' },
  { id: 'keep-id-b', name: '乙.pdf', path: '乙.pdf', size: 30, status: 'error' },
  { id: 'nested-id', name: '预算.xlsx', path: '预算/预算.xlsx', size: 40, status: 'queued' },
]
describe('资料文件管理器', () => {
  test('bulk actions receive selected document IDs and never folder IDs', async () => {
    let removed: string[] = []
    let reprocessed: string[] = []
    const page = render(<LibraryFileManager files={files} onOpen={() => {}} onRemove={(items) => { removed = items.map((item) => item.id) }} onReprocess={(items) => { reprocessed = items.map((item) => item.id) }} />)
    fireEvent.click(page.getByLabelText('选择当前列表中的全部文件'))
    fireEvent.click(page.getByRole('button', { name: '重新处理', exact: true }))
    fireEvent.click(page.getByRole('button', { name: '从资料库移除', exact: true }))
    expect(reprocessed.sort()).toEqual(['keep-id-a', 'keep-id-b'])
    expect(removed.sort()).toEqual(['keep-id-a', 'keep-id-b'])
  })
  test('row selects without opening; preview preserves nested document ID', async () => {
    let opened = ''
    const page = render(<LibraryFileManager files={files} onOpen={(file) => { opened = file.id }} />)
    fireEvent.change(page.getByLabelText('搜索当前资料库文件'), { target: { value: '预算.xlsx' } })
    fireEvent.click(await page.findByTitle('预算/预算.xlsx'))
    expect(opened).toBe('')
    expect((page.getByLabelText('选择 预算.xlsx') as HTMLInputElement).checked).toBe(true)
    fireEvent.click(page.getByRole('button', { name: '预览 预算.xlsx' }))
    expect(opened).toBe('nested-id')
  })
  test('plain row selection replaces the selection and checkbox supports multiple files', () => {
    const page = render(<LibraryFileManager files={files} contentOnly onOpen={() => {}} />)
    fireEvent.click(page.getByTitle('甲.md'))
    fireEvent.click(page.getByTitle('甲.md'))
    expect((page.getByLabelText('选择 甲.md') as HTMLInputElement).checked).toBe(false)
    fireEvent.click(page.getByTitle('乙.pdf'))
    expect((page.getByLabelText('选择 甲.md') as HTMLInputElement).checked).toBe(false)
    expect((page.getByLabelText('选择 乙.pdf') as HTMLInputElement).checked).toBe(true)
    fireEvent.click(page.getByLabelText('选择 甲.md'))
    expect((page.getByLabelText('选择 甲.md') as HTMLInputElement).checked).toBe(true)
    expect((page.getByLabelText('选择 乙.pdf') as HTMLInputElement).checked).toBe(true)
    fireEvent.click(page.getByRole('button', { name: '预览 甲.md' }))
    expect((page.getByLabelText('选择 乙.pdf') as HTMLInputElement).checked).toBe(true)
  })
  test('read-only remote capabilities omit upload, remove and reprocess', async () => {
    const page = render(<LibraryFileManager files={files} onOpen={() => {}} onDownload={() => {}} />)
    fireEvent.click(page.getByLabelText('选择 甲.md'))
    expect(page.queryByRole('button', { name: '从资料库移除', exact: true })).toBeNull()
    expect(page.queryByRole('button', { name: '重新处理', exact: true })).toBeNull()
    expect(page.queryByRole('button', { name: '添加文件', exact: true })).toBeNull()
    expect(page.getByRole('button', { name: '下载原件', exact: true })).toBeTruthy()
    fireEvent.click(page.getByRole('button', { name: '网格视图', exact: true }))
    await waitFor(() => expect(page.container.querySelector('.fox-library-files.is-cards')).toBeTruthy())
  })
})
