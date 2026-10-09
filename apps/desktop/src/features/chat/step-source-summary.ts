/**
 * The extra source a process step can name — kept in its own tiny module so the
 * row can render it without pulling the lazily-loaded process helpers into the
 * entry bundle.
 *
 * Every value comes from the call's own fields or from its result. The point is to
 * answer "which source" without pasting arguments, code, internal ids or
 * diagnostics; when nothing reliable is present it returns '' and the row keeps its
 * action wording instead of inventing a source.
 */
export function stepExtraSource(tool: string, input: unknown, result: unknown): string {
  const call = input && typeof input === 'object' && !Array.isArray(input) ? input as Record<string, unknown> : {}
  const outcome = result && typeof result === 'object' && !Array.isArray(result) ? result as Record<string, unknown> : {}
  const text = (value: unknown) => typeof value === 'string' && value.trim() ? value.trim() : ''
  const namesFrom = (source: Record<string, unknown>, key: string, field: string) => {
    const list = source[key]
    if (!Array.isArray(list)) return ''
    for (const item of list) {
      const named = item && typeof item === 'object' && !Array.isArray(item)
        ? text((item as Record<string, unknown>)[field])
        : text(item)
      if (named) return named
    }
    return ''
  }
  const hostOf = (value: string) => {
    try { return value ? new URL(value).host : '' } catch { return '' }
  }
  if (tool === 'read') {
    // A worksheet the read actually reported, e.g. "Detailed Sales".
    return namesFrom(outcome, 'sheets', 'name')
  }
  if (tool === 'attachment_compute' || tool === 'compute_job_start' || tool === 'read_attachment') {
    // The attached workbook's own name, then a project path the caller named.
    // Raw attachment ids are resolved by `attachmentSourceNames` instead: an id this
    // layer cannot turn into a name is never shown as an internal identifier.
    return namesFrom(outcome, 'attachments', 'name')
      || namesFrom(call, 'projectPaths', 'name')
  }
  if (tool === 'web_search' || tool === 'web_read' || tool === 'http_request' || tool === 'browser_open') {
    const title = text(outcome.title) || text(call.title)
    if (title) return title
    const url = text(call.url) || text(call.uri)
    if (url) return hostOf(url)
    const first = Array.isArray(outcome.results) && outcome.results.length
      ? outcome.results[0] as Record<string, unknown> : null
    return first ? text(first.title) || hostOf(text(first.url)) : ''
  }
  if (tool === 'ls' || tool === 'find') {
    // The row already renders the path when the call carried one; only a call with
    // no path is known to be the project root.
    return text(call.path) ? '' : '项目根目录'
  }
  return ''
}

/**
 * The uploaded attachments an `attachmentIds` list names, resolved to the names the
 * conversation shows.
 *
 * Only the conversation's own attachment-name table may resolve an id: guessing from
 * the value's *appearance* (an extension, a slash) produced plausible-looking names
 * for things that were never attachments, so a value that is not in the table
 * contributes nothing. No internal id, handle or bare word ever reaches the row.
 */
export function attachmentSourceNames(input: unknown, names?: ReadonlyMap<string, string>): string {
  const call = input && typeof input === 'object' && !Array.isArray(input) ? input as Record<string, unknown> : {}
  const list = call.attachmentIds
  if (!Array.isArray(list) || !names) return ''
  for (const item of list) {
    if (typeof item !== 'string') continue
    const mapped = names.get(item)
    if (typeof mapped === 'string' && mapped.trim()) return mapped.trim()
  }
  return ''
}
