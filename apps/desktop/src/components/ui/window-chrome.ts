/**
 * The custom window title bar (minimise / maximise / close) is drawn by the app
 * itself. Modal overlays (Sheet, Dialog, AlertDialog) must leave it usable:
 *
 * - The title bar publishes its height as `--fox-titlebar-height` on <html>
 *   while it is mounted; overlays and sheets start below it (0px when there is
 *   no title bar, e.g. browser preview or tests without the shell).
 * - The title bar opts back into pointer events (Radix sets
 *   `pointer-events: none` on <body> while a modal layer is open).
 * - A press on the title bar is not an "outside" interaction: it must not
 *   dismiss the open sheet/dialog. Shared Content components wrap their
 *   outside handlers with `keepOpenOnWindowChrome`.
 */
export const TITLEBAR_HEIGHT_VAR = '--fox-titlebar-height'
export const WINDOW_CHROME_ATTR = 'data-fox-window-chrome'

export function isWindowChromeTarget(target: EventTarget | null | undefined): boolean {
  const element = target as { closest?: (selector: string) => unknown } | null | undefined
  return Boolean(element && typeof element.closest === 'function' && element.closest(`[${WINDOW_CHROME_ATTR}]`))
}

type OutsideEvent = { target: EventTarget | null; preventDefault: () => void }

export function keepOpenOnWindowChrome<E extends OutsideEvent>(handler?: (event: E) => void) {
  return (event: E) => {
    if (isWindowChromeTarget(event.target)) {
      event.preventDefault()
      return
    }
    handler?.(event)
  }
}

/** Called by the title bar on mount; returns the cleanup. */
export function publishTitlebarHeight(px: number): () => void {
  if (typeof document === 'undefined') return () => {}
  const root = document.documentElement
  root.style.setProperty(TITLEBAR_HEIGHT_VAR, `${px}px`)
  return () => { root.style.removeProperty(TITLEBAR_HEIGHT_VAR) }
}
