export const TEXT_SCALE_STORAGE_KEY = 'fox.preferences.textScale'
export const TEXT_SCALE_MIN = 70
export const TEXT_SCALE_MAX = 140
export const TEXT_SCALE_DEFAULT = 100

export function normalizeTextScale(value: unknown): number {
  if (value === null || value === undefined || value === '') return TEXT_SCALE_DEFAULT
  const parsed = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(parsed)) return TEXT_SCALE_DEFAULT
  return Math.min(TEXT_SCALE_MAX, Math.max(TEXT_SCALE_MIN, Math.round(parsed)))
}

export function readTextScale(): number {
  if (typeof window === 'undefined') return TEXT_SCALE_DEFAULT
  return normalizeTextScale(window.localStorage.getItem(TEXT_SCALE_STORAGE_KEY))
}

export function applyTextScale(value: unknown): number {
  const scale = normalizeTextScale(value)
  if (typeof document !== 'undefined') {
    document.documentElement.style.setProperty('--fox-text-scale', String(scale / 100))
  }
  return scale
}

export function persistTextScale(value: unknown): number {
  const scale = applyTextScale(value)
  if (typeof window !== 'undefined') {
    window.localStorage.setItem(TEXT_SCALE_STORAGE_KEY, String(scale))
  }
  return scale
}

export function applyStoredTextScale(): number {
  return applyTextScale(readTextScale())
}
