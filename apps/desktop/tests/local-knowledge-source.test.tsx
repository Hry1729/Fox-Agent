import { afterEach, expect, test } from 'bun:test'
import { cleanup, render } from '@testing-library/react'
import { LocalKnowledgeSourceText, localSourceRange } from '../src/features/local-knowledge/local-knowledge-source-text'
import { runtimeProcess } from '../src/features/chat/timeline/fox-runtime-process-model'
import type { RunEventRecord } from '../src/features/conversations/model/types'

afterEach(cleanup)

test('local citations retain their target and locator when search and read report the same chunk', () => {
  const source = { id: 'local:kb:chunk', source: 'local', title: '合同.md', knowledgeBaseId: 'kb', documentId: 'doc', chunkId: 'chunk', anchor: 'bytes:7-10', excerpt: '款' }
  const events = [1, 2].map(seq => ({ runId: 'run', seq, eventType: 'source.added', event: source, createdAt: seq })) as RunEventRecord[]
  expect(runtimeProcess(events).sources).toEqual([{
    id: 'local:kb:chunk', source: 'local', title: '合同.md', knowledgeBaseId: 'kb', documentId: 'doc', excerpt: '款',
    locator: { chunkId: 'chunk', anchor: 'bytes:7-10', excerpt: '款', page: undefined },
  }])
})

test('UTF-8 byte anchors highlight the exact Chinese source in the original text', () => {
  const text = '前言\n条款 A-7：押金 37,500 元\n附录'
  const start = new TextEncoder().encode('前言\n').length
  const excerpt = '条款 A-7：押金 37,500 元'
  const end = start + new TextEncoder().encode(excerpt).length
  const view = render(<LocalKnowledgeSourceText text={text} locator={{ anchor: `bytes:${start}-${end}`, excerpt }} />)
  const mark = view.getByLabelText('引用片段')
  expect(mark.tagName).toBe('MARK')
  expect(mark.textContent).toBe(excerpt)
  expect(view.container.textContent).toBe(text)
})

test('stale or invalid byte offsets use a matching excerpt and never claim a false location', () => {
  expect(localSourceRange('甲乙丙', { anchor: 'bytes:1-4', excerpt: '乙' })).toEqual({ start: 1, end: 2 })
  expect(localSourceRange('新前缀旧片段', { anchor: 'bytes:0-9', excerpt: '旧片段' })).toEqual({ start: 3, end: 6 })
  expect(localSourceRange('现有正文', { anchor: 'bytes:0-999', excerpt: '已删除的片段' })).toBeNull()
  const view = render(<LocalKnowledgeSourceText text="现有正文" locator={{ excerpt: '已删除的片段' }} />)
  expect(view.queryByLabelText('引用片段')).toBeNull()
  expect(view.getByRole('status').textContent).toContain('未能')
})
