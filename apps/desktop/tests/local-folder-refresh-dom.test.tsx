import { afterEach, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
Object.defineProperty(Element.prototype, 'animate', { configurable: true, value: undefined })
const React = await import('react')
const { act, cleanup, render, waitFor } = await import('@testing-library/react')
const { LocalFolderDetailPage } = await import('../src/features/local-knowledge/local-folder-pages')
import type { LocalKnowledgeGateway, LocalKnowledgeCatalogFile } from '../src/features/local-knowledge/model'
afterEach(cleanup)
const source = { id: 'refresh-test', displayName: '测试目录', rootPath: 'D:/fixture', fileCount: 1, totalSize: 1, lastScannedAt: '', createdAt: '', updatedAt: '' }
const file = (name: string): LocalKnowledgeCatalogFile => ({ id: name, sourceId: source.id, sourceName: source.displayName, sourcePath: source.rootPath, absolutePath: `D:/fixture/${name}`, relativePath: name, name, extension: 'md', mimeType: 'text/markdown', sizeBytes: 1, modifiedAt: '' })

test('cached files display while scan is pending, then missing entries disappear and new files appear', async () => {
  let files = [file('deleted.md')]
  let release!: () => void
  const gateway = {
    listFileSources: async () => [source],
    listLocalFiles: async () => files,
    rescanFileSource: async () => { await new Promise<void>((resolve) => { release = resolve }); files = [file('added.md')]; return source },
  } as unknown as LocalKnowledgeGateway
  const page = render(<LocalFolderDetailPage gateway={gateway} sourceId={source.id} onBack={() => {}} renderPreview={() => null} />)
  await waitFor(() => expect(page.getByText('deleted.md')).toBeTruthy())
  await waitFor(() => expect(release).toBeDefined())
  expect(page.queryByText('正在读取文件夹…')).toBeNull()
  expect(page.getByText('deleted.md')).toBeTruthy()
  await act(async () => { release() })
  await waitFor(() => expect(page.getByText('added.md')).toBeTruthy())
  expect(page.queryByText('deleted.md')).toBeNull()
})

test('failed background scan keeps the previous list and exposes retry', async () => {
  const gateway = {
    listFileSources: async () => [source], listLocalFiles: async () => [file('kept.md')],
    rescanFileSource: async () => { throw Error('设备暂不可用') },
  } as unknown as LocalKnowledgeGateway
  const page = render(<LocalFolderDetailPage gateway={gateway} sourceId={source.id} onBack={() => {}} renderPreview={() => null} />)
  await waitFor(() => expect(page.getByRole('alert').textContent).toContain('设备暂不可用'))
  expect(page.getByText('kept.md')).toBeTruthy()
})

test('leaving before the deferred scan starts cancels it', async () => {
  let scans = 0
  const gateway = {
    listFileSources: async () => [source], listLocalFiles: async () => [file('kept.md')],
    rescanFileSource: async () => { scans++; return source },
  } as unknown as LocalKnowledgeGateway
  const page = render(<LocalFolderDetailPage gateway={gateway} sourceId={source.id} onBack={() => {}} renderPreview={() => null} />)
  await waitFor(() => expect(page.getByText('kept.md')).toBeTruthy())
  page.unmount()
  await new Promise((resolve) => setTimeout(resolve, 200))
  expect(scans).toBe(0)
})
