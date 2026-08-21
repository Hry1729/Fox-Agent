export function buildQuery(params?: Record<string, unknown>): string
export function normalizeWorkspacePath(path?: string): string
export function buildBreadcrumbs(
  path: string,
  rootLabel?: string
): Array<{ name: string; path: string }>
export function formatFileSize(bytes?: number | string | null): string
export function getPreviewType(
  path?: string,
  contentType?: string
): 'markdown' | 'text' | 'image' | 'pdf' | 'html' | 'spreadsheet' | 'office' | 'unsupported'
export function parseSpreadsheetBlob(blob: Blob): Promise<{
  sheets: Array<{
    name: string
    rows: string[][]
    totalRows: number
    totalColumns: number
    truncated: boolean
  }>
  truncated: boolean
  limits: { rows: number; columns: number }
}>
export function parseDownloadFilename(contentDisposition?: string): string
export function normalizePreviewResponse(
  response: Response,
  baseFile?: Record<string, unknown> | object
): Promise<{
  name?: string
  path?: string
  content?: string | null
  previewType:
    | 'markdown'
    | 'text'
    | 'image'
    | 'pdf'
    | 'html'
    | 'spreadsheet'
    | 'office'
    | 'unsupported'
    | string
  previewUrl?: string
  supported?: boolean
  message?: string
  spreadsheet?: {
    sheets: Array<{
      name: string
      rows: string[][]
      totalRows: number
      totalColumns: number
      truncated: boolean
    }>
    truncated: boolean
    limits: { rows: number; columns: number }
  }
  [key: string]: unknown
}>
