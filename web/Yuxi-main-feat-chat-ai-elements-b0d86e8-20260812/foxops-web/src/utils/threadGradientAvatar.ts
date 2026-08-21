const hashSeed = (value: unknown) => {
  const text = String(value || 'thread-avatar')
  let hash = 2166136261
  for (const char of text) {
    hash ^= char.codePointAt(0) || 0
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

const createSeededRandom = (seed: number) => {
  let state = seed || 0x6d2b79f5
  return () => {
    state += 0x6d2b79f5
    let value = state
    value = Math.imul(value ^ (value >>> 15), value | 1)
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61)
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296
  }
}

const hsl = (hue: number, saturation: number, lightness: number) =>
  `hsl(${Math.round(hue)} ${Math.round(saturation)}% ${Math.round(lightness)}%)`

/** 根据会话标识生成稳定的随机渐变，刷新后保持一致。 */
export const createThreadGradientStyle = (threadId: unknown) => {
  const random = createSeededRandom(hashSeed(threadId))
  const angle = Math.round(random() * 359)
  const baseHue = Math.round(random() * 359)
  const secondaryHue = (baseHue + 28 + random() * 54) % 360
  const secondaryStop = 58 + Math.round(random() * 30)
  const primaryColor = hsl(baseHue, 46 + random() * 12, 80 + random() * 7)
  const secondaryColor = hsl(secondaryHue, 44 + random() * 14, 78 + random() * 8)

  return {
    background: `linear-gradient(${angle}deg, ${primaryColor} 0%, ${secondaryColor} ${secondaryStop}%)`,
    color: 'transparent'
  }
}
