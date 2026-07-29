const ZIP_LOCAL_FILE_SIGNATURE = 0x04034b50
const ZIP_CENTRAL_DIRECTORY_SIGNATURE = 0x02014b50
const ZIP_END_OF_CENTRAL_DIRECTORY_SIGNATURE = 0x06054b50
const ZIP64_SENTINEL_16 = 0xffff
const ZIP64_SENTINEL_32 = 0xffffffff
const MAX_ZIP_COMMENT_BYTES = 0xffff

export interface ZipSafetyLimits {
  maxEntries: number
  maxUncompressedBytes: number
  maxEntryUncompressedBytes: number
  maxCompressionRatio: number
}

export interface ZipArchiveSummary {
  entries: number
  compressedBytes: number
  uncompressedBytes: number
}

export function hasZipSignature(bytes: ArrayBuffer | Uint8Array) {
  const value = asBytes(bytes)
  if (value.byteLength < 4) return false
  return new DataView(value.buffer, value.byteOffset, value.byteLength).getUint32(0, true) === ZIP_LOCAL_FILE_SIGNATURE
}

/**
 * Inspects the ZIP central directory before an Office parser expands the archive.
 * ZIP64 is deliberately rejected for the in-app fast preview; users can still
 * download and open those files with a desktop application.
 */
export function assertSafeZipArchive(
  input: ArrayBuffer | Uint8Array,
  limits: ZipSafetyLimits,
): ZipArchiveSummary {
  const bytes = asBytes(input)
  if (!hasZipSignature(bytes)) throw new Error('文件不是有效的 ZIP Office 文档。')
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const eocdOffset = findEndOfCentralDirectory(view)
  if (eocdOffset < 0) throw new Error('Office 文档的 ZIP 目录不完整或已损坏。')

  const diskNumber = view.getUint16(eocdOffset + 4, true)
  const centralDisk = view.getUint16(eocdOffset + 6, true)
  const entriesOnDisk = view.getUint16(eocdOffset + 8, true)
  const entries = view.getUint16(eocdOffset + 10, true)
  const centralSize = view.getUint32(eocdOffset + 12, true)
  const centralOffset = view.getUint32(eocdOffset + 16, true)
  if (diskNumber !== 0 || centralDisk !== 0 || entriesOnDisk !== entries) {
    throw new Error('快速预览不支持分卷 ZIP Office 文档。')
  }
  if (entries === ZIP64_SENTINEL_16 || centralSize === ZIP64_SENTINEL_32 || centralOffset === ZIP64_SENTINEL_32) {
    throw new Error('快速预览不支持 ZIP64 Office 文档，请下载后使用桌面应用打开。')
  }
  if (entries > limits.maxEntries) throw new Error(`Office 文档包含过多内部文件（${entries} 个）。`)
  if (centralOffset + centralSize > eocdOffset || centralOffset > bytes.byteLength) {
    throw new Error('Office 文档的 ZIP 目录范围无效。')
  }

  let cursor = centralOffset
  let compressedBytes = 0
  let uncompressedBytes = 0
  for (let index = 0; index < entries; index += 1) {
    if (cursor + 46 > eocdOffset || view.getUint32(cursor, true) !== ZIP_CENTRAL_DIRECTORY_SIGNATURE) {
      throw new Error('Office 文档的 ZIP 目录条目无效。')
    }
    const flags = view.getUint16(cursor + 8, true)
    const compressed = view.getUint32(cursor + 20, true)
    const uncompressed = view.getUint32(cursor + 24, true)
    const nameLength = view.getUint16(cursor + 28, true)
    const extraLength = view.getUint16(cursor + 30, true)
    const commentLength = view.getUint16(cursor + 32, true)
    if ((flags & 0x1) !== 0) throw new Error('快速预览不支持加密的 Office 文档。')
    if (compressed === ZIP64_SENTINEL_32 || uncompressed === ZIP64_SENTINEL_32) {
      throw new Error('快速预览不支持 ZIP64 Office 文档，请下载后使用桌面应用打开。')
    }
    if (uncompressed > limits.maxEntryUncompressedBytes) {
      throw new Error('Office 文档中的单个内部文件展开后过大。')
    }
    if (uncompressed > 0 && (compressed === 0 || uncompressed / compressed > limits.maxCompressionRatio)) {
      throw new Error('Office 文档的压缩比异常，已停止快速预览。')
    }
    compressedBytes += compressed
    uncompressedBytes += uncompressed
    if (!Number.isSafeInteger(uncompressedBytes) || uncompressedBytes > limits.maxUncompressedBytes) {
      throw new Error('Office 文档展开后的总大小超过快速预览限制。')
    }
    cursor += 46 + nameLength + extraLength + commentLength
    if (cursor > eocdOffset) throw new Error('Office 文档的 ZIP 目录条目越界。')
  }
  if (cursor > centralOffset + centralSize) throw new Error('Office 文档的 ZIP 目录大小不一致。')
  return { entries, compressedBytes, uncompressedBytes }
}

function findEndOfCentralDirectory(view: DataView) {
  const minimumOffset = Math.max(0, view.byteLength - MAX_ZIP_COMMENT_BYTES - 22)
  for (let offset = view.byteLength - 22; offset >= minimumOffset; offset -= 1) {
    if (view.getUint32(offset, true) !== ZIP_END_OF_CENTRAL_DIRECTORY_SIGNATURE) continue
    const commentLength = view.getUint16(offset + 20, true)
    if (offset + 22 + commentLength === view.byteLength) return offset
  }
  return -1
}

function asBytes(value: ArrayBuffer | Uint8Array) {
  return value instanceof Uint8Array ? value : new Uint8Array(value)
}
