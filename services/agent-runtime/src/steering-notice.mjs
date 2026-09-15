// Canonical wrapper for mid-run user requests ("steering"). The Rust Host
// builds the identical text when it records these messages in durable history
// (steering_notice_text in kernel_coordinator/steering.rs); the live loop and
// the replacement batch-resume runner both splice the same wording into the
// provider transcript. Steering is plain user text and never grants
// authority, tools or replayable writes.
export function steeringNoticeText(content) {
  return `用户在运行过程中补充要求（不改变已有授权，按既有工具策略执行）：\n${content}`
}

// Validate a directive/frame `steering` array the same bounds the Host and
// protocol enforce: unique non-empty ids, bounded non-empty contents and a
// 64 KiB outstanding total. Returns normalized notices or throws.
export function validateSteeringNotices(steering, fail) {
  if (steering === undefined) return []
  if (!Array.isArray(steering)) fail('steering must be an array')
  const seen = new Set()
  let totalBytes = 0
  for (const notice of steering) {
    if (!notice || typeof notice !== 'object'
        || typeof notice.messageId !== 'string' || !notice.messageId.trim()
        || notice.messageId.length > 128
        || typeof notice.content !== 'string' || !notice.content.trim()
        || [...notice.content].length > 8000) {
      fail('invalid steering notice')
    }
    if (seen.has(notice.messageId)) fail('duplicate steering notice')
    seen.add(notice.messageId)
    totalBytes += Buffer.byteLength(notice.content, 'utf8')
    if (totalBytes > 64 * 1024) fail('steering exceeds the bounded queue size')
  }
  return steering
}
