/**
 * Enter-key resolution for the chat prompt textarea.
 *
 * `fox.preferences.sendKey` names the key that **sends**:
 *
 * - `'enter'` (default) — Enter sends; Ctrl/Cmd+Enter and Shift+Enter insert a newline.
 * - `'mod-enter'` — Ctrl/Cmd+Enter sends; Enter and Shift+Enter insert a newline.
 *
 * Compatibility: `'mod-enter'` is the legacy stored value and keeps its original
 * meaning, so a profile that already selected "Ctrl / Cmd + Enter 发送" is not
 * silently inverted by this change. Ctrl/Cmd+Enter can therefore never both send
 * and break the line at the same time — the preference decides which one it does,
 * and the settings page states the trade-off explicitly.
 */

export const PROMPT_SEND_SHORTCUT_KEY = 'fox.preferences.sendKey'

export type PromptSendShortcut = 'enter' | 'mod-enter'

export const DEFAULT_PROMPT_SEND_SHORTCUT: PromptSendShortcut = 'enter'

/** What the textarea should do with the key event it just received. */
export type PromptEnterAction = 'send' | 'newline' | 'default'

export function isPromptSendShortcut(value: unknown): value is PromptSendShortcut {
  return value === 'enter' || value === 'mod-enter'
}

/** Unknown, missing and malformed stored values fall back to the default. */
export function normalizePromptSendShortcut(value: unknown): PromptSendShortcut {
  return isPromptSendShortcut(value) ? value : DEFAULT_PROMPT_SEND_SHORTCUT
}

export function readPromptSendShortcut(
  storage?: { getItem: (key: string) => string | null } | null,
): PromptSendShortcut {
  const store = storage ?? (typeof window === 'undefined' ? null : window.localStorage)
  if (!store) return DEFAULT_PROMPT_SEND_SHORTCUT
  try {
    return normalizePromptSendShortcut(store.getItem(PROMPT_SEND_SHORTCUT_KEY))
  } catch {
    return DEFAULT_PROMPT_SEND_SHORTCUT
  }
}

export interface PromptEnterKeyEvent {
  key: string
  shiftKey?: boolean
  ctrlKey?: boolean
  metaKey?: boolean
  altKey?: boolean
  /** `KeyboardEvent.isComposing` — an IME candidate window still owns the key. */
  isComposing?: boolean
  /** IME confirmation keys report 229 on WebKit-derived engines and older Chromium. */
  keyCode?: number
}

/**
 * True while an IME composition owns the Enter key. Confirming a Chinese/Japanese
 * candidate must never submit the draft.
 */
export function promptEnterIsComposing(event: PromptEnterKeyEvent) {
  return event.isComposing === true || event.keyCode === 229
}

/**
 * Alt+Enter and every non-Enter key are left to the browser (`'default'`), so
 * platform behaviour and the slash/mention menus keep working untouched.
 */
export function resolvePromptEnterAction(
  event: PromptEnterKeyEvent,
  shortcut: PromptSendShortcut,
): PromptEnterAction {
  if (event.key !== 'Enter' || event.altKey) return 'default'
  if (promptEnterIsComposing(event)) return 'default'
  if (event.shiftKey) return 'newline'
  const modifier = event.ctrlKey === true || event.metaKey === true
  if (shortcut === 'mod-enter') return modifier ? 'send' : 'newline'
  return modifier ? 'newline' : 'send'
}

export interface PromptNewlineInsertion {
  value: string
  /** Caret offset after the inserted newline. */
  caret: number
}

function clampIndex(index: number, length: number) {
  if (!Number.isFinite(index)) return length
  return Math.min(Math.max(Math.trunc(index), 0), length)
}

/**
 * Replaces the current selection with a single newline. Kept pure so the DOM path
 * and its tests share one definition: replacing a selection is a deliberate
 * "break the line here" gesture, not an append.
 */
export function insertPromptNewline(
  value: string,
  selectionStart: number,
  selectionEnd: number,
): PromptNewlineInsertion {
  const text = typeof value === 'string' ? value : ''
  const first = clampIndex(selectionStart, text.length)
  const second = clampIndex(selectionEnd, text.length)
  const from = Math.min(first, second)
  const to = Math.max(first, second)
  return { value: `${text.slice(0, from)}\n${text.slice(to)}`, caret: from + 1 }
}
