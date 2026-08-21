const DICEBEAR_GLYPHS_AVATAR_BASE_URL = 'https://api.dicebear.com/10.x/glyphs/svg'

const AVATAR_BACKGROUND_TOKENS = [
  {
    background: 'linear-gradient(135deg, var(--el-color-primary), var(--el-color-primary-light-3))',
    color: '#fff'
  },
  {
    background: 'linear-gradient(135deg, #8b5cf6, #c4b5fd)',
    color: '#fff'
  },
  {
    background: 'linear-gradient(135deg, #0891b2, #67e8f9)',
    color: '#fff'
  },
  {
    background: 'linear-gradient(135deg, var(--el-color-warning), var(--el-color-warning-light-3))',
    color: '#fff'
  },
  {
    background: 'linear-gradient(135deg, var(--el-color-danger), var(--el-color-danger-light-3))',
    color: '#fff'
  }
]

const normalizeSeed = (id: string | number) => {
  if (id === null || id === undefined || String(id).trim() === '') {
    throw new Error('generatePixelAvatar requires an id')
  }
  return String(id).trim()
}

/** 根据 ID 生成 Dicebear 像素头像 URL */
export const generatePixelAvatar = (id: string | number) => {
  const seed = normalizeSeed(id)
  return `${DICEBEAR_GLYPHS_AVATAR_BASE_URL}?seed=${encodeURIComponent(seed)}`
}

/** 头像文字回退 */
export const getAvatarInitials = (name?: string, kind: 'user' | 'agent' = 'user') => {
  const fallback = kind === 'agent' ? '智能' : '用户'
  const normalizedName = String(name || '').trim()
  if (!normalizedName) return fallback
  return Array.from(normalizedName).slice(0, 2).join('')
}

export const getAvatarColorIndex = (seed?: string | number) => {
  const normalizedSeed = String(seed || '').trim()
  const value = normalizedSeed || 'avatar'
  let hash = 0
  for (const char of value) {
    hash = (hash * 31 + char.codePointAt(0)!) >>> 0
  }
  return hash % AVATAR_BACKGROUND_TOKENS.length
}

export const getAvatarFallbackStyle = (seed?: string | number) =>
  AVATAR_BACKGROUND_TOKENS[getAvatarColorIndex(seed)]
