// "查看错误详情" text (the backend stderr tail included) in every error entry:
// the timeline, the thread error card and the assistant message all render
// it through ErrorState. A tail line can be ~2000 bytes without spaces
// (paths, JSON) or CJK, so it wraps anywhere and the block scrolls instead of
// growing without bound. Inline style as well as the class, so the layout
// holds even where the stylesheet is not loaded.
export const ERROR_DIAGNOSTIC_CLASS = 'aui-error-diagnostic'

export const ERROR_DIAGNOSTIC_MAX_HEIGHT = '20rem'

export const ERROR_DIAGNOSTIC_STYLE = {
  whiteSpace: 'pre-wrap',
  overflowWrap: 'anywhere',
  wordBreak: 'break-word',
  maxHeight: ERROR_DIAGNOSTIC_MAX_HEIGHT,
  overflowY: 'auto',
  overflowX: 'hidden',
} as const
