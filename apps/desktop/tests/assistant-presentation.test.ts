import { describe, expect, test } from 'bun:test'
import { normalizeAssistantMarkdown } from '../src/features/conversations/model/assistant-presentation'

describe('assistant presentation normalization', () => {
  test('removes emoji and accidental prose indentation', () => {
    expect(normalizeAssistantMarkdown('  结果 ✅  已完成\n　下一步：验证')).toBe('结果 已完成\n下一步：验证')
  })

  test('preserves real Markdown list structure', () => {
    const markdown = '- 第一项\n  - 子项\n1. 第一步'
    expect(normalizeAssistantMarkdown(markdown)).toBe(markdown)
  })

  test('does not alter emoji or indentation inside code', () => {
    const markdown = '说明 🚀\n```ts\n  const status = "✅"\n```\n`🙂`'
    expect(normalizeAssistantMarkdown(markdown)).toBe('说明\n```ts\n  const status = "✅"\n```\n`🙂`')
  })
})
