import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'

/** Shown under 服务地址 and as the save error for a base URL already in use. */
export const DUPLICATE_SERVICE_URL_TEXT = '这个服务地址已经添加过了'
/** Host error code for the same case (model_service::DUPLICATE_BASE_URL_CODE). */
export const DUPLICATE_SERVICE_URL_CODE = 'model.duplicate_base_url'
const GENERIC_SAVE_ERROR_TEXT = '保存失败，请稍后重试'

/**
 * Comparison key for a service base URL, mirroring the Host normalization:
 * trimmed, lower-cased, trailing slashes removed.
 */
export function serviceUrlKey(url: string): string {
  return url.trim().replace(/\/+$/, '').toLowerCase()
}

/** The other service (not `ownId`) that already uses `baseUrl`, if any. */
export function findDuplicateServiceUrl<T extends { id: string; baseUrl: string }>(items: readonly T[], ownId: string | undefined, baseUrl: string): T | null {
  const key = serviceUrlKey(baseUrl)
  if (!key) return null
  return items.find((item) => item.id !== ownId && serviceUrlKey(item.baseUrl) === key) ?? null
}

const RAW_STORAGE_TEXT = /UNIQUE constraint failed|constraint failed|SQLITE_|sqlite|no such (table|column)|syntax error|database (is locked|disk image)|readonly database|FOREIGN KEY/i

/**
 * Text for a failed model-service save/delete. Never shows raw SQL: the
 * duplicate base URL becomes the duplicate hint, any other storage text a
 * generic retry hint.
 */
export function providerErrorText(cause: unknown): string {
  const { code, message } = desktopErrorDetails(cause)
  if (code === DUPLICATE_SERVICE_URL_CODE || /UNIQUE constraint failed:\s*model_providers\.base_url/i.test(message)) return DUPLICATE_SERVICE_URL_TEXT
  if (RAW_STORAGE_TEXT.test(message)) return GENERIC_SAVE_ERROR_TEXT
  return message
}
