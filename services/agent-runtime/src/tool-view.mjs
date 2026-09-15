// Model-view bounding for large re-readable reference tool results.
//
// The durable result is never truncated: Host persistence keeps the full
// content, pairing, success/failure state and execution receipt. This only
// shapes the content projected into the model context so a long help index,
// document read or data sample cannot flood the input window.
//
// Two rules keep the durable result reachable after bounding:
//   1. Structured (JSON) payloads are projected *structurally*. Keys, item
//      order, item count and any `next` navigation entry survive, so a paged
//      catalog stays parseable and every page remains reachable through the
//      same controlled tool. A character-level cut would produce invalid JSON
//      and silently drop the middle of the catalog.
//   2. Unstructured text is bounded on UTF-8 code-point boundaries, never
//      inside a multi-byte character, so no U+FFFD replacement character is
//      ever introduced.
//   3. When neither is possible — `next` alone exceeds the budget, or records
//      would have to be dropped from a collection this module cannot point back
//      into — the payload is published **unchanged**. A bounded view that
//      omitted records it cannot address would claim reachability it does not
//      have, and a head/tail cut of JSON would publish invalid JSON.

// Tools whose output is re-readable reference material rather than a one-shot
// fact. Execution/write/fact tools are never bounded.
//
// `call_mcp_tool` is deliberately absent: a generic MCP call has unknown
// side-effect and re-read semantics ("call it again" is not guaranteed to
// return the same value, and may repeat a write), so its result must pass
// through verbatim. `list_mcp_tools` only enumerates the catalog and is safe.
//
// Every name here must be a tool Fox actually registers; an entry that can never
// match would advertise a capability nobody has. (`attachment_pages`,
// `graph_read` and `read_only_graph_read` were removed for exactly that reason.)
const BOUNDABLE_TOOLS = new Set([
  'office_help',
  'office_read',
  'office_validate',
  'read',
  'read_attachment',
  'query_knowledge_graph',
  'list_mcp_tools',
])

// Reference text kept verbatim per tool result (UTF-8 bytes).
const HEAD_BYTES = 6_000
const TAIL_BYTES = 2_000
const MAX_BYTES = 9_000
const RECEIPT_MARKER = 'FOX_EXECUTION_RECEIPT_V1'
// The tool the model calls to recover bytes omitted from a bounded view. Its
// schema, Run policy entry and Host dispatch live in `host-tools.mjs`,
// `TOOL_CONTRACTS` and `runtime_host/tool_result_read.rs`.
export const RESULT_REF_TOOL = 'read_tool_result'
// Host caps `tool_calls.result_json` at this size (mirrors the Rust constant);
// above it only a bounded preview survives.
const MAX_STORED_TOOL_RESULT_BYTES = 128 * 1024
// Slack for the result envelope (`content`, `details`, timestamps) around the
// view payload: a result sitting near the cap is treated as NOT retrievable
// rather than promising bytes that were replaced by a preview.
const STORED_ENVELOPE_MARGIN_BYTES = 4_096
// Longest single string leaf kept verbatim in a structural projection. The
// projection lowers this progressively until the JSON fits the budget.
const STRUCT_FIELD_BYTES = 160
const STRUCT_FIELD_BYTES_FLOOR = 16
const STRUCT_VIEW_KEY = 'foxModelView'
// Smallest projection that can still name the payload and carry `next`; used to
// decide whether navigation can be carried at all.
const MIN_STRUCTURED_ENVELOPE_BYTES = 384

function utf8Bytes(value) {
  return Buffer.byteLength(value ?? '', 'utf8')
}

/**
 * Stable reference to the durable *record* of one settled tool call. Host keys
 * that record by the pair (run id, tool call id) — `tool_calls.run_id` with
 * `tool_calls.runtime_tool_call_id`, declared UNIQUE — and the Host resolves it
 * back to stored bytes under the owning conversation's authorization
 * (`Database::tool_result_range`, `parse_tool_result_ref` in Rust).
 *
 * The stored copy is bounded at 128 KiB (`MAX_STORED_TOOL_RESULT_BYTES`): above
 * that the row holds a Fox preview with `truncated: true`, and the reader says
 * so instead of pretending the full text is available. Never present this
 * reference as a guarantee of complete historical content.
 */
export function toolResultRef(runId, toolCallId) {
  if (typeof runId !== 'string' || typeof toolCallId !== 'string') return null
  if (runId.length === 0 || toolCallId.length === 0) return null
  return `fox-result://${runId}/${toolCallId}`
}

/**
 * Parse `fox-result://<runId>/<toolCallId>` back into its identity pair, the
 * mirror of the Rust `parse_tool_result_ref`. Returns null for anything that is
 * not exactly one run id and one call id.
 */
export function parseToolResultRef(reference) {
  if (typeof reference !== 'string' || !reference.startsWith('fox-result://')) return null
  const rest = reference.slice('fox-result://'.length)
  const split = rest.indexOf('/')
  if (split < 0) return null
  const runId = rest.slice(0, split)
  const toolCallId = rest.slice(split + 1)
  if (!runId || !toolCallId || toolCallId.includes('/')) return null
  if (runId.length > 512 || toolCallId.length > 512) return null
  return { runId, toolCallId }
}

/** Largest prefix of `text` no longer than `maxBytes`, never splitting a code point. */
export function utf8Prefix(text, maxBytes) {
  let used = 0
  let end = 0
  while (end < text.length) {
    const code = text.charCodeAt(end)
    let size = 1
    let width = 1
    if (code < 0x80) {
      size = 1
    } else if (code < 0x800) {
      size = 2
    } else if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(end + 1)
      if (next >= 0xdc00 && next <= 0xdfff) {
        size = 4
        width = 2
      } else {
        size = 3 // lone high surrogate: encoded as U+FFFD by Buffer, as by any encoder
      }
    } else {
      size = 3
    }
    if (used + size > maxBytes) break
    used += size
    end += width
  }
  return text.slice(0, end)
}

/** Largest suffix of `text` no longer than `maxBytes`, never splitting a code point. */
export function utf8Suffix(text, maxBytes) {
  let used = 0
  const kept = []
  let index = text.length
  while (index > 0) {
    let width = 1
    const code = text.charCodeAt(index - 1)
    let size
    if (code < 0x80) {
      size = 1
    } else if (code < 0x800) {
      size = 2
    } else if (code >= 0xdc00 && code <= 0xdfff && index >= 2) {
      const high = text.charCodeAt(index - 2)
      if (high >= 0xd800 && high <= 0xdbff) {
        size = 4
        width = 2
      } else {
        size = 3
      }
    } else {
      size = 3
    }
    if (used + size > maxBytes) break
    used += size
    index -= width
    kept.push(text.slice(index, index + width))
  }
  kept.reverse()
  return kept.join('')
}

function replaceLongStrings(value, fieldBytes, preserve) {
  if (typeof value === 'string') {
    if (preserve || utf8Bytes(value) <= fieldBytes) return value
    return `${utf8Prefix(value, Math.max(0, fieldBytes - 3))}…`
  }
  if (Array.isArray(value)) {
    return value.map((item) => replaceLongStrings(item, fieldBytes, preserve))
  }
  if (value && typeof value === 'object') {
    const out = {}
    for (const [key, child] of Object.entries(value)) {
      out[key] = replaceLongStrings(child, fieldBytes, preserve)
    }
    return out
  }
  return value
}

/**
 * Whether bytes omitted from the model view can still be recovered.
 *
 * True only while the result is small enough that Host stores it whole, so
 * `read_tool_result` can walk the real thing. Above the cap Host keeps a bounded
 * preview and no range read can reach beyond it — the projection must then keep
 * every record instead of omitting content it has no way to hand back.
 */
function resultIsRetrievable(totalBytes) {
  return totalBytes + STORED_ENVELOPE_MARGIN_BYTES <= MAX_STORED_TOOL_RESULT_BYTES
}

/** How the model reaches bytes this projection omitted. */
function retrievalInstruction(reference) {
  if (!reference) {
    return '本次未附带结果引用，请改用同一只读工具以更精确的 selector/页码重新查询。'
  }
  return (
    `调用 ${RESULT_REF_TOOL} {"reference":"${reference}"} 可只读取回 Host 已保存的本次结果` +
    '（不重新执行任何工具，也不会重放写操作）；返回 details.complete=true 前按 details.nextOffset 继续接力即可取得全部原文。'
  )
}

function viewNote(totalBytes, retrievable, extra) {
  return {
    bounded: true,
    originalBytes: totalBytes,
    budgetBytes: MAX_BYTES,
    // Whether every omitted byte can still be reached. False means Host did not
    // keep the whole result, so the view must not omit anything.
    retrievable,
    ...extra,
  }
}

/** Attach the notice without pushing the payload over the budget. */
function withNote(payload, note, isArray) {
  if (isArray) {
    return { [STRUCT_VIEW_KEY]: note, foxBoundedList: payload }
  }
  if (payload && typeof payload === 'object') {
    return { ...payload, [STRUCT_VIEW_KEY]: note }
  }
  return { [STRUCT_VIEW_KEY]: note, foxValue: payload }
}

/**
 * Split a top-level `next` navigation entry off the payload so it can be
 * re-attached verbatim after projection. Navigation is what makes omitted
 * records reachable, so it is never shortened and never dropped on its own.
 */
function splitNext(payload) {
  if (payload && typeof payload === 'object' && !Array.isArray(payload)) {
    const { next, ...body } = payload
    return [body, next === undefined || next === null ? null : next]
  }
  return [payload, null]
}

/** Compose the bounded view: projected body + note + `next` verbatim. */
function composeView(body, note, isArray, next) {
  const out = withNote(body, note, isArray)
  if (next !== null && next !== undefined && out && typeof out === 'object') {
    return { ...out, next }
  }
  return out
}

function largestArrayField(work) {
  if (!work || typeof work !== 'object' || Array.isArray(work)) return null
  let best = null
  for (const [key, child] of Object.entries(work)) {
    if (!Array.isArray(child)) continue
    if (!best || child.length > work[best].length) best = key
  }
  return best
}

/**
 * Continuation that resumes exactly at the first record the projection omitted,
 * for the one paged contract this module can prove: `office_help`, whose
 * `apps/desktop/src-tauri/src/office.rs::shape_help` slices a sorted catalog as
 * `start = (page - 1) * pageSize` with `1 <= pageSize <= 60` and whose input
 * schema declares exactly `format` (required), `element`, `page`, `pageSize`.
 *
 * Absolute index `resumeIndex` is reachable by any `pageSize` dividing it (`1`
 * always divides), with `page = resumeIndex / pageSize + 1`. Only fields the
 * tool's real input schema declares are emitted. Returns null for anything
 * else, which makes the caller keep every record instead of dropping records it
 * cannot point back to. Mirrors `paged_resume` / `paged_contract` in
 * kernel_compaction.rs.
 */
export function pagedResume(payload, collectionKey, kept) {
  if (collectionKey !== 'properties') return null
  if (!payload || typeof payload !== 'object') return null
  const { format, element, page, pageSize } = payload
  if (typeof format !== 'string' || format.length === 0) return null
  if (typeof element !== 'string' || element.length === 0) return null
  if (!Number.isInteger(page) || page < 1) return null
  if (!Number.isInteger(pageSize) || pageSize < 1 || pageSize > 60) return null
  const resumeIndex = (page - 1) * pageSize + kept
  // `resumeIndex === 0` is legal: it re-reads from the first record, and paging
  // forward from page 1 still reaches every later record.
  if (!Number.isSafeInteger(resumeIndex) || resumeIndex < 0) return null
  let pageSizeOut = 1
  for (let divisor = 1; divisor <= 60; divisor += 1) {
    if (resumeIndex % divisor === 0) pageSizeOut = divisor
  }
  return {
    tool: 'office_help',
    arguments: {
      format,
      element,
      page: resumeIndex / pageSizeOut + 1,
      pageSize: pageSizeOut,
    },
    instruction:
      `This page was shortened for the model view. Records from index ${resumeIndex} on are not in this response; ` +
      `echoing this continuation resumes exactly at record ${resumeIndex + 1} (pageSize=${pageSizeOut}) and paging forward reaches every remaining record.`,
  }
}

/**
 * Project a parsed JSON payload so it stays valid JSON within `maxBytes`.
 * Keys, item order and `next` navigation survive; records are dropped only
 * together with a continuation that provably reaches them under the tool's real
 * schema. Returns null when no honest projection fits, so the caller keeps the
 * original content rather than publishing a view whose navigation lies.
 */
export function projectStructuredText(tool, parsed, maxBytes, reference) {
  if (parsed === null || typeof parsed !== 'object') return null
  const isArray = Array.isArray(parsed)
  const totalBytes = utf8Bytes(JSON.stringify(parsed))
  // Decided once for the whole payload: omitting anything is only honest while
  // the omitted bytes stay reachable.
  const retrievable = resultIsRetrievable(totalBytes)
  const note = viewNote(
    totalBytes,
    retrievable,
    reference
      ? {
          resultRef: reference,
          resultRefNote: retrievalInstruction(reference),
          reason: retrievable
            ? '模型视图有界化：原始结果对象未被修改。被省略的内容可按 resultRef 取回，不要凭省略内容编造字段。'
            : '模型视图有界化：原始结果对象未被修改。此结果体量接近或超过 Host 存储上限，超出的部分只会存为摘要预览，因此本视图不省略任何记录。',
        }
      : {
          reason: retrievable
            ? '模型视图有界化：原始结果对象未被修改。被省略的记录只能按下述续读方式取回，不要凭省略内容编造字段。'
            : '模型视图有界化：原始结果对象未被修改。此结果体量接近或超过 Host 存储上限，因此本视图不省略任何记录。',
        },
  )

  const [body, next] = splitNext(parsed)
  const nextBytes = next === null ? 0 : utf8Bytes(JSON.stringify(next))
  // Navigation is reserved inside the budget. If it cannot be carried, do not
  // bound this result at all rather than silently hide where it points.
  if (nextBytes + MIN_STRUCTURED_ENVELOPE_BYTES > maxBytes) return null

  for (
    let fieldBytes = STRUCT_FIELD_BYTES;
    fieldBytes >= STRUCT_FIELD_BYTES_FLOOR;
    fieldBytes = Math.floor(fieldBytes / 2)
  ) {
    const text = JSON.stringify(
      composeView(replaceLongStrings(body, fieldBytes, false), note, isArray, next),
    )
    if (utf8Bytes(text) <= maxBytes) return text
  }

  // The body still does not fit: drop trailing records from the largest
  // collection, but only while the omitted records stay reachable.
  if (!isArray) {
    const shrunk = replaceLongStrings(body, STRUCT_FIELD_BYTES_FLOOR, false)
    const key = largestArrayField(shrunk)
    if (key) {
      const full = shrunk[key]
      const resumable = pagedResume(parsed, key, 0) !== null
      // Reachability does not depend on how many records are kept, so decide it
      // once: the paged contract can be resumed for any keep count; anything
      // else must keep every record it cannot point back to.
      if (next !== null && !resumable) return null
      // Records may leave the view only while they stay reachable: either the
      // paged contract can be replayed, or Host kept the whole result *and* the
      // view carries the reference `read_tool_result` needs to walk it. With
      // neither, the result is left unbounded rather than losing records
      // everywhere.
      if (!resumable && !(retrievable && reference)) return null
      let keep = full.length
      while (keep > 0) {
        keep = Math.floor(keep / 2)
        const dropped = {
          ...note,
          omittedItems: full.length - keep,
          collectionKey: key,
        }
        if (dropped.omittedItems > 0) {
          let advice = resumable
            ? '用同一只读工具按 next 的页码继续查询即可取回'
            : `${retrievalInstruction(reference)} `
          if (resumable && reference) {
            advice += `；也可调用 ${RESULT_REF_TOOL} {"reference":"${reference}"} 按范围读回本次保存的原文`
          }
          advice += '；不要凭被省略的内容编造字段，也不要为读取历史结果而重放任何写操作。'
          dropped.reRead = `本响应未包含 collectionKey=${key} 的前 ${keep} 项之后的记录；${advice}`
        }
        const composedNext =
          next === null ? null : (pagedResume(parsed, key, keep) ?? next)
        const text = JSON.stringify(
          composeView({ ...shrunk, [key]: full.slice(0, keep) }, dropped, false, composedNext),
        )
        if (utf8Bytes(text) <= maxBytes) return text
      }
    }
  }

  // Last resort: a minimal valid envelope that still carries `next` verbatim.
  // Reached only when records were allowed to leave the view above — i.e. when
  // they are reachable.
  const fallback = JSON.stringify(
    composeView(
      null,
      viewNote(totalBytes, retrievable, {
        ...(reference ? { resultRef: reference } : {}),
        omitted: true,
        note: retrievalInstruction(reference),
      }),
      false,
      next,
    ),
  )
  return utf8Bytes(fallback) <= maxBytes ? fallback : null
}

/** Serialized `next` of a JSON object, kept verbatim by the text fallback. */
function navigationOf(text) {
  const trimmed = text.trim()
  if (!trimmed.startsWith('{')) return null
  let parsed
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== 'object') return null
  if (parsed.next === undefined || parsed.next === null) return null
  return JSON.stringify(parsed.next)
}

function textBoundNotice(total, headBytes, tailBytes, omitted, reference) {
  return [
    `[Fox 已为本条工具结果建立有界视图：原文约 ${total} 字节，模型上下文中保留开头 ${headBytes} 与结尾 ${tailBytes} 字节（原始结果对象未被修改）。`,
    retrievalInstruction(reference),
    '也可以按需用同一只读工具以更精确的 selector/页码重新查询——office_help 用 property=<属性名> 或 page=<页码>，office_read/read 用更精确的 selector 或更小范围；不要凭省略内容编造字段，也不要为读取历史结果重复任何写操作。]',
  ]
    .filter(Boolean)
    .join('')
}

/**
 * Bound one text block. Returns the text the model view must publish, or null
 * when the caller keeps the original (small, receipt-bearing, or non-boundable
 * tool).
 *
 * A structured (JSON) payload that no honest projection can carry is returned
 * **unchanged** rather than cut: a head/tail cut would produce invalid JSON and
 * hide records whose continuation was dropped with it. Returning the original
 * is the truthful model view — nothing omitted, nothing to reach for.
 */
export function boundToolText(tool, { isError = false, text = '', resultRef = null } = {}) {
  if (isError || !BOUNDABLE_TOOLS.has(tool)) return null
  if (typeof text !== 'string' || utf8Bytes(text) <= MAX_BYTES) return null
  // A receipt block is execution evidence and is never reshaped.
  if (text.includes(RECEIPT_MARKER)) return null

  const trimmed = text.trim()
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    let parsed
    try {
      parsed = JSON.parse(trimmed)
    } catch {
      parsed = undefined
    }
    if (parsed !== undefined) {
      const projected = projectStructuredText(tool, parsed, MAX_BYTES, resultRef)
      // No honest structured view: keep the payload intact. `next` alone may
      // exceed the whole budget, and omitting records it cannot point back to
      // would make the view lie about navigation.
      return projected !== null ? projected : text
    }
  }

  // A head/tail cut keeps the beginning and the end but loses the middle, so it
  // is only honest while the omitted bytes can be walked back: Host must keep
  // the whole result and the view must carry the reference. Without both there
  // is no path to the middle, and the text is published unchanged rather than
  // summarized into something unreachable.
  if (!(resultIsRetrievable(utf8Bytes(text)) && resultRef)) return text
  const total = utf8Bytes(text)
  const head = utf8Prefix(text, HEAD_BYTES)
  const tail = utf8Suffix(text, TAIL_BYTES)
  const omitted = Math.max(0, total - utf8Bytes(head) - utf8Bytes(tail))
  const parts = [
    head,
    '',
    textBoundNotice(total, utf8Bytes(head), utf8Bytes(tail), omitted, resultRef),
    `…[省略约 ${omitted} 字节，可用同一只读工具按需取回]…`,
    '',
    tail,
  ]
  // Navigation is kept outside the head/tail budget: a text-bound view that
  // dropped `next` would silently hide the records it claims are reachable.
  const navigation = navigationOf(text)
  if (navigation !== null) parts.push('', `["next" 导航原样保留] ${navigation}`)
  return parts.join('\n')
}

/**
 * Bound a settled result's content array for the model view. Returns the
 * possibly-replaced content; durable callers must keep the original result.
 */
export function boundToolResultContent(tool, { isError = false, content = [], resultRef = null } = {}) {
  if (isError || !BOUNDABLE_TOOLS.has(tool) || !Array.isArray(content)) return content
  let changed = false
  const next = content.map(block => {
    if (!block || block.type !== 'text') return block
    const bounded = boundToolText(tool, { isError, text: block.text, resultRef })
    // A structured payload returned unchanged is not a change.
    if (bounded === null || bounded === block.text) return block
    changed = true
    return { type: 'text', text: bounded }
  })
  return changed ? next : content
}

// ---------------------------------------------------------------------------
// `read_tool_result` model-visible cursor projection.
//
// `read_tool_result` returns each range's text in `content[0]` and its
// navigation facts (`nextOffset`, `complete`, …) in `details`. The provider
// projection serializes only `content` text blocks and drops `details`, so a
// multi-page read would lose the cursor and the completion/retrievability flags.
// These helpers re-print the whitelisted navigation facts as a trailing text
// block in the *model view* only: the durable result, the raw fragment, and the
// rest of `details` stay untouched. The fragment remains the first block so a
// reconstruction hash of the ranges never needs to read the metadata.
// ---------------------------------------------------------------------------

// The final text block carries Host navigation; the raw fragment can itself
// contain this text and must never be treated as metadata or rewritten.
// Mirrors `kernel_compaction::READ_RESULT_CURSOR_MARKER` on the Rust side.
export const READ_RESULT_CURSOR_MARKER = 'FOX_RESULT_CURSOR_V1'

// Whitelist of `read_tool_result` details that are model-relevant. Everything
// else (`runId`, `toolCallId`, `toolName`, `status`, `source`, `reread`) is
// either redundant with `reference` or Host-private, and stays out of the view.
const READ_RESULT_PUBLIC_FIELDS = [
  'reference', 'offset', 'returnedBytes', 'nextOffset',
  'complete', 'originalBytes', 'retrievable', 'truncated',
]

/** Extract the whitelisted navigation facts from `read_tool_result` details. */
export function readToolResultNavigationView(details) {
  if (!details || typeof details !== 'object' || Array.isArray(details)) return null
  const view = {}
  for (const field of READ_RESULT_PUBLIC_FIELDS) {
    if (Object.hasOwn(details, field)) view[field] = details[field]
  }
  return Object.keys(view).length > 0 ? view : null
}

/** Render the cursor as a single-line, JSON-parseable trailing text block. */
export function renderReadResultNavigation(nav) {
  return `${READ_RESULT_CURSOR_MARKER} ${JSON.stringify(nav)}`
}

/** The trailing text block for one range, or null when there is no navigation. */
export function readResultNavigationBlock(details) {
  const nav = readToolResultNavigationView(details)
  return nav ? { type: 'text', text: renderReadResultNavigation(nav) } : null
}

/**
 * Project a settled tool result's content into the model view. For
 * `read_tool_result` this bounds nothing (the tool is not re-readable reference
 * material) and appends the cursor metadata block so a continuation stays
 * reachable; for every other tool it is the existing bounded view. `details`
 * may be null or absent.
 */
export function modelToolResultContent(tool, { isError = false, content = [], details = null, resultRef = null } = {}) {
  const bounded = boundToolResultContent(tool, { isError, content, resultRef })
  const view = Array.isArray(bounded) ? bounded : content
  if (isError || tool !== RESULT_REF_TOOL) return view
  const block = readResultNavigationBlock(details)
  if (!block) return view
  // A replay may already contain a projected cursor. Compare the final block
  // semantically (Rust and Node serialize keys in different orders), keeping
  // the first block's stored bytes intact even if they look like a cursor.
  const last = view.at(-1)
  const prefix = `${READ_RESULT_CURSOR_MARKER} `
  if (view.length > 1 && last?.type === 'text' && last.text?.startsWith(prefix)) {
    try {
      const prior = JSON.parse(last.text.slice(prefix.length))
      const nav = readToolResultNavigationView(details)
      if (prior && Object.keys(prior).length === Object.keys(nav).length
          && Object.entries(nav).every(([key, value]) => prior[key] === value)) return view
    } catch { /* A source text block is not navigation merely because it has a marker. */ }
  }
  return [...view, block]
}
