export const COLOR_THEME_STORAGE_KEY = 'fox.preferences.colorTheme'

export const COLOR_THEMES = [
  { id: 'graphite', label: '石墨', description: '中性灰阶，墨色主按钮，紧凑利落' },
  { id: 'paper', label: '暖纸', description: '米白暖灰，Fox 蓝主色，圆润柔和' },
  { id: 'mist', label: '雾蓝', description: '冷调蓝灰，延续 Fox 经典配色' },
] as const

export type ColorTheme = (typeof COLOR_THEMES)[number]['id']

export const COLOR_THEME_DEFAULT: ColorTheme = 'graphite'

export function normalizeColorTheme(value: unknown): ColorTheme {
  return COLOR_THEMES.some((theme) => theme.id === value) ? (value as ColorTheme) : COLOR_THEME_DEFAULT
}

export function readColorTheme(): ColorTheme {
  if (typeof window === 'undefined') return COLOR_THEME_DEFAULT
  return normalizeColorTheme(window.localStorage.getItem(COLOR_THEME_STORAGE_KEY))
}

export function applyColorTheme(value: unknown): ColorTheme {
  const theme = normalizeColorTheme(value)
  if (typeof document !== 'undefined') {
    document.documentElement.dataset.colorTheme = theme
  }
  return theme
}

export function persistColorTheme(value: unknown): ColorTheme {
  const theme = applyColorTheme(value)
  if (typeof window !== 'undefined') {
    window.localStorage.setItem(COLOR_THEME_STORAGE_KEY, theme)
  }
  return theme
}

export function applyStoredColorTheme(): ColorTheme {
  return applyColorTheme(readColorTheme())
}
