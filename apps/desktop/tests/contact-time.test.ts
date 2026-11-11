import { describe, expect, test } from 'bun:test'
import { contactGroupTimeLabel, contactTimeGroups } from '../src/features/companions/chat-time'

describe('联系人消息时间分组', () => {
  test('相邻消息连续时只在组首显示，满15分钟重新显示', () => {
    const start = new Date(2026, 10, 11, 18, 3).getTime()
    const groups = contactTimeGroups([
      { id: 'user', createdAt: start },
      { id: 'reply', createdAt: start + 60_000 },
      { id: 'next', createdAt: start + 16 * 60_000 },
    ])
    expect([...groups.keys()]).toEqual(['user', 'next'])
  })
  test('跨天即使不足15分钟也开始新组，历史分页首条总有时间', () => {
    const start = new Date(2026, 10, 11, 23, 59).getTime()
    expect([...contactTimeGroups([{ id: 'last', createdAt: start }, { id: 'first', createdAt: start + 60_000 }]).keys()]).toEqual(['last', 'first'])
    expect([...contactTimeGroups([{ id: 'page', createdAt: start }]).keys()]).toEqual(['page'])
    expect(contactTimeGroups([]).size).toBe(0)
  })
  test('用本地日期标明今天、昨天和更早日期，不把历史记录标为今天', () => {
    const now = new Date(2026, 10, 11, 20)
    expect(contactGroupTimeLabel(new Date(2026, 10, 11, 18, 3).getTime(), now)).toBe('今天 18:03')
    expect(contactGroupTimeLabel(new Date(2026, 10, 10, 18, 3).getTime(), now)).toBe('昨天 18:03')
    expect(contactGroupTimeLabel(new Date(2026, 9, 1, 18, 3).getTime(), now)).toContain('10月1日')
    expect(contactGroupTimeLabel(new Date(2025, 9, 1, 18, 3).getTime(), now)).toContain('2025')
  })
})
