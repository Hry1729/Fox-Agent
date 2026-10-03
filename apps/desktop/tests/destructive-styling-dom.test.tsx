import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, render, screen } = await import('@testing-library/react')
const { Button } = await import('../src/components/ui/button')
const { Badge } = await import('../src/components/ui/badge')
const { Alert } = await import('../src/components/ui/alert')
const { ContextMenuItem } = await import('../src/components/ui/context-menu')
const { DropdownMenuItem } = await import('../src/components/ui/dropdown-menu')
// Radix's portal stops mounting once another suite in the same process has used
// `react-dom/server`; rendering the menu content inline through the primitives
// keeps the assertion on the item classes and removes the cross-file coupling.
const { DropdownMenu: DropdownMenuPrimitive, ContextMenu: ContextMenuPrimitive } = await import('radix-ui')

afterEach(cleanup)

/** Radix mounts menu content into a portal, so wait for it instead of assuming
 *  the first commit already contains it. */
const menuItem = (name: string | RegExp) => waitFor(() => screen.getByRole('menuitem', { name }))

/** Radix popper mounts state after render; keep the noise out of the assertions. */
const mount = (element: React.ReactElement) => act(async () => { render(element) })

const classes = (element: Element) => element.className.split(/\s+/).filter(Boolean)

/**
 * A resting destructive control must read as a neutral surface with an explicit
 * danger boundary. The low-opacity red wash (`bg-destructive/10` and friends) is
 * what made whole buttons look pink, so it must not come back as a resting fill.
 */
const RESTING_RED_FILLS = ['bg-destructive/10', 'bg-destructive/20', 'bg-destructive/30']

function expectNeutralRestingSurface(element: Element, label: string) {
  const tokens = classes(element)
  for (const fill of RESTING_RED_FILLS) {
    expect({ label, fill, present: tokens.includes(fill) }).toEqual({ label, fill, present: false })
  }
  expect(tokens).toContain('text-destructive')
  expect(tokens.some((token) => token.startsWith('border-destructive/'))).toBe(true)
}

describe('public destructive styling', () => {
  test('the destructive button keeps a neutral surface in every state', () => {
    render(React.createElement(Button, { variant: 'destructive' }, '删除'))
    const button = screen.getByRole('button', { name: '删除' })
    expectNeutralRestingSurface(button, 'button')
    const tokens = classes(button)
    // Interaction still strengthens the danger colour, which is what carries the risk.
    expect(tokens.some((token) => token.startsWith('hover:bg-destructive/'))).toBe(true)
    expect(tokens.some((token) => token.startsWith('hover:border-destructive/'))).toBe(true)
    expect(tokens.some((token) => token.startsWith('focus-visible:ring-destructive/'))).toBe(true)
    expect(tokens.some((token) => token.startsWith('dark:hover:bg-destructive/'))).toBe(true)
    expect(tokens.some((token) => token.startsWith('dark:border-destructive/'))).toBe(true)
    // Disabled feedback comes from the shared base, not from a red fill.
    expect(tokens).toContain('disabled:opacity-50')
  })

  test('the destructive button stays neutral while disabled', () => {
    render(React.createElement(Button, { variant: 'destructive', disabled: true }, '删除'))
    const button = screen.getByRole('button', { name: '删除' }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expectNeutralRestingSurface(button, 'disabled button')
  })

  test('the destructive badge keeps the same neutral surface', () => {
    render(React.createElement(Badge, { variant: 'destructive' }, '已禁用'))
    expectNeutralRestingSurface(screen.getByText('已禁用'), 'badge')
  })

  test('the destructive alert keeps the same neutral surface', () => {
    render(React.createElement(Alert, { variant: 'destructive' }, React.createElement('span', null, '运行失败')))
    expectNeutralRestingSurface(screen.getByRole('alert'), 'alert')
  })

  test('a destructive dropdown item uses the neutral focus background with a danger outline', async () => {
    render(React.createElement(
      DropdownMenuPrimitive.Root,
      { open: true },
      React.createElement(DropdownMenuPrimitive.Trigger, null, '更多'),
      React.createElement(DropdownMenuPrimitive.Content, { forceMount: true },
        React.createElement(DropdownMenuItem, { variant: 'destructive' }, '删除项目')),
    ))
    const item = screen.getByRole('menuitem', { name: '删除项目' })
    expect(item.getAttribute('data-variant')).toBe('destructive')
    const tokens = classes(item)
    for (const fill of RESTING_RED_FILLS) expect(tokens).not.toContain(fill)
    // A hovered/focused destructive row keeps the shared neutral background and
    // marks the risk with a thin inset danger outline plus danger text.
    expect(tokens).toContain('focus:bg-accent')
    expect(tokens).toContain('data-[variant=destructive]:focus:text-destructive')
    expect(tokens).toContain('data-[variant=destructive]:focus:ring-inset')
    expect(tokens.some((token) => token.startsWith('data-[variant=destructive]:focus:ring-destructive/'))).toBe(true)
  })

  test('a destructive context menu item matches the dropdown contract', async () => {
    render(React.createElement(
      ContextMenuPrimitive.Root,
      { open: true },
      React.createElement(ContextMenuPrimitive.Trigger, null, React.createElement('div', null, '目标')),
      React.createElement(ContextMenuPrimitive.Content, { forceMount: true },
        React.createElement(ContextMenuItem, { variant: 'destructive' }, '移入回收站')),
    ))
    const item = screen.getByRole('menuitem', { name: '移入回收站' })
    expect(item.getAttribute('data-variant')).toBe('destructive')
    const tokens = classes(item)
    for (const fill of RESTING_RED_FILLS) expect(tokens).not.toContain(fill)
    expect(tokens).toContain('data-[variant=destructive]:focus:text-destructive')
    expect(tokens.some((token) => token.startsWith('data-[variant=destructive]:focus:ring-destructive/'))).toBe(true)
  })

  test('a non-destructive menu item carries no unconditional danger colour', async () => {
    render(React.createElement(
      DropdownMenuPrimitive.Root,
      { open: true },
      React.createElement(DropdownMenuPrimitive.Trigger, null, '更多'),
      React.createElement(DropdownMenuPrimitive.Content, { forceMount: true },
        React.createElement(DropdownMenuItem, null, '重命名对话')),
    ))
    const item = screen.getByRole('menuitem', { name: '重命名对话' })
    expect(item.getAttribute('data-variant')).toBe('default')
    const tokens = classes(item)
    // Danger is opt-in per item: the default variant carries no unconditional
    // danger colour and no danger fill.
    expect(tokens).not.toContain('text-destructive')
    expect(tokens.some((token) => token === 'text-destructive' || token.startsWith('bg-destructive'))).toBe(false)
  })
})
