/** 文件图标 URL 解析（简化版，Yuxi 的 resolveFileIconUrl 依赖大量 icon 资源，这里用 emoji 占位） */
export const resolveFileIconUrl = (name, options = {}) => {
  // MVP：返回空字符串，FileTypeIcon 会显示 fallback 文字
  // 后续可接入 Art Design Pro 的文件图标体系
  return ''
}
