// Keyboard primitives adapted from https://uiarc.dev/r/shortcut-recorder.json.
// Keep this module independent of React and the recorder so global listeners stay lightweight.
export type Platform = 'mac' | 'other'
export interface ShortcutToken { label: string; id: string; spoken: string }
export type ShortcutKeyEvent = Pick<KeyboardEvent, 'key' | 'code' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey'>

const MODIFIER_ORDER = ['mod', 'ctrl', 'alt', 'shift', 'meta'] as const
const MODIFIER_KEYS: Record<string, string> = { Meta: 'meta', OS: 'meta', Control: 'ctrl', Alt: 'alt', AltGraph: 'alt', Shift: 'shift' }
const CODE_KEYS: Record<string, string> = {
  Comma: ',', Period: '.', Slash: '/', Semicolon: ';', Quote: "'", BracketLeft: '[', BracketRight: ']', Backslash: '\\', Minus: '-', Equal: '=', Backquote: '`',
  Space: 'space', Enter: 'enter', NumpadEnter: 'enter', Escape: 'escape', Backspace: 'backspace', Delete: 'delete', Tab: 'tab',
  ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right', Home: 'home', End: 'end', PageUp: 'pageup', PageDown: 'pagedown',
}
const isModifier = (part: string) => (MODIFIER_ORDER as readonly string[]).includes(part)

export function keyId({ key, code }: Pick<KeyboardEvent, 'key' | 'code'>): string | null {
  if (MODIFIER_KEYS[key]) return MODIFIER_KEYS[key]
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase()
  if (/^Digit\d$/.test(code)) return code.slice(5)
  if (/^Numpad\d$/.test(code)) return code.slice(6)
  if (CODE_KEYS[code]) return CODE_KEYS[code]
  if (/^F(?:[1-9]|1\d|2[0-4])$/i.test(key)) return key.toLowerCase()
  if (['CapsLock', 'Fn', 'Dead', 'Unidentified', 'Process'].includes(key)) return null
  // Synthetic events may have no code. Preserve named keys as well as printable ones.
  if (CODE_KEYS[key]) return CODE_KEYS[key]
  return key.length === 1 ? key.toLowerCase() : null
}

export function splitShortcut(shortcut: string) {
  const parts = shortcut.toLowerCase().split('+').map((part) => part.trim()).filter(Boolean)
  return { mods: new Set(parts.filter(isModifier)), key: parts.filter((part) => !isModifier(part)).pop() ?? '' }
}

export function normalizeShortcut(shortcut: string, platform: Platform) {
  const { mods, key } = splitShortcut(shortcut)
  if (platform === 'mac' && mods.delete('meta')) mods.add('mod')
  if (platform === 'other' && mods.delete('ctrl')) mods.add('mod')
  return [...MODIFIER_ORDER.filter((mod) => mods.has(mod)), key].filter(Boolean).join('+')
}

export function shortcutFromEvent(event: ShortcutKeyEvent, platform: Platform) {
  const key = keyId(event)
  if (!key || ['meta', 'ctrl', 'alt', 'shift'].includes(key)) return null
  const mods: string[] = []
  if (platform === 'mac' ? event.metaKey : event.ctrlKey) mods.push('mod')
  if (platform === 'mac' && event.ctrlKey) mods.push('ctrl')
  if (event.altKey) mods.push('alt')
  if (event.shiftKey) mods.push('shift')
  if (platform === 'other' && event.metaKey) mods.push('meta')
  return normalizeShortcut([...mods, key].join('+'), platform)
}

export function matchesShortcut(event: ShortcutKeyEvent, shortcut: string, platform: Platform) {
  const pressed = shortcutFromEvent(event, platform)
  return Boolean(pressed && pressed === normalizeShortcut(shortcut, platform))
}

const MAC_KEYS: Record<string, [string, string]> = { enter: ['↩', 'Return'], backspace: ['⌫', 'Delete'], delete: ['⌦', 'Forward delete'], tab: ['⇥', 'Tab'], escape: ['esc', 'Escape'] }
const SHARED_KEYS: Record<string, [string, string]> = {
  space: ['Space', 'Space'], enter: ['Enter', 'Enter'], escape: ['Esc', 'Escape'], backspace: ['Backspace', 'Backspace'], delete: ['Del', 'Delete'], tab: ['Tab', 'Tab'],
  up: ['↑', 'Up arrow'], down: ['↓', 'Down arrow'], left: ['←', 'Left arrow'], right: ['→', 'Right arrow'], home: ['Home', 'Home'], end: ['End', 'End'], pageup: ['PgUp', 'Page up'], pagedown: ['PgDn', 'Page down'],
  ',': [',', 'Comma'], '.': ['.', 'Period'], '/': ['/', 'Slash'], ';': [';', 'Semicolon'], "'": ["'", 'Quote'], '[': ['[', 'Left bracket'], ']': [']', 'Right bracket'], '\\': ['\\', 'Backslash'], '-': ['-', 'Minus'], '=': ['=', 'Equals'], '`': ['`', 'Backtick'],
}

export function shortcutTokens(shortcut: string, platform: Platform): ShortcutToken[] {
  const { mods, key } = splitShortcut(normalizeShortcut(shortcut, platform))
  const tokens: ShortcutToken[] = []
  const add = (label: string, id: string, spoken: string) => tokens.push({ label, id, spoken })
  if (platform === 'mac') {
    if (mods.has('ctrl')) add('⌃', 'ctrl', 'Control')
    if (mods.has('alt')) add('⌥', 'alt', 'Option')
    if (mods.has('shift')) add('⇧', 'shift', 'Shift')
    if (mods.has('mod')) add('⌘', 'meta', 'Command')
  } else {
    if (mods.has('mod')) add('Ctrl', 'ctrl', 'Control')
    if (mods.has('alt')) add('Alt', 'alt', 'Alt')
    if (mods.has('shift')) add('Shift', 'shift', 'Shift')
    if (mods.has('meta')) add('Win', 'meta', 'Windows')
  }
  if (key) {
    const named = (platform === 'mac' ? MAC_KEYS[key] : undefined) ?? SHARED_KEYS[key]
    add(named?.[0] ?? key.toUpperCase(), key, named?.[1] ?? key.toUpperCase())
  }
  return tokens
}

export function formatShortcut(shortcut: string, platform: Platform) {
  const tokens = shortcutTokens(shortcut, platform)
  return { text: tokens.map((token) => token.label).join(platform === 'mac' ? '' : '+'), spoken: tokens.map((token) => token.spoken).join(' ') }
}

export function detectShortcutPlatform(): Platform {
  if (typeof navigator === 'undefined') return 'other'
  const hint = (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ?? navigator.platform ?? navigator.userAgent
  return /mac|iphone|ipad|ipod/i.test(hint) ? 'mac' : 'other'
}

export const SHORTCUT_ACTIONS = [
  { id: 'new-conversation', label: '新建对话', description: '使用当前助手开始一段新对话', defaultShortcut: 'mod+n' },
  { id: 'add-project', label: '添加项目', description: '选择本机文件夹并加入工作台', defaultShortcut: 'mod+o' },
  { id: 'open-settings', label: '打开设置', description: '打开应用与外观设置', defaultShortcut: 'mod+,' },
  { id: 'toggle-sidebar', label: '切换侧栏', description: '展开或收起左侧导航', defaultShortcut: 'mod+b' },
  { id: 'search', label: '搜索', description: '打开当前工作区的搜索', defaultShortcut: 'mod+k' },
] as const
export type ShortcutActionId = (typeof SHORTCUT_ACTIONS)[number]['id']
export type ShortcutPreferences = Record<ShortcutActionId, string | null>
export const SHORTCUT_STORAGE_KEY = 'fox.preferences.shortcuts.v1'
export const SHORTCUT_CHANGED_EVENT = 'fox:shortcuts-changed'
export const DEFAULT_SHORTCUT_PREFERENCES: Readonly<ShortcutPreferences> = Object.freeze(Object.fromEntries(SHORTCUT_ACTIONS.map((action) => [action.id, action.defaultShortcut])) as ShortcutPreferences)

export function isValidShortcut(shortcut: unknown, platform: Platform = detectShortcutPlatform()): shortcut is string {
  if (typeof shortcut !== 'string' || shortcut.length > 80) return false
  const parts = shortcut.toLowerCase().split('+').map((part) => part.trim())
  const keys = parts.filter((part) => !isModifier(part))
  if (keys.length !== 1 || !keys[0] || parts.some((part) => !part)) return false
  const key = keys[0]
  if (!/^[a-z0-9]$/.test(key) && !/^f(?:[1-9]|1\d|2[0-4])$/.test(key) && !Object.values(CODE_KEYS).includes(key)) return false
  const { mods } = splitShortcut(normalizeShortcut(shortcut, platform))
  return /^f(?:[1-9]|1\d|2[0-4])$/.test(key) || ['mod', 'ctrl', 'alt', 'meta'].some((mod) => mods.has(mod))
}

/** Malformed data falls back per action; explicit null keeps an action disabled. */
export function normalizeShortcutPreferences(value: unknown, platform: Platform = detectShortcutPlatform()): ShortcutPreferences {
  const source = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
  const next = { ...DEFAULT_SHORTCUT_PREFERENCES }
  for (const action of SHORTCUT_ACTIONS) {
    const candidate = source[action.id]
    if (candidate === null) next[action.id] = null
    else if (isValidShortcut(candidate, platform)) next[action.id] = normalizeShortcut(candidate, platform)
  }
  // Prioritise explicit custom bindings over fallback defaults if a partial stored record collides.
  const ordered = [...SHORTCUT_ACTIONS].sort((a, b) => Number(Object.hasOwn(source, b.id)) - Number(Object.hasOwn(source, a.id)))
  const seen = new Set<string>()
  for (const action of ordered) {
    const shortcut = next[action.id]
    if (shortcut && seen.has(shortcut)) next[action.id] = null
    else if (shortcut) seen.add(shortcut)
  }
  return next
}

export function readShortcutPreferences(): ShortcutPreferences {
  try {
    const raw = typeof window === 'undefined' ? null : window.localStorage.getItem(SHORTCUT_STORAGE_KEY)
    return normalizeShortcutPreferences(raw ? JSON.parse(raw) : null)
  } catch { return { ...DEFAULT_SHORTCUT_PREFERENCES } }
}

/** Throws on write failure so the settings page never claims an unsaved binding is active. */
export function persistShortcutPreferences(preferences: ShortcutPreferences): ShortcutPreferences {
  const next = normalizeShortcutPreferences(preferences)
  if (typeof window !== 'undefined') {
    window.localStorage.setItem(SHORTCUT_STORAGE_KEY, JSON.stringify(next))
    window.dispatchEvent(new Event(SHORTCUT_CHANGED_EVENT))
  }
  return next
}

export function findShortcutConflicts(actionId: ShortcutActionId, shortcut: string | null, preferences = readShortcutPreferences(), platform = detectShortcutPlatform()) {
  if (!shortcut) return []
  const normal = normalizeShortcut(shortcut, platform)
  return SHORTCUT_ACTIONS.filter((action) => action.id !== actionId && preferences[action.id] && normalizeShortcut(preferences[action.id]!, platform) === normal)
}

/** One atomic write transfers a shortcut and explicitly disables its previous owner. */
export function setShortcutBinding(actionId: ShortcutActionId, shortcut: string | null, options: { replaceConflicts?: boolean } = {}) {
  const platform = detectShortcutPlatform()
  if (shortcut !== null && !isValidShortcut(shortcut, platform)) throw new Error('请使用 Ctrl、Cmd 或 Alt 加一个按键，或使用功能键')
  const preferences = readShortcutPreferences()
  const conflicts = findShortcutConflicts(actionId, shortcut, preferences, platform)
  if (conflicts.length && !options.replaceConflicts) throw new Error(`快捷键已用于“${conflicts[0].label}”`)
  for (const conflict of conflicts) preferences[conflict.id] = null
  preferences[actionId] = shortcut === null ? null : normalizeShortcut(shortcut, platform)
  return persistShortcutPreferences(preferences)
}

export function restoreDefaultShortcuts() {
  return persistShortcutPreferences({ ...DEFAULT_SHORTCUT_PREFERENCES })
}

export function getShortcutBinding(actionId: ShortcutActionId): string | null {
  return readShortcutPreferences()[actionId]
}

export type ShortcutActionEvent = ShortcutKeyEvent & Partial<Pick<KeyboardEvent, 'defaultPrevented' | 'repeat' | 'isComposing' | 'keyCode' | 'target'>>
/** Exact modifiers; suppress IME, repeats, handled events, and any focused shortcut recorder. */
export function matchesShortcutAction(event: ShortcutActionEvent, actionId: ShortcutActionId, preferences = readShortcutPreferences(), platform = detectShortcutPlatform()): boolean {
  if (event.defaultPrevented || event.repeat || event.isComposing || event.keyCode === 229 || event.key === 'Process') return false
  const target = event.target as Element | null | undefined
  if (typeof target?.closest === 'function' && target.closest('[data-shortcut-recorder]')) return false
  const shortcut = preferences[actionId]
  return Boolean(shortcut && matchesShortcut(event, shortcut, platform))
}
