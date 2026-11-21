import { afterEach, expect, test } from 'bun:test'
import { act, cleanup, renderHook } from '@testing-library/react'
import { useRightPanelNavigation } from '../src/features/chat/right-panel/navigation'

afterEach(cleanup)

test('右侧栏默认收起，展开时进入引导页', () => {
  const { result } = renderHook(() => useRightPanelNavigation())
  expect(result.current.collapsed).toBe(true)
  act(() => result.current.openGuide())
  expect(result.current.collapsed).toBe(false)
  expect(result.current.mode).toBeNull()
})

test('收起已打开的页面后清除当前页和最大化状态', () => {
  const { result } = renderHook(() => useRightPanelNavigation('files', false))
  act(() => result.current.setMaximized(true))
  act(() => result.current.collapse())
  expect(result.current.collapsed).toBe(true)
  expect(result.current.maximized).toBe(false)
  act(() => result.current.openGuide())
  expect(result.current.collapsed).toBe(false)
  expect(result.current.mode).toBeNull()
})
