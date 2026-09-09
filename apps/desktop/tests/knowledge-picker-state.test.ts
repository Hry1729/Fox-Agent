import { describe, expect, test } from 'bun:test'
import { filterKnowledgePickerItems, knowledgePickerStatus, localKnowledgePickerState } from '../src/features/chat/knowledge-picker-state'

describe('knowledge picker', () => {
  test('keyword-ready libraries are selectable without a vector generation', () => {
    expect(localKnowledgePickerState({ documentCount: 1, activeJobStatus: null, textIndexReady: true, vectorIndexReady: false })).toEqual({ available: true, status: '可用 · 关键词检索', reason: '' })
    expect(localKnowledgePickerState({ documentCount: 2, activeJobStatus: 'running', textIndexReady: true }).available).toBe(true)
  })
  test('imported files are distinguished from a ready index and an empty library', () => {
    const pending = localKnowledgePickerState({ documentCount: 1, activeJobStatus: 'completed', textIndexReady: false })
    expect(pending.available).toBe(false)
    expect(pending.status).toBe('待解析')
    expect(pending.reason).toContain('文档已导入')
    expect(localKnowledgePickerState({ documentCount: 0, activeJobStatus: null }).status).toBe('待导入')
  })
  const items = [
    { source: 'local', name: 'Fox 使用指南', description: '常用操作教程', unavailable: true },
    { source: 'local', name: 'ceshi', description: '29 个文档' },
    { source: 'remote', name: 'ceshi', description: '远程资料' },
  ]
  test('available libraries come first without mutating input', () => {
    expect(filterKnowledgePickerItems(items, 'local', '').map(item => item.name)).toEqual(['ceshi', 'Fox 使用指南'])
    expect(items[0].name).toBe('Fox 使用指南')
  })
  test('search is source-scoped, trimmed and case insensitive', () => {
    expect(filterKnowledgePickerItems(items, 'local', ' CESHI ')).toEqual([items[1]])
    expect(filterKnowledgePickerItems(items, 'local', '教程')).toEqual([items[0]])
    expect(filterKnowledgePickerItems(items, 'remote', '教程')).toEqual([])
  })
  test('status reflects usable index, not simply a completed job', () => {
    expect(knowledgePickerStatus('completed', true)).toBe('已索引')
    expect(knowledgePickerStatus('completed', false)).toBe('待导入')
    expect(knowledgePickerStatus(null, false)).toBe('待导入')
    expect(knowledgePickerStatus('RUNNING', true)).toBe('处理中')
    expect(knowledgePickerStatus('failed', false)).toBe('处理失败')
  })
})
