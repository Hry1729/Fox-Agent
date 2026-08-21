/**
 * 判断是否可用媒体地址。
 * 注意：浏览器对 src="" 读取 img.src 会回落成当前页 URL（如 http://localhost:3006/），
 * src="/" 也会解析成站点根路径，都会触发 ResourceError。
 */
export function isUsableMediaSrc(src) {
  if (src == null) return false
  const value = String(src).trim()
  if (!value) return false
  if (value === '/' || value === '#' || value === 'about:blank') return false
  if (value === 'undefined' || value === 'null') return false
  // 误把站点根路径当头像
  if (/^https?:\/\/[^/]+\/?$/i.test(value)) return false
  return true
}

/** 不可用时返回 undefined，方便直接绑给 :src */
export function usableMediaSrc(src) {
  return isUsableMediaSrc(src) ? String(src).trim() : undefined
}
