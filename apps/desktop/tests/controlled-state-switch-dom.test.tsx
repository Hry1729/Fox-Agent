import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { act, cleanup, fireEvent, render, renderHook } = await import('@testing-library/react')
const { useControlledState } = await import('../src/hooks/use-controlled-state')
const { Switch, SwitchThumb, SwitchIcon, useSwitch } = await import('../src/components/animate-ui/primitives/base/switch')
afterEach(cleanup)

describe('controlled state ownership', () => {
  test('a rejected request preserves the controlled value and forwards callback arguments', () => {
    const calls: unknown[][] = []
    const details = { reason: 'save' }
    const { result } = renderHook(() => useControlledState({ value: false, defaultValue: true, onChange: (...args: [boolean, typeof details]) => calls.push(args) }))
    act(() => result.current[1](true, details))
    expect(result.current[0]).toBe(false)
    expect(calls).toEqual([[true, details]])
  })

  test('new controlled props are visible in the first layout effect, including falsy values', () => {
    const commits: number[] = []
    function Probe({ value }: { value: number }) {
      const [state] = useControlledState({ value, defaultValue: 9 })
      React.useLayoutEffect(() => { commits.push(state) }, [value])
      return null
    }
    const view = render(<Probe value={1} />)
    view.rerender(<Probe value={0} />)
    expect(commits).toEqual([1, 0])
  })

  test('uncontrolled values update locally and defaultValue is only an initial value', () => {
    const calls: string[] = []
    const { result, rerender } = renderHook(({ defaultValue }) => useControlledState({ defaultValue, onChange: (next: string) => calls.push(next) }), { initialProps: { defaultValue: 'initial' } })
    act(() => result.current[1]('edited'))
    rerender({ defaultValue: 'changed-default' })
    expect(result.current[0]).toBe('edited')
    expect(calls).toEqual(['edited'])
  })
})

function ContextProbe() {
  return <span data-testid="switch-context">{String(useSwitch().isChecked)}</span>
}
function Contents() {
  return <><SwitchThumb /><SwitchIcon position="left" transition={{ duration: 0 }}>on</SwitchIcon><SwitchIcon position="right" transition={{ duration: 0 }}>off</SwitchIcon><ContextProbe /></>
}
function expectState(container: HTMLElement, checked: boolean) {
  const button = container.querySelector('[role="switch"]')!
  expect(button.getAttribute('aria-checked')).toBe(String(checked))
  expect(button.getAttribute('data-state')).toBe(checked ? 'checked' : 'unchecked')
  expect(button.hasAttribute('data-checked')).toBe(checked)
  expect(container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.checked).toBe(checked)
  expect(container.querySelector('[data-testid="switch-context"]')!.textContent).toBe(String(checked))
}

describe('real Base/Motion switch', () => {
  test.each([false, true])('parent rejection keeps checked=%s aligned on repeated clicks', checked => {
    const requests: boolean[] = []
    const view = render(<Switch aria-label="save" checked={checked} onCheckedChange={next => requests.push(next)}><Contents /></Switch>)
    fireEvent.click(view.getByRole('switch', { name: 'save' }))
    expectState(view.container, checked)
    fireEvent.click(view.getByRole('switch', { name: 'save' }))
    expectState(view.container, checked)
    expect(requests).toEqual([!checked, !checked])
  })

  test('a delayed save failure keeps the old value; successful retry changes all representations together', async () => {
    let settle!: (success: boolean) => void
    const requests: boolean[] = []
    function SavingSwitch() {
      const [checked, setChecked] = React.useState(false)
      const [saving, setSaving] = React.useState(false)
      return <Switch aria-label="save" checked={checked} disabled={saving} onCheckedChange={async next => {
        requests.push(next)
        setSaving(true)
        const success = await new Promise<boolean>(resolve => { settle = resolve })
        if (success) setChecked(next)
        setSaving(false)
      }}><Contents /></Switch>
    }
    const view = render(<SavingSwitch />)
    fireEvent.click(view.getByRole('switch'))
    expectState(view.container, false)
    await act(async () => settle(false))
    expectState(view.container, false)
    fireEvent.click(view.getByRole('switch'))
    expectState(view.container, false)
    await act(async () => settle(true))
    expectState(view.container, true)
    expect(requests).toEqual([true, true])
  })

  test('uncontrolled clicks toggle both ways and canceled Base events keep every representation unchanged', () => {
    const detailsSeen: unknown[] = []
    const view = render(<Switch aria-label="local" defaultChecked={false} onCheckedChange={(_next, details) => detailsSeen.push(details)}><Contents /></Switch>)
    fireEvent.click(view.getByRole('switch'))
    expectState(view.container, true)
    fireEvent.click(view.getByRole('switch'))
    expectState(view.container, false)
    expect(detailsSeen).toHaveLength(2)
    view.rerender(<Switch aria-label="local" onCheckedChange={(_next, details) => details.cancel()}><Contents /></Switch>)
    fireEvent.click(view.getByRole('switch'))
    expectState(view.container, false)
  })

  test('disabled and readOnly switches do not request a change', () => {
    const requests: boolean[] = []
    const view = render(<Switch aria-label="locked" defaultChecked disabled onCheckedChange={next => requests.push(next)}><Contents /></Switch>)
    fireEvent.click(view.getByRole('switch'))
    expectState(view.container, true)
    view.rerender(<Switch aria-label="locked" defaultChecked readOnly onCheckedChange={next => requests.push(next)}><Contents /></Switch>)
    fireEvent.click(view.getByRole('switch'))
    expectState(view.container, true)
    expect(requests).toEqual([])
  })
})
