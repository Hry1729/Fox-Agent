import { afterEach, expect, test } from 'bun:test'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { ResizeDivider } from '../src/features/chat/shell/fox-resize-divider'

afterEach(cleanup)

test.each([
  { label: '右栏', direction: -1 as const, x: 500, moves: [490, 450, 510], expected: [410, 450, 390] },
  { label: '左栏', direction: 1 as const, x: 500, moves: [510, 550, 490], expected: [410, 450, 390] },
])('$label 连续移动按指针起点和实际宽度计算，不依赖重新布局', ({ label, direction, x, moves, expected }) => {
  const widths: number[] = []
  let measurements = 0
  const view = render(<ResizeDivider className="divider" label={label} width={760} direction={direction}
    measure={() => { measurements++; return { width: 400, min: 280, max: 500 } }} onResize={(width) => widths.push(width)} />)
  const divider = view.getByRole('separator')
  divider.setPointerCapture = () => {}
  fireEvent.pointerDown(divider, { button: 0, pointerId: 1, clientX: x })
  expect(divider.dataset.resizing).toBe('true')
  for (const clientX of moves) fireEvent.pointerMove(divider, { pointerId: 1, clientX })
  expect(widths).toEqual(expected)
  expect(measurements).toBe(1)
  fireEvent.pointerUp(divider, { pointerId: 1, clientX: moves.at(-1) })
  expect(divider.dataset.resizing).toBeUndefined()
  fireEvent.pointerMove(divider, { pointerId: 1, clientX: 300 })
  expect(widths).toEqual(expected)
})

test('拖动超过范围时限制宽度，键盘从实际宽度调整', () => {
  const widths: number[] = []
  const view = render(<ResizeDivider className="divider" label="右栏" width={760} direction={-1}
    measure={() => ({ width: 400, min: 280, max: 450 })} onResize={(width) => widths.push(width)} />)
  const divider = view.getByRole('separator')
  divider.setPointerCapture = () => {}
  fireEvent.pointerDown(divider, { button: 0, pointerId: 1, clientX: 500 })
  fireEvent.pointerMove(divider, { pointerId: 1, clientX: 300 })
  fireEvent.pointerMove(divider, { pointerId: 1, clientX: 700 })
  fireEvent.pointerCancel(divider, { pointerId: 1 })
  fireEvent.keyDown(divider, { key: 'ArrowRight' })
  expect(widths).toEqual([450, 280, 392])
  expect(divider.dataset.resizing).toBeUndefined()
})
