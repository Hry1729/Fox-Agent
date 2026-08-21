import { describe, expect, test } from 'bun:test'
import { resolveYuxiLoginReturn, resolveYuxiServiceReturn } from '../src/features/knowledge/knowledge-navigation'

describe('knowledge navigation', () => {
  test('returns login flows to their origin', () => {
    expect(resolveYuxiLoginReturn('knowledge')).toBe('knowledge')
    expect(resolveYuxiLoginReturn('agents')).toBe('agents')
    expect(resolveYuxiLoginReturn('settings-yuxi')).toBe('settings-yuxi')
    expect(resolveYuxiLoginReturn(null)).toBe('settings-yuxi')
  })

  test('returns service configuration to its origin', () => {
    expect(resolveYuxiServiceReturn('knowledge')).toBe('knowledge')
    expect(resolveYuxiServiceReturn('login')).toBe('login')
    expect(resolveYuxiServiceReturn('settings-yuxi')).toBe('settings-yuxi')
    expect(resolveYuxiServiceReturn(null)).toBe('settings-yuxi')
  })
})
