// sandbox.md 13.26 #1: modal Sheet / Dialog overlays must not cover the custom
// window title bar. The title bar publishes --fox-titlebar-height, overlays and
// sheets start below it, the bar re-enables pointer events (Radix locks
// <body>), and a press on the bar does not dismiss the open layer.
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { readFileSync } from 'node:fs'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render } = await import('@testing-library/react')
const { MemoizedWindowTitlebar, WINDOW_TITLEBAR_HEIGHT_PX } = await import('../src/features/chat/shell/fox-window-titlebar')
const { Sheet, SheetContent, SheetTitle, SheetDescription } = await import('../src/components/ui/sheet')
const { Dialog, DialogContent, DialogTitle, DialogDescription } = await import('../src/components/ui/dialog')
const { AlertDialog, AlertDialogContent, AlertDialogTitle, AlertDialogDescription } = await import('../src/components/ui/alert-dialog')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { isWindowChromeTarget } = await import('../src/components/ui/window-chrome')

afterEach(() => cleanup())

const noop = () => {}
const titlebar = () => React.createElement(MemoizedWindowTitlebar, { leftSidebarCollapsed: false, onNewChat: noop, onOpenProject: noop, onSettings: noop, onAbout: noop, onToggleSidebar: noop, onZoom: noop })
const windowButtons = () => ['最小化', '最大化', '关闭'].map((label) => {
  const button = document.querySelector(`.fox-window-controls [aria-label="${label}"]`) as HTMLElement | null
  if (!button) throw new Error(`no window button ${label}`)
  return button
})
const TOP = 'top-[var(--fox-titlebar-height,0px)]'

function mountWith(layer: 'sheet' | 'dialog' | 'alert') {
  const content = layer === 'sheet'
    ? React.createElement(Sheet, { open: true }, React.createElement(SheetContent, { side: 'right' }, React.createElement(SheetTitle, null, '编辑服务'), React.createElement(SheetDescription, null, 'd')))
    : layer === 'dialog'
      ? React.createElement(Dialog, { open: true }, React.createElement(DialogContent, null, React.createElement(DialogTitle, null, '添加服务'), React.createElement(DialogDescription, null, 'd')))
      : React.createElement(AlertDialog, { open: true }, React.createElement(AlertDialogContent, null, React.createElement(AlertDialogTitle, null, '删除服务'), React.createElement(AlertDialogDescription, null, 'd')))
  return render(React.createElement(TooltipProvider, null, titlebar(), content))
}

describe('window title bar stays usable under modal layers', () => {
  test('the title bar publishes its height and marks itself as window chrome', () => {
    render(React.createElement(TooltipProvider, null, titlebar()))
    expect(document.documentElement.style.getPropertyValue('--fox-titlebar-height')).toBe(`${WINDOW_TITLEBAR_HEIGHT_PX}px`)
    const header = document.querySelector('.fox-window-titlebar') as HTMLElement
    expect(header.hasAttribute('data-fox-window-chrome')).toBe(true)
    for (const button of windowButtons()) expect(isWindowChromeTarget(button)).toBe(true)
    cleanup()
    expect(document.documentElement.style.getPropertyValue('--fox-titlebar-height')).toBe('')
  })

  test('the title bar opts back into pointer events (CSS)', () => {
    const css = readFileSync(new URL('../src/styles/workbench.css', import.meta.url), 'utf8')
    const rule = css.slice(css.indexOf('.fox-window-titlebar {'), css.indexOf('}', css.indexOf('.fox-window-titlebar {')))
    expect(rule).toContain('pointer-events: auto')
  })

  for (const layer of ['sheet', 'dialog', 'alert'] as const) {
    test(`${layer}: overlay and content start below the title bar; window buttons are not covered`, async () => {
      mountWith(layer)
      const slot = layer === 'sheet' ? 'sheet' : layer === 'dialog' ? 'dialog' : 'alert-dialog'
      const overlay = document.querySelector(`[data-slot="${slot}-overlay"]`) as HTMLElement
      expect(overlay).toBeTruthy()
      expect(overlay.className).toContain(TOP)
      expect(overlay.className.split(/\s+/)).not.toContain('inset-0')
      const content = document.querySelector(`[data-slot="${slot}-content"]`) as HTMLElement
      expect(content.className).toContain('var(--fox-titlebar-height')
      // No overlay or content is inside the title bar, and the buttons stay in the document.
      for (const button of windowButtons()) {
        expect(overlay.contains(button)).toBe(false)
        expect(content.contains(button)).toBe(false)
      }
    })
  }

  test('pressing minimise / maximise / close does not dismiss an open sheet or dialog', async () => {
    for (const layer of ['sheet', 'dialog'] as const) {
      let open = true
      const Harness = () => {
        const [isOpen, setOpen] = React.useState(true)
        open = isOpen
        const onOpenChange = (next: boolean) => setOpen(next)
        return React.createElement(TooltipProvider, null, titlebar(), layer === 'sheet'
          ? React.createElement(Sheet, { open: isOpen, onOpenChange }, React.createElement(SheetContent, null, React.createElement(SheetTitle, null, 't'), React.createElement(SheetDescription, null, 'd')))
          : React.createElement(Dialog, { open: isOpen, onOpenChange }, React.createElement(DialogContent, null, React.createElement(DialogTitle, null, 't'), React.createElement(DialogDescription, null, 'd'))))
      }
      render(React.createElement(Harness))
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
      for (const button of windowButtons()) {
        await act(async () => {
          fireEvent.pointerDown(button, { button: 0, pointerType: 'mouse' })
          fireEvent.pointerUp(button, { button: 0, pointerType: 'mouse' })
          fireEvent.click(button)
          await new Promise((resolve) => setTimeout(resolve, 10))
        })
        expect(open).toBe(true)
      }
      // A press elsewhere (the overlay) still dismisses it.
      const overlay = document.querySelector(`[data-slot="${layer}-overlay"]`) as HTMLElement
      await act(async () => {
        fireEvent.pointerDown(overlay, { button: 0, pointerType: 'mouse' })
        fireEvent.click(overlay)
        await new Promise((resolve) => setTimeout(resolve, 10))
      })
      expect(open).toBe(false)
      cleanup()
    }
  })
})
