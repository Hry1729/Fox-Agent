type FileRow = { is_folder?: boolean; type?: string; status?: string } | null

const normalizeStatus = (row: FileRow) => String(row?.status || '').toLowerCase()

/** 文件是否可首次入库（已解析 / 入库失败） */
export const canIndexFile = (row: FileRow) => {
  if (!row || row.is_folder || row.type === 'folder') return false
  const status = normalizeStatus(row)
  return status === 'parsed' || status === 'error_indexing'
}

/** 文件是否可重新入库（已入库完成） */
export const canReindexFile = (row: FileRow) => {
  if (!row || row.is_folder || row.type === 'folder') return false
  const status = normalizeStatus(row)
  return status === 'done' || status === 'indexed' || status === 'completed'
}

/** 是否展示入库/重新入库按钮 */
export const canShowIndexButton = (row: FileRow) => canIndexFile(row) || canReindexFile(row)

/** 是否为重新入库操作（已完成入库后的二次入库） */
export const isReindexAction = (row: FileRow) => canReindexFile(row)

/** 单行入库按钮文案 */
export const getIndexActionLabel = (row: FileRow) => {
  if (canReindexFile(row)) return '重新入库'
  const status = normalizeStatus(row)
  return status === 'error_indexing' ? '重试入库' : '入库'
}

/** 入库按钮样式：未完成用主题色，已完成用与详情一致的灰蓝色 */
export const getIndexActionIconClass = (row: FileRow) =>
  isReindexAction(row) ? 'bg-info/12 text-info' : 'bg-theme/12 text-theme'

/** 入库参数弹窗标题 */
export const getIndexConfigTitle = (row: FileRow) => {
  if (canReindexFile(row)) return '重新入库参数配置'
  return `${getIndexActionLabel(row)}参数配置`
}
