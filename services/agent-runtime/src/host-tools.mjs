import { Type } from 'typebox'
import { KNOWLEDGE_TOOL_NAMES } from './runtime-contract.mjs'
import { defineFoxTools } from './tool-adapter.mjs'
import { VALIDATION_CHECK_TYPES } from './validation-policy.mjs'

const VALIDATION_CHECK_TYPE_SCHEMA = Type.Union(
  VALIDATION_CHECK_TYPES.map((checkType) => Type.Literal(checkType)),
)

const TASK_ATTEMPT_STATUS_SCHEMA = Type.Union([
  Type.Literal('succeeded'),
  Type.Literal('failed'),
  Type.Literal('blocked'),
  Type.Literal('cancelled'),
])

const TRIMMED_REQUIRED_STRING_PATTERN = '^(?![\\s\\u0085])[\\s\\S]*[^\\s\\u0085]$'

function trimmedRequiredString(maxLength) {
  return Type.String({
    minLength: 1,
    maxLength,
    pattern: TRIMMED_REQUIRED_STRING_PATTERN,
  })
}

const PLAN_NODE_KEY_SCHEMA = Type.String({
  minLength: 1,
  maxLength: 100,
  pattern: '^[A-Za-z0-9._-]+$',
})

const SERIAL_PLAN_TASK_SCHEMA = Type.Object({
  title: trimmedRequiredString(500),
  detail: Type.Optional(Type.Union([trimmedRequiredString(4000), Type.Null()])),
  ordinal: Type.Integer({ minimum: 0 }),
}, { additionalProperties: false })

const GRAPH_PLAN_TASK_SCHEMA = Type.Object({
  nodeKey: PLAN_NODE_KEY_SCHEMA,
  title: trimmedRequiredString(300),
  detail: Type.Optional(Type.Union([trimmedRequiredString(4000), Type.Null()])),
  ordinal: Type.Integer({ minimum: 0 }),
  dependsOn: Type.Optional(Type.Array(PLAN_NODE_KEY_SCHEMA, {
    maxItems: 3,
    uniqueItems: true,
  })),
  acceptanceCriteria: Type.Array(trimmedRequiredString(1000), {
    minItems: 1,
    maxItems: 8,
    uniqueItems: true,
  }),
}, { additionalProperties: false })

const PLAN_TASKS_SCHEMA = Type.Union([
  Type.Array(SERIAL_PLAN_TASK_SCHEMA, { minItems: 1, maxItems: 64 }),
  Type.Array(GRAPH_PLAN_TASK_SCHEMA, { minItems: 1, maxItems: 3 }),
])

function preparePlanRevisionArguments(args) {
  if (!args || typeof args !== 'object' || Array.isArray(args) || typeof args.tasks !== 'string') {
    return args
  }
  const serializedTasks = args.tasks.trim()
  if (!serializedTasks.startsWith('[') || serializedTasks.length > 400_000) return args
  try {
    const tasks = JSON.parse(serializedTasks)
    return Array.isArray(tasks) ? { ...args, tasks } : args
  } catch {
    return args
  }
}

const hostStorage = new WeakMap()
export const hostToolResultStorage = (_id, result) => result && typeof result === "object" ? hostStorage.get(result) : null

async function executeHostTool(toolCallId, tool, input, requestHost, signal) {
  const response = await requestHost('tool.execute', { toolCallId, tool, input }, signal)
  const payload = response?.payload ?? {}
  if (payload.isError || !Object.prototype.hasOwnProperty.call(payload, 'result')) {
    const baseMessage = payload.error || `Fox host tool ${tool} failed.`
    // A completed attempt that did not succeed keeps its diagnostics in the
    // persisted result. Surface them here as well, or the model is told only
    // that something failed and repeats the identical call. The Host text is
    // already bounded and credential-redacted; the full output stays behind
    // the result reference and is reachable through read_tool_result.
    const resultText = Array.isArray(payload.result?.content)
      ? payload.result.content
        .filter((block) => typeof block?.text === 'string' && block.text.trim())
        .map((block) => block.text.trim())
        .join('\n\n')
      : ''
    // A result that exceeded the inline cap comes back as the Host's bounded
    // preview, so its body is deliberately not on the wire. The Host states the
    // reference (its own run/call identity, never tool input) together with
    // whether that reference really resolves, which is how the model reaches the
    // retained diagnostics without repeating the failing call.
    const reference = typeof payload.resultRef === 'string' && payload.resultRef
      ? payload.resultRef
      : ''
    const storageNote = reference
      ? payload.resultRefNote || `完整输出可按 ${reference} 用 read_tool_result 分页读回`
      : ''
    const parts = [baseMessage]
    if (resultText && !baseMessage.includes(resultText)) parts.push(resultText)
    if (reference && !parts.some((part) => part.includes(reference))) parts.push(storageNote)
    const message = parts.join('\n')
    const serializedDetails = payload.errorDetails && typeof payload.errorDetails === 'object'
      ? JSON.stringify(payload.errorDetails)
      : ''
    const error = new Error(serializedDetails
      ? `${message}\nFox error details: ${serializedDetails}`
      : message)
    if (payload.errorDetails && typeof payload.errorDetails === 'object') {
      error.details = payload.errorDetails
      if (typeof payload.errorDetails.code === 'string' && payload.errorDetails.code) {
        error.code = payload.errorDetails.code
      }
      if (typeof payload.errorDetails.retryable === 'boolean') {
        error.retryable = payload.errorDetails.retryable
      }
    }
    if (typeof payload.errorCode === 'string' && payload.errorCode) error.code = payload.errorCode
    // The classification belongs to the execution, so a payload that carries
    // it only inside the persisted result is still unambiguous.
    const detailCode = payload.result?.details?.errorCode
    if (!error.code && typeof detailCode === 'string' && detailCode) error.code = detailCode
    throw error
  }
  if (payload.result && typeof payload.result === "object" && payload.toolResultStorage) hostStorage.set(payload.result, payload.toolResultStorage)
  return payload.result
}

const COMPUTE_PARAMETERS = Type.Object({
  attachmentIds: Type.Optional(Type.Array(Type.String({ minLength: 1 }), { maxItems: 8, uniqueItems: true })),
  artifactIds: Type.Optional(Type.Array(Type.String({ minLength: 1 }), { maxItems: 8, uniqueItems: true })),
  code: Type.String({ minLength: 1, maxLength: 131072 }),
  processing: Type.Optional(Type.Literal('chunked')),
  profile: Type.Optional(Type.Union([Type.Literal('standard'), Type.Literal('large')])),
}, { additionalProperties: false })

export function createHostTools(requestHost) {
  return defineFoxTools([
    {
      name: 'read_attachment',
      label: 'Read attachment',
      description: 'Read a page of a UTF-8 text, DOCX, XLS, XLSX, PPT or PPTX attachment from this conversation. Spreadsheets include sheet names and cell addresses; formula results are cached, not recalculated. XLSX also returns a manifest-based sheets directory (up to 64 entries, each with name, Unicode offset and short preview), sheetCount and sheetDirectoryTruncated. Use sheets[].offset to jump directly to a sheet; do not guess its name or repeatedly scan pages for boundaries. If the directory or a name is truncated, inspect sheets using attachment_compute. Returns hasMore, nextOffset and totalCharacters. For full-file statistics use attachment_compute directly instead of paging all rows into context. Continue with offset=nextOffset only when more source text is needed. Offsets count Unicode characters. Default limit 12000, maximum 24000. Legacy DOC and extracted text larger than 1 MiB are rejected.',
      parameters: Type.Object({ attachmentId: Type.String(), offset: Type.Optional(Type.Integer({ minimum: 0 })), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 24000 })) }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'read_attachment', params, requestHost, signal),
    },
    {
      name: 'attachment_compute',
      label: 'Compute attachment with JavaScript',
      description: 'Execute JavaScript against whole conversation attachments without copying rows into the model context. Pass attachmentIds explicitly to load uploaded files; omitting both attachmentIds and artifactIds gives an EMPTY attachments array and is only for calculations without files. Available without a project; originals stay read-only and generated files go to a conversation workspace. Use this tool for spreadsheet counts, deduplication, grouping, arithmetic, percentages and charts; never compute bulk statistics mentally from paginated read_attachment text. JavaScript globals: attachments = [{id,name,kind,sheets:[{name,rows:[[number|string|boolean|null,...],...]}],text?}]. Workbook rows include the header row. Select sheets by name. Call saveFile(name, UTF8content, optionalMediaType) to create JSON/CSV/SVG/HTML/text output; names must be plain filenames. Generated files return files[].id. In a later call, pass artifactIds:[id] to independently reread or continue calculating from a saved JSON/CSV/text result; it appears in attachments with that ID. Only computed files owned by this conversation are readable. Explicitly return a concise JSON-serializable result from the code. Use raw JavaScript without Markdown fences or escapes; quote entire object keys containing spaces, punctuation or quotation marks. On a script error, inspect the reported code line and retry with corrected code in a new call; a failed call creates no output files. No process, require, filesystem or network APIs. Inspect sheet metadata or a few sample rows with code first if structure is unknown, then compute the full dataset. Use actual column meanings, document deduplication rules and missing values, and do not invent results if execution fails. Formula cells use cached results. Example code: const rows=attachments[0].sheets[0].rows; return {rows:rows.length-1,headers:rows[0]}; When the returned JSON exceeds the inline result limit, the full value is stored for you and the tool returns a bounded summary instead: {summary:{storedBytes,rows,columns,sampleRows,complete:false,storedAs:"compute-artifact",artifactId,artifactName,note}} plus files[].id. The complete data is not lost and never needs to be retyped. The large-result JSON artifact is consumed inside the compute environment: pass its id to a later attachment_compute call as artifactIds:[id] to read and transform it there (it is not an attachmentId, and read_tool_result only reads fox-result:// references, not compute-artifact ids). To write the table into Excel, turn it into CSV/TSV in that same code, save it with saveFile("table.csv", csvText), and pass the returned file id (files[].id) to office_import_data as artifactId (the Host streams the saved CSV/TSV bytes directly); never pass the JSON artifact itself as an import and never paste the data inline. The summary rows/columns and sample are enough to choose the next step.',
      parameters: Type.Object({
        attachmentIds: Type.Optional(Type.Array(Type.String({ minLength: 1 }), { minItems: 1, maxItems: 8, uniqueItems: true })),
        artifactIds: Type.Optional(Type.Array(Type.String({ minLength: 1 }), { minItems: 1, maxItems: 8, uniqueItems: true })),
        code: Type.String({ minLength: 1, maxLength: 131072 }),
        timeoutMs: Type.Optional(Type.Integer({ minimum: 100, maximum: 30000 })),
        processing: Type.Optional(Type.Union([Type.Literal('whole'), Type.Literal('chunked')])),
        profile: Type.Optional(Type.Union([Type.Literal('standard'), Type.Literal('large')])),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'attachment_compute', params, requestHost, signal),
    },
    {
      name: 'compute_job_start', label: 'Start or resume background computation',
      description: "Start bounded background attachment computation. For a new job supply idempotencyKey and params. Poll compute_job_status until terminal, then read compute_job_result. A paused job can be resumed using jobId only; never repeat completed writes. Background jobs always use chunked processing: code defines onChunk(chunk) and optional onFinish(). Use profile=large for large files. Whole-array scripts belong in synchronous attachment_compute. Run authorization and execution budget still apply.",
      // Object-rooted, not a top-level union. The Kernel worker installs these
      // schemas as proposal definitions and a `anyOf` at the root is not an
      // installable function signature, so the union form made the whole
      // `kernel.ready` handshake fail for every attachment-capable Run — the
      // data-analysis scenario included. The two shapes stay mutually exclusive
      // in the handler: `idempotencyKey` + `params` start a job, `jobId` alone
      // resumes one, and supplying neither is rejected there.
      parameters: Type.Object({
        idempotencyKey: Type.Optional(Type.String({ minLength: 1, maxLength: 200 })),
        params: Type.Optional(COMPUTE_PARAMETERS),
        jobId: Type.Optional(Type.String({ minLength: 1 })),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'compute_job_start', params, requestHost, signal),
    },
    {
      name: 'compute_job_status', label: 'Check computation progress',
      description: "Read durable job progress without starting work. waitMs (up to 5000) waits briefly for progress/completion. Do not declare delivery complete while a required job is queued/running/paused.",
      parameters: Type.Object({ jobId: Type.String({ minLength: 1 }), waitMs: Type.Optional(Type.Integer({ minimum: 0, maximum: 5000 })) }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'compute_job_status', params, requestHost, signal),
    },
    {
      name: 'compute_job_cancel', label: 'Cancel background computation',
      description: "Request cancellation. Running remains running until the executor confirms it stopped; inspect cancelRequestedAt/cancelAcknowledgedAt. Only jobs from this conversation can be cancelled.",
      parameters: Type.Object({ jobId: Type.String({ minLength: 1 }) }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'compute_job_cancel', params, requestHost, signal),
    },
    {
      name: 'compute_job_result', label: 'Read computation result',
      description: "Read one UTF-8 byte range of a completed, integrity-checked stored result. Concatenate content using nextOffset until complete=true; this never runs the computation again.",
      parameters: Type.Object({ jobId: Type.String({ minLength: 1 }), offset: Type.Optional(Type.Integer({ minimum: 0 })), limit: Type.Optional(Type.Integer({ minimum: 4, maximum: 65536 })) }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'compute_job_result', params, requestHost, signal),
    },
    {
      name: 'read_tool_result',
      label: 'Read stored tool result',
      description: 'Read bytes Fox already stored for a settled tool call in this Run, using the fox-result://<runId>/<toolCallId> reference printed in a bounded tool view. This never executes the original tool, so it cannot repeat a write: it only re-reads what Host recorded. Pass the reference and, to continue a previous read, offset=<the nextOffset you were given>; repeat until complete=true. The result is the stored text as the first content block, followed by a trailing `FOX_RESULT_CURSOR_V1 {…}` block carrying reference, offset, returnedBytes, nextOffset, complete, originalBytes, retrievable and truncated. Use that block\'s nextOffset to continue, and stop at complete=true. When retrievable is false or truncated is true, Host kept only a bounded preview, so the omitted bytes are NOT reachable from this reference and must not be guessed — get them another way instead.',
      parameters: Type.Object({
        reference: Type.String({ description: 'A fox-result://<runId>/<toolCallId> reference from this conversation.' }),
        offset: Type.Optional(Type.Integer({ minimum: 0, description: 'Byte offset to start reading from; use the nextOffset returned by the previous read.' })),
        limit: Type.Optional(Type.Integer({ minimum: 4, maximum: 65536, description: 'Maximum bytes to return in one read; minimum 4, default 16384, maximum 65536. Offsets and lengths count UTF-8 bytes.' })),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'read_tool_result', params, requestHost, signal),
    },
    {
      name: 'skill_load',
      label: 'Discover or load skill instructions',
      description: 'Two modes. (1) Load: pass skillId (from the "Skill catalog" section, or from a catalog page you listed) and Fox returns that skill\'s full SKILL.md body as the first content block; details carries id, version, contentSha256, chars, bytes, requiredTools, missingTools and toolsAvailable. (2) Discover: pass no skillId — optionally query (substring over id/name/description), offset and limit (max 20) — and Fox returns one bounded page of the skills enabled for this Run: id, name, version, short description, size and dependency state, plus nextOffset to continue. Use discovery whenever a skill you need is not listed in the prompt: a tight budget can truncate both the catalog and the omission list, so an unlisted id must still be findable here. Loading a skill never grants a tool: if toolsAvailable is false, the required tool calls will be rejected by the frozen Run scope — say so instead of attempting them. Do not invent skillIds, do not pass scope or conversation fields, and do not reload a skill already present in full above.',
      parameters: Type.Object({
        skillId: Type.Optional(Type.String({
          minLength: 1,
          maxLength: 128,
          pattern: '^[A-Za-z0-9_-]+$',
          description: 'The id shown in parentheses in the Skill catalog line. Omit to list the catalog instead.',
        })),
        query: Type.Optional(Type.String({
          maxLength: 64,
          description: 'Discovery only: case-insensitive substring matched against skill id, name and description. Omit to page through everything.',
        })),
        offset: Type.Optional(Type.Integer({
          minimum: 0,
          description: 'Discovery only: how many matched entries to skip; pass the nextOffset from the previous page.',
        })),
        limit: Type.Optional(Type.Integer({
          minimum: 1,
          maximum: 20,
          description: 'Discovery only: entries per page (1-20, default 20).',
        })),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'skill_load', params, requestHost, signal),
    },
    {
      name: 'write_file',
      label: 'Write file',
      description: 'Create or replace a UTF-8 text file inside the authorized project. Call this tool for a real file write; describing a write in text does nothing. Fox may require user approval before the host writes it.',
      parameters: Type.Object({
        path: Type.String(),
        content: Type.String(),
        createDirectories: Type.Optional(Type.Boolean()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'write_file', params, requestHost, signal),
    },
    {
      name: 'edit_file',
      label: 'Edit file',
      description: 'Replace an exact text section in a UTF-8 file inside the authorized project. Call this tool for a real edit; Fox shows a diff and enforces the project permission mode.',
      parameters: Type.Object({
        path: Type.String(),
        oldText: Type.String(),
        newText: Type.String(),
        replaceAll: Type.Optional(Type.Boolean()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'edit_file', params, requestHost, signal),
    },
    {
      name: 'run_command',
      label: 'Run command',
      description: 'Run a real non-interactive command with a working directory inside the authorized project. Every call requires explicit approval. The Host starts cmd.exe /D /S /C on Windows and sh -lc elsewhere; the shell itself is not configurable, so use batch/cmd syntax on Windows (%VAR%, &&) and POSIX syntax elsewhere. To call PowerShell, wrap it: powershell -NoProfile -Command "...". Blank cwd means the project root. timeoutSeconds is clamped to 1..120 and the whole call also obeys the Run budget. Console programs write the system OEM codepage, so non-ASCII output may be decoded imperfectly; when a command must produce exact text, redirect it to a file (command > out.txt 2>&1) and read that file instead. A non-zero exit, a timeout or a dropped output is reported with errorCode plus the output the command actually produced - read it before retrying, and do not repeat an identical call that already failed. When the retained output is too large to send back, the result arrives as a bounded preview carrying a fox-result:// reference; page it with read_tool_result instead of re-running. If the Host reports its output-reader budget is full, the command did not start and had no side effect - that is transient backpressure, so retry shortly or redirect output to a file. For long commands, use action=start with command and a stable idempotencyKey (timeoutSeconds defaults to 600, at most 3600 and bounded by the remaining Run budget); this returns a durable jobId promptly. Use action=status or cancel with jobId only. Use action=output with jobId, stream stdout/stderr, raw-byte offset and limit 4..65536, and continue at nextOffset. atEndOfAvailable is not streamClosed: output may still arrive. Keep polling until terminal; failed/cancelled jobs return isError plus readable retained output. A key reused with different command/cwd/timeout is rejected. Interrupted commands are never resumed or replayed. Omit action or use sync for the original bounded synchronous call.',
      parameters: Type.Object({
        action: Type.Optional(Type.Union(['sync', 'start', 'status', 'output', 'cancel'].map(value => Type.Literal(value)))),
        command: Type.Optional(Type.String()),
        cwd: Type.Optional(Type.String()),
        timeoutSeconds: Type.Optional(Type.Number()),
        idempotencyKey: Type.Optional(Type.String()),
        jobId: Type.Optional(Type.String()),
        stream: Type.Optional(Type.Union([Type.Literal('stdout'), Type.Literal('stderr')])),
        offset: Type.Optional(Type.Integer({ minimum: 0 })),
        limit: Type.Optional(Type.Integer({ minimum: 4, maximum: 65536 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'run_command', params, requestHost, signal),
    },
    {
      name: 'web_search',
      label: 'Search the web',
      description: 'Search the live web. Fox includes a no-key DuckDuckGo fallback and can use a configured Brave, Tavily, Exa, or SearXNG provider when available.',
      parameters: Type.Object({
        query: Type.String(),
        provider: Type.Optional(Type.Union([
          Type.Literal('auto'),
          Type.Literal('brave'),
          Type.Literal('tavily'),
          Type.Literal('exa'),
          Type.Literal('searxng'),
          Type.Literal('duckduckgo'),
        ])),
        maxResults: Type.Optional(Type.Integer({ minimum: 1, maximum: 10 })),
        includeDomains: Type.Optional(Type.Array(Type.String())),
        excludeDomains: Type.Optional(Type.Array(Type.String())),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'web_search', params, requestHost, signal),
    },
    {
      name: 'web_read',
      label: 'Read webpage',
      description: 'Fetch a public HTTP or HTTPS page through Fox Host and extract bounded Markdown or plain text. Private, loopback, link-local, and reserved network targets are blocked.',
      parameters: Type.Object({
        url: Type.String(),
        format: Type.Optional(Type.Union([Type.Literal('markdown'), Type.Literal('text')])),
        maxChars: Type.Optional(Type.Integer({ minimum: 1000, maximum: 300000 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'web_read', params, requestHost, signal),
    },
    {
      name: 'http_request',
      label: 'Send HTTP request',
      description: 'Send an approved HTTP request to an exact allowlisted public host. Private networks, cross-host redirects, credential headers, responses over 10 MiB, and timeouts over 15 seconds are blocked. Use a Keyring-backed OpenAPI Connector for authenticated APIs.',
      parameters: Type.Object({
        method: Type.Optional(Type.Union([
          Type.Literal('GET'), Type.Literal('HEAD'), Type.Literal('POST'),
          Type.Literal('PUT'), Type.Literal('PATCH'), Type.Literal('DELETE'),
        ])),
        url: Type.String(),
        allowedHosts: Type.Array(Type.String(), { minItems: 1, maxItems: 20 }),
        headers: Type.Optional(Type.Record(Type.String(), Type.String())),
        body: Type.Optional(Type.Unknown()),
        timeoutSeconds: Type.Optional(Type.Integer({ minimum: 1, maximum: 15 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'http_request', params, requestHost, signal),
    },
    {
      name: 'system_info',
      label: 'Read system information',
      description: 'Read an approved structured snapshot of CPU, memory, disks, and optionally bounded process summaries. Environment output is restricted to a fixed non-secret allowlist and never exposes variables containing API keys, tokens, secrets, or passwords.',
      parameters: Type.Object({
        includeProcesses: Type.Optional(Type.Boolean()),
        maxProcesses: Type.Optional(Type.Integer({ minimum: 1, maximum: 100 })),
        includeEnvironment: Type.Optional(Type.Boolean()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'system_info', params, requestHost, signal),
    },
    {
      name: 'sqlite_read',
      label: 'Query SQLite read-only',
      description: 'Run one approved read-only SQLite statement against a database inside the authorized project. Fox opens the database read-only and enforces a 1000-row, 100-column, 2 MiB, and 5-second result boundary.',
      parameters: Type.Object({
        path: Type.String(),
        query: Type.String(),
        parameters: Type.Optional(Type.Array(Type.Unknown(), { maxItems: 100 })),
        rowLimit: Type.Optional(Type.Integer({ minimum: 1, maximum: 1000 })),
        timeoutSeconds: Type.Optional(Type.Integer({ minimum: 1, maximum: 5 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'sqlite_read', params, requestHost, signal),
    },
    {
      name: 'structured_data',
      label: 'Process structured data',
      description: 'Parse, format, convert, query, or validate JSON, YAML, TOML, XML, CSV, and TSV data. Input may be inline or a file inside the authorized project.',
      parameters: Type.Object({
        action: Type.Union([
          Type.Literal('format'),
          Type.Literal('convert'),
          Type.Literal('query'),
          Type.Literal('validate'),
        ]),
        data: Type.Optional(Type.String()),
        path: Type.Optional(Type.String()),
        inputFormat: Type.Optional(Type.Union([
          Type.Literal('auto'), Type.Literal('json'), Type.Literal('yaml'),
          Type.Literal('toml'), Type.Literal('xml'), Type.Literal('csv'), Type.Literal('tsv'),
        ])),
        outputFormat: Type.Optional(Type.Union([
          Type.Literal('json'), Type.Literal('yaml'), Type.Literal('toml'),
          Type.Literal('xml'), Type.Literal('csv'), Type.Literal('tsv'),
        ])),
        query: Type.Optional(Type.String()),
        requiredKeys: Type.Optional(Type.Array(Type.String())),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'structured_data', params, requestHost, signal),
    },
    {
      name: 'git_read',
      label: 'Read Git state',
      description: 'Read structured Git status, diff, log, or blame information for the authorized project without using a shell or modifying the repository.',
      parameters: Type.Object({
        operation: Type.Union([
          Type.Literal('status'), Type.Literal('diff'), Type.Literal('log'), Type.Literal('blame'),
        ]),
        path: Type.Optional(Type.String()),
        ref: Type.Optional(Type.String()),
        base: Type.Optional(Type.String()),
        staged: Type.Optional(Type.Boolean()),
        maxEntries: Type.Optional(Type.Integer({ minimum: 1, maximum: 100 })),
        startLine: Type.Optional(Type.Integer({ minimum: 1 })),
        endLine: Type.Optional(Type.Integer({ minimum: 1 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'git_read', params, requestHost, signal),
    },
    {
      name: 'test_run',
      label: 'Run tests',
      description: 'Detect and run an existing Rust, Node, or Python test command in the authorized project. Fox executes direct process arguments, never a shell, and returns failures as structured results.',
      parameters: Type.Object({
        runner: Type.Optional(Type.Union([
          Type.Literal('auto'), Type.Literal('node'), Type.Literal('rust'), Type.Literal('python'),
        ])),
        script: Type.Optional(Type.String()),
        target: Type.Optional(Type.String()),
        cwd: Type.Optional(Type.String()),
        timeoutSeconds: Type.Optional(Type.Integer({ minimum: 1, maximum: 600 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'test_run', params, requestHost, signal),
    },
    {
      name: 'code_check',
      label: 'Check code',
      description: 'Run an existing project lint, type-check, Cargo check, Clippy, Ruff, or Mypy command selected from a bounded detection matrix. Fox never downloads tools implicitly.',
      parameters: Type.Object({
        check: Type.Optional(Type.Union([
          Type.Literal('auto'), Type.Literal('lint'), Type.Literal('typecheck'),
        ])),
        ecosystem: Type.Optional(Type.Union([
          Type.Literal('auto'), Type.Literal('node'), Type.Literal('rust'), Type.Literal('python'),
        ])),
        cwd: Type.Optional(Type.String()),
        timeoutSeconds: Type.Optional(Type.Integer({ minimum: 1, maximum: 600 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'code_check', params, requestHost, signal),
    },
    {
      name: 'format_code',
      label: 'Format code',
      description: 'Check or apply formatting with an existing project formatter for Rust, Node, or Python. Every call requires approval and no formatter package is downloaded implicitly.',
      parameters: Type.Object({
        mode: Type.Union([Type.Literal('check'), Type.Literal('write')]),
        ecosystem: Type.Optional(Type.Union([
          Type.Literal('auto'), Type.Literal('node'), Type.Literal('rust'), Type.Literal('python'),
        ])),
        path: Type.Optional(Type.String()),
        cwd: Type.Optional(Type.String()),
        timeoutSeconds: Type.Optional(Type.Integer({ minimum: 1, maximum: 600 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'format_code', params, requestHost, signal),
    },
    {
      name: 'tabular_data',
      label: 'Analyze tabular data',
      description: 'Preview, filter, or aggregate bounded CSV, TSV, and JSON tables from inline data or an authorized project file. XLSX support is intentionally deferred to the next phase.',
      parameters: Type.Object({
        operation: Type.Union([
          Type.Literal('preview'), Type.Literal('filter'), Type.Literal('aggregate'),
        ]),
        data: Type.Optional(Type.String()),
        path: Type.Optional(Type.String()),
        format: Type.Optional(Type.Union([
          Type.Literal('auto'), Type.Literal('csv'), Type.Literal('tsv'), Type.Literal('json'),
        ])),
        offset: Type.Optional(Type.Integer({ minimum: 0 })),
        limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 500 })),
        filterColumn: Type.Optional(Type.String()),
        filterValue: Type.Optional(Type.String()),
        filterMode: Type.Optional(Type.Union([Type.Literal('equals'), Type.Literal('contains')])),
        aggregate: Type.Optional(Type.Union([
          Type.Literal('count'), Type.Literal('sum'), Type.Literal('avg'),
          Type.Literal('min'), Type.Literal('max'),
        ])),
        column: Type.Optional(Type.String()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'tabular_data', params, requestHost, signal),
    },
    {
      name: 'work_snapshot_get',
      label: 'Get work snapshot',
      description: 'Load the current goal, tasks, and evidence for the Host-owned current conversation. The conversation identity is injected by Fox.',
      parameters: Type.Object({}),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'work_snapshot_get', params, requestHost, signal),
    },
    {
      name: 'graph_readonly_activate',
      label: 'Activate read-only graph',
      description: 'Atomically activate one approved bounded read-only Graph PlanRevision. Fox injects the current durable_v2 Run and Host ToolCall identity; this input cannot redefine nodes or execution state.',
      parameters: Type.Object({
        planRevisionId: trimmedRequiredString(200),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'graph_readonly_activate', params, requestHost, signal),
    },
    {
      name: 'graph_readonly_snapshot_get',
      label: 'Get read-only graph snapshot',
      description: 'Read the bounded Host projection for an activated read-only Graph. Readiness is derived from current Task, Attempt, Evidence, and Review facts; this tool never starts or cancels a node.',
      parameters: Type.Object({
        goalId: trimmedRequiredString(200),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'graph_readonly_snapshot_get', params, requestHost, signal),
    },
    {
      name: 'graph_readonly_node_start',
      label: 'Start read-only graph node',
      description: 'Atomically claim one currently runnable queued Graph Task and dispatch its isolated read-only Child Run. Fox injects the current durable_v2 Lead Run and real Host ToolCall identity, freezes exactly read/ls/find/grep for the Child, and keeps node acceptance separate from Child completion.',
      parameters: Type.Object({
        goalId: trimmedRequiredString(200),
        taskId: trimmedRequiredString(200),
        expectedTaskVersion: Type.Integer({ minimum: 1 }),
        attemptId: trimmedRequiredString(200),
        workerAgentId: trimmedRequiredString(200),
        objective: trimmedRequiredString(8000),
        context: trimmedRequiredString(12000),
        budget: Type.Object({
          maxDurationMs: Type.Integer({ minimum: 1000, maximum: 45000 }),
          maxTotalTokens: Type.Integer({ minimum: 256, maximum: 4096 }),
          maxOutputTokens: Type.Integer({ minimum: 64, maximum: 1024 }),
          maxToolCalls: Type.Integer({ minimum: 0, maximum: 6 }),
        }, { additionalProperties: false }),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'graph_readonly_node_start', params, requestHost, signal),
    },
    {
      name: 'graph_readonly_node_review',
      label: 'Review read-only graph node',
      description: 'Request an independent review of one running high-risk read-only Graph Attempt whose implementation Child completed. The frozen criteria must be bound to fresh Host-valid Evidence exactly as for node finish; Fox injects the real durable_v2 Lead Run and Host ToolCall identity. A Reviewer pass still does not accept the node or Goal, and the later node finish must reuse this review request unchanged, including the exact criterionEvidence and summary.',
      parameters: Type.Object({
        goalId: trimmedRequiredString(200),
        taskId: trimmedRequiredString(200),
        attemptId: trimmedRequiredString(200),
        expectedTaskVersion: Type.Integer({ minimum: 1 }),
        expectedAttemptVersion: Type.Integer({ minimum: 1 }),
        criterionEvidence: Type.Array(Type.Object({
          criterion: trimmedRequiredString(1000),
          evidenceIds: Type.Array(trimmedRequiredString(200), {
            minItems: 1,
            maxItems: 16,
            uniqueItems: true,
          }),
        }, { additionalProperties: false }), {
          minItems: 1,
          maxItems: 8,
        }),
        summary: trimmedRequiredString(4000),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'graph_readonly_node_review', params, requestHost, signal),
    },
    {
      name: 'graph_readonly_node_finish',
      label: 'Finish read-only graph node',
      description: 'Accept one running read-only Graph node only after its delegated Child completed and every frozen criterion is exactly bound to fresh, live, Host-validated Evidence from the current parent Lead Attempt. For a high-risk reviewed node, reuse the passed review request unchanged, including the exact criterionEvidence and summary. Fox injects the durable_v2 Lead Run and real Host ToolCall identity; this tool never creates Goal Acceptance.',
      parameters: Type.Object({
        goalId: trimmedRequiredString(200),
        taskId: trimmedRequiredString(200),
        attemptId: trimmedRequiredString(200),
        expectedTaskVersion: Type.Integer({ minimum: 1 }),
        expectedAttemptVersion: Type.Integer({ minimum: 1 }),
        criterionEvidence: Type.Array(Type.Object({
          criterion: trimmedRequiredString(1000),
          evidenceIds: Type.Array(trimmedRequiredString(200), {
            minItems: 1,
            maxItems: 16,
            uniqueItems: true,
          }),
        }, { additionalProperties: false }), {
          minItems: 1,
          maxItems: 8,
        }),
        summary: trimmedRequiredString(4000),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'graph_readonly_node_finish', params, requestHost, signal),
    },
    {
      name: 'graph_readonly_node_cancel',
      label: 'Cancel read-only graph node',
      description: 'Persist a bounded cancellation intent for one running standard-risk read-only Graph Attempt. Fox injects the current durable_v2 parent Lead Run and real Host ToolCall identity, derives the unique delegated Child, finalizes this ToolCall before signalling the Child runtime, and reconciles only authoritative Child terminal facts.',
      parameters: Type.Object({
        goalId: trimmedRequiredString(200),
        taskId: trimmedRequiredString(200),
        attemptId: trimmedRequiredString(200),
        expectedTaskVersion: Type.Integer({ minimum: 1 }),
        expectedAttemptVersion: Type.Integer({ minimum: 1 }),
        reason: trimmedRequiredString(2000),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'graph_readonly_node_cancel', params, requestHost, signal),
    },
    {
      name: 'graph_readonly_accept',
      label: 'Accept read-only graph',
      description: 'Request final Goal Acceptance for the current activated read-only Graph only after the Host snapshot proves every node accepted under the current Plan with no open findings. Fox injects the durable_v2 Lead Run and Host ToolCall identity; generic Goal Acceptance cannot replace this dedicated boundary.',
      parameters: Type.Object({
        goalId: trimmedRequiredString(200),
        expectedGoalVersion: Type.Integer({ minimum: 1 }),
        summary: trimmedRequiredString(4000),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'graph_readonly_accept', params, requestHost, signal),
    },
    {
      name: 'workflow_snapshot_get',
      label: 'Get expert workflow snapshot',
      description: 'Load the active persisted expert Workflow, its current Stage, mapped Host Task, attempts, output checkpoints, and pending Gate. Conversation identity is injected by Fox.',
      parameters: Type.Object({}),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'workflow_snapshot_get', params, requestHost, signal),
    },
    {
      name: 'workflow_start',
      label: 'Start expert workflow',
      description: 'Start the versioned Workflow frozen in the active expert package. Fox validates the declared input JSON Schema and atomically creates one Host Goal plus one ordered Task per Stage.',
      parameters: Type.Object({ input: Type.Optional(Type.Unknown()) }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'workflow_start', params, requestHost, signal),
    },
    {
      name: 'workflow_stage_start',
      label: 'Start workflow stage',
      description: 'Start or retry only the current queued Workflow Stage. Fox binds the mapped Task to the real current Run and enforces the persisted retry limit.',
      parameters: Type.Object({ stageId: Type.Optional(Type.String()) }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'workflow_stage_start', params, requestHost, signal),
    },
    {
      name: 'workflow_stage_complete',
      label: 'Complete workflow stage',
      description: 'Checkpoint the current Stage output after valid Task Evidence exists. Fox validates the Stage output JSON Schema; the final Stage also requires and validates workflowOutput before completing the Host Goal.',
      parameters: Type.Object({
        stageOutput: Type.Unknown(),
        workflowOutput: Type.Optional(Type.Unknown()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'workflow_stage_complete', params, requestHost, signal),
    },
    {
      name: 'workflow_stage_fail',
      label: 'Fail workflow stage attempt',
      description: 'Record a real Stage failure. Fox requeues the Stage while attempts remain, otherwise marks the Workflow failed and blocks the mapped Host Goal.',
      parameters: Type.Object({ error: Type.String({ minLength: 1, maxLength: 2000 }) }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'workflow_stage_fail', params, requestHost, signal),
    },
    {
      name: 'workflow_cancel',
      label: 'Cancel expert workflow',
      description: 'Cancel the active Workflow and map all unfinished Stages to terminal Host Task states. Use only when the user asks to stop or a declared stop condition is met.',
      parameters: Type.Object({ reason: Type.Optional(Type.String({ maxLength: 2000 })) }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'workflow_cancel', params, requestHost, signal),
    },
    {
      name: 'team_snapshot_get',
      label: 'Get expert team snapshot',
      description: 'Load the persisted Supervisor Team and its real Child Run members, isolated statuses, bounded results, usage, and failures.',
      parameters: Type.Object({}),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'team_snapshot_get', params, requestHost, signal),
    },
    {
      name: 'team_start',
      label: 'Start expert team',
      description: 'Start the versioned Supervisor Team frozen in the active expert package. The current Run remains Lead and Fox persists the Team before any member is dispatched.',
      parameters: Type.Object({
        objective: Type.String({ minLength: 1, maxLength: 8000 }),
        context: Type.Optional(Type.String({ maxLength: 12000 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'team_start', params, requestHost, signal),
    },
    {
      name: 'team_member_start',
      label: 'Start expert team member',
      description: 'Serially dispatch one declared Team member as a real isolated Child Run. Fox binds the frozen member identity, instructions, tool scope, budget, and parent cancellation chain.',
      parameters: Type.Object({
        memberId: Type.String({ minLength: 1, maxLength: 128 }),
        task: Type.String({ minLength: 1, maxLength: 8000 }),
        context: Type.Optional(Type.String({ maxLength: 12000 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'team_member_start', params, requestHost, signal),
    },
    {
      name: 'team_collect',
      label: 'Collect expert team',
      description: 'Collect all persisted Team members. Fox keeps the Team running until every declared member has been dispatched and is terminal, then produces one bounded aggregate result.',
      parameters: Type.Object({
        waitMs: Type.Optional(Type.Integer({ minimum: 0, maximum: 60000 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'team_collect', params, requestHost, signal),
    },
    {
      name: 'team_cancel',
      label: 'Cancel expert team',
      description: 'Cancel the active Team and propagate cancellation to its active isolated Child Run before terminalizing the Team record.',
      parameters: Type.Object({ reason: Type.Optional(Type.String({ maxLength: 2000 })) }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'team_cancel', params, requestHost, signal),
    },
    {
      name: 'child_agent_list',
      label: 'List child agents',
      description: 'List two separate Host-approved catalogs: reusable assistant/worker templates for ordinary Child Runs, and expert packages for one-off isolated consultations. Catalog entries are templates, not persistent child instances.',
      parameters: Type.Object({}),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'child_agent_list', params, requestHost, signal),
    },
    {
      name: 'child_run_start',
      label: 'Start child run',
      description: 'Start one isolated asynchronous Child Run. Use mode=worker with an optional agentId from child_agent_list.agents for ordinary delegated work. Use mode=expert_consultation with an exact expertId from child_agent_list.experts for a one-off specialist report; this never attaches, replaces, or removes the conversation expert. Prefer project-relative paths and bounded context. Fox enforces depth, concurrency, duration, and permission limits.',
      parameters: Type.Object({
        mode: Type.Optional(Type.Union([
          Type.Literal('worker'),
          Type.Literal('expert_consultation'),
        ])),
        objective: Type.String({ minLength: 1, maxLength: 8000, description: 'Required exact key: objective. State the bounded task for the child; never omit or rename this field.' }),
        context: Type.Optional(Type.String({ maxLength: 12000 })),
        agentId: Type.Optional(Type.String({ minLength: 1, maxLength: 160 })),
        expertId: Type.Optional(Type.String({ minLength: 1, maxLength: 160 })),
        budget: Type.Optional(Type.Object({
          maxDurationMs: Type.Optional(Type.Integer({ minimum: 1000, maximum: 900000 })),
        })),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'child_run_start', params, requestHost, signal),
    },
    {
      name: 'child_run_collect',
      label: 'Collect child runs',
      description: 'Collect Host-persisted Child Run statuses and bounded final results. Use only exact childRunId values returned by successful child_run_start results in this parent Run. Wait for that result before proposing collect; never invent IDs or collect an unknown ID in the same batch as start. A positive waitMs waits for all requested children to become terminal or until the timeout, without blocking other children.',
      parameters: Type.Object({
        childRunIds: Type.Array(Type.String({ minLength: 1, maxLength: 160 }), { minItems: 1, maxItems: 8 }),
        waitMs: Type.Optional(Type.Integer({ minimum: 0, maximum: 60000 })),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'child_run_collect', params, requestHost, signal),
    },
    {
      name: 'child_run_cancel',
      label: 'Cancel child run',
      description: 'Cancel one active Child Run owned by the current parent Run. Fox propagates cancellation to the isolated runtime process and persists the terminal state.',
      parameters: Type.Object({ childRunId: Type.String({ minLength: 1, maxLength: 160 }) }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'child_run_cancel', params, requestHost, signal),
    },
    {
      name: 'goal_propose',
      label: 'Propose goal',
      description: 'Propose one substantive goal only when the latest user message explicitly asks Fox to create or track a goal or execution plan. Never infer goal intent from pasted content, Markdown headings, educational text, examples, or incidental planning words. If Fox already created an empty host placeholder, this call refines that same goal before tasks exist.',
      parameters: Type.Object({
        title: Type.String(),
        objective: Type.String(),
        acceptanceSummary: Type.Optional(Type.String()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'goal_propose', params, requestHost, signal),
    },
    {
      name: 'goal_complete',
      label: 'Complete goal',
      description: 'Ask Fox Host to complete an active goal after every required task is completed or skipped. Validate the evidence for every completed task first. Fox validates conversation ownership, task state, valid evidence, and the optimistic goal version before persisting completion.',
      parameters: Type.Object({
        goalId: Type.String(),
        expectedVersion: Type.Integer({ minimum: 1 }),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'goal_complete', params, requestHost, signal),
    },
    {
      name: 'task_create_many',
      label: 'Create work tasks',
      description: 'Atomically create ordered tasks under an active goal owned by this conversation. riskLevel is an elevation hint and never lowers the frozen Host ValidationPolicy. A proposed goal must first be confirmed and activated by Fox Host.',
      parameters: Type.Object({
        goalId: Type.String(),
        tasks: Type.Array(Type.Object({
          title: Type.String(),
          detail: Type.Optional(Type.String()),
          ordinal: Type.Integer({ minimum: 0 }),
          riskLevel: Type.Optional(Type.Union([
            Type.Literal('low'),
            Type.Literal('standard'),
            Type.Literal('high'),
            Type.Literal('critical'),
          ])),
        }, { additionalProperties: false }), { minItems: 1 }),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_create_many', params, requestHost, signal),
    },
    {
      name: 'task_update',
      label: 'Update work task',
      description: 'Apply a legal task state transition through Fox using optimistic concurrency.',
      parameters: Type.Object({
        taskId: Type.String(),
        status: Type.Union([
          Type.Literal('queued'),
          Type.Literal('in_progress'),
          Type.Literal('completed'),
          Type.Literal('blocked'),
          Type.Literal('interrupted'),
          Type.Literal('skipped'),
        ]),
        expectedVersion: Type.Integer({ minimum: 1 }),
        blockedReason: Type.Optional(Type.String()),
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_update', params, requestHost, signal),
    },
    {
      name: 'task_attempt_start',
      label: 'Start task attempt',
      description: 'Start one Host-owned validation attempt for a task. Fox injects the current conversation, Run, and frozen ValidationPolicy, then enforces optimistic task versioning.',
      parameters: Type.Object({
        taskId: Type.String({ maxLength: 200 }),
        attemptId: Type.String({ maxLength: 200 }),
        expectedVersion: Type.Integer({ minimum: 1 }),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_attempt_start', params, requestHost, signal),
    },
    {
      name: 'task_repair_start',
      label: 'Start task repair',
      description: 'Start a bounded repair attempt from persisted validation findings. Fox owns the current conversation, Run, frozen ValidationPolicy, repair budget, and final authorization.',
      parameters: Type.Object({
        taskId: trimmedRequiredString(200),
        attemptId: trimmedRequiredString(200),
        expectedVersion: Type.Integer({ minimum: 1 }),
        rootCause: trimmedRequiredString(4000),
        findingIds: Type.Array(trimmedRequiredString(200), {
          minItems: 1,
          maxItems: 32,
          uniqueItems: true,
        }),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_repair_start', params, requestHost, signal),
    },
    {
      name: 'task_repair_escalate_start',
      label: 'Escalate task repair to a human',
      description: 'Request one Host-owned repair attempt that requires explicit human approval. Fox injects the current conversation, Run, frozen ValidationPolicy, approval identity, repair budget, and final authorization; this call cannot create or reuse a grant.',
      parameters: Type.Object({
        taskId: trimmedRequiredString(200),
        attemptId: trimmedRequiredString(200),
        expectedVersion: Type.Integer({ minimum: 1 }),
        rootCause: trimmedRequiredString(4000),
        findingIds: Type.Array(trimmedRequiredString(200), {
          minItems: 1,
          maxItems: 32,
          uniqueItems: true,
        }),
        escalationReason: trimmedRequiredString(2000),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_repair_escalate_start', params, requestHost, signal),
    },
    {
      name: 'task_attempt_finish',
      label: 'Finish task attempt',
      description: 'Finish the current Host-owned task attempt. A succeeded attempt cannot declare a failure reason; every other terminal status must provide one.',
      parameters: Type.Object({
        taskId: Type.String({ maxLength: 200 }),
        attemptId: Type.String({ maxLength: 200 }),
        expectedVersion: Type.Integer({ minimum: 1 }),
        expectedAttemptVersion: Type.Integer({ minimum: 1 }),
        status: TASK_ATTEMPT_STATUS_SCHEMA,
        failureReason: Type.Optional(Type.String({ minLength: 1, maxLength: 4000 })),
      }, {
        additionalProperties: false,
        allOf: [{
          if: {
            properties: { status: { const: 'succeeded' } },
            required: ['status'],
          },
          then: { not: { required: ['failureReason'] } },
          else: { required: ['failureReason'] },
        }],
      }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_attempt_finish', params, requestHost, signal),
    },
    {
      name: 'task_evidence_add',
      label: 'Add task evidence',
      description: 'Attach validated evidence to a task in this conversation. validationCheckType must be allowed by the frozen Host ValidationPolicy for the Run.',
      parameters: Type.Object({
        taskId: Type.String(),
        evidenceType: Type.Union([
          Type.Literal('tool_call'), Type.Literal('trace_span'), Type.Literal('test_result'),
          Type.Literal('file_diff'), Type.Literal('artifact'), Type.Literal('user_confirmation'),
          Type.Literal('external_reference'),
        ]),
        refKind: Type.Union([
          Type.Literal('tool_call'), Type.Literal('artifact'), Type.Literal('run_event'),
          Type.Literal('message'), Type.Literal('source'),
        ]),
        refId: Type.String(),
        summary: Type.String(),
        validationCheckType: Type.Optional(VALIDATION_CHECK_TYPE_SCHEMA),
      }, { additionalProperties: false }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_evidence_add', params, requestHost, signal),
    },
    {
      name: 'task_evidence_validate',
      label: 'Validate task evidence',
      description: 'Revalidate one evidence reference and return its current validity status.',
      parameters: Type.Object({ evidenceId: Type.String() }),
      execute: (toolCallId, params, signal) =>
        executeHostTool(toolCallId, 'task_evidence_validate', params, requestHost, signal),
    },
    {
      name: 'plan_revision_create',
      label: 'Create plan revision',
      description: 'Persist a proposed PlanRevision for an active goal. Include the complete ordered task plan rather than only the delta. Ordinary plans use title/detail/ordinal. A read-only Graph plan must give every node nodeKey and 1-8 acceptanceCriteria, with optional dependsOn; Fox Host validates the bounded DAG and never accepts execution state in this input. Fox Host keeps the revision proposed until the user approves or rejects it.',
      parameters: Type.Object({
        goalId: Type.String(),
        title: Type.String(),
        summary: Type.String(),
        tasks: PLAN_TASKS_SCHEMA,
      }, { additionalProperties: false }),
      prepareArguments: preparePlanRevisionArguments,
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'plan_revision_create', params, requestHost, signal),
    },
    {
      name: 'review_finding_add',
      label: 'Add review finding',
      description: 'Persist an independent review result. Record real issues as open findings. When review finds no blocker, add one resolved info finding that states the review scope and checks performed.',
      parameters: Type.Object({
        goalId: Type.String(), taskId: Type.Optional(Type.String()), planRevisionId: Type.String(),
        severity: Type.Union([Type.Literal('critical'), Type.Literal('high'), Type.Literal('medium'), Type.Literal('low'), Type.Literal('info')]),
        category: Type.String(), title: Type.String(), detail: Type.String(),
        status: Type.Optional(Type.Union([Type.Literal('open'), Type.Literal('resolved'), Type.Literal('waived')])),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'review_finding_add', params, requestHost, signal),
    },
    {
      name: 'review_finding_resolve',
      label: 'Resolve review finding',
      description: 'Resolve or explicitly waive a persisted review finding after the issue has been addressed or accepted.',
      parameters: Type.Object({ findingId: Type.String(), status: Type.Union([Type.Literal('resolved'), Type.Literal('waived')]) }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'review_finding_resolve', params, requestHost, signal),
    },
    {
      name: 'acceptance_submit',
      label: 'Submit final acceptance',
      description: 'Submit final A1 acceptance from the same independent Run that reviewed the approved plan. Every passed criterion must name its method and cite Host-validated evidence; Fox Host binds reviewer identity to the real Run ID.',
      parameters: Type.Object({
        goalId: Type.String(),
        expectedVersion: Type.Integer({ minimum: 1 }),
        summary: Type.String(),
        checks: Type.Array(Type.Object({
          criterion: Type.String(),
          method: Type.Union([
            Type.Literal('test'), Type.Literal('inspection'), Type.Literal('review'),
            Type.Literal('manual'), Type.Literal('other'),
          ]),
          status: Type.Union([Type.Literal('passed'), Type.Literal('not_applicable')]),
          evidenceIds: Type.Array(Type.String()),
          detail: Type.Optional(Type.String()),
        }), { minItems: 1 }),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'acceptance_submit', params, requestHost, signal),
    },
    {
      name: 'memory_search',
      label: 'Search confirmed memory',
      description: 'Search only user-confirmed, enabled memories visible to this conversation. Fox records the query, selection reason, rank, evidence excerpt, Run, and recall time for user audit.',
      parameters: Type.Object({
        query: Type.String(),
        limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'memory_search', params, requestHost, signal),
    },
    {
      name: 'memory_propose',
      label: 'Propose long-term memory',
      description: 'Submit one concise, evidence-backed candidate memory for user governance. This never confirms or enables memory, never stores a whole conversation, and creates an explicit conflict when it disagrees with confirmed memory.',
      parameters: Type.Object({
        scope: Type.Optional(Type.Union([
          Type.Literal('global'), Type.Literal('agent'), Type.Literal('project'),
        ])),
        kind: Type.Union([
          Type.Literal('preference'), Type.Literal('identity'), Type.Literal('project'),
          Type.Literal('workflow'), Type.Literal('fact'), Type.Literal('other'),
        ]),
        canonicalKey: Type.String({ minLength: 1, maxLength: 160 }),
        content: Type.String({ minLength: 1, maxLength: 4000 }),
        evidenceExcerpt: Type.String({ minLength: 1, maxLength: 1000 }),
        sourceMessageId: Type.Optional(Type.String()),
        confidence: Type.Optional(Type.Number({ minimum: 0, maximum: 1 })),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'memory_propose', params, requestHost, signal),
    },
  ], { source: 'fox-host', execution: 'host', trusted: true })
}

export const KNOWLEDGE_REFERENCE_SCHEMA = Type.Union([
  Type.Object({
    source: Type.Literal('local'),
    id: Type.String(),
    revision: Type.Optional(Type.String()),
  }),
  Type.Object({
    source: Type.Literal('remote'),
    connectionId: Type.String(),
    id: Type.String(),
    revision: Type.Optional(Type.String()),
  }),
])

const KNOWLEDGE_REFERENCES_SCHEMA = Type.Array(KNOWLEDGE_REFERENCE_SCHEMA, { minItems: 1 })

function prepareKnowledgeArguments(args) {
  if (!args || typeof args !== 'object' || Array.isArray(args)) return args
  const prepared = { ...args }
  for (const [key, array] of [['target', false], ['targets', true]]) {
    const value = prepared[key]
    if (typeof value !== 'string' || value.length > 64_000) continue
    const serialized = value.trim()
    if (!serialized.startsWith(array ? '[' : '{')) continue
    try {
      const parsed = JSON.parse(serialized)
      if (parsed && typeof parsed === 'object' && Array.isArray(parsed) === array) prepared[key] = parsed
    } catch { /* Keep malformed arguments for ordinary schema validation. */ }
  }
  return prepared
}

export function createKnowledgeTools(requestHost) {
  return defineFoxTools([
    {
      name: KNOWLEDGE_TOOL_NAMES.list,
      prepareArguments: prepareKnowledgeArguments,
      label: 'List knowledge bases',
      description: 'List knowledge bases explicitly enabled for the current Fox conversation, including their local or remote source. Optionally narrow the result with source-aware targets.',
      parameters: Type.Object({
        target: Type.Optional(KNOWLEDGE_REFERENCE_SCHEMA),
        targets: Type.Optional(KNOWLEDGE_REFERENCES_SCHEMA),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, KNOWLEDGE_TOOL_NAMES.list, params, requestHost, signal),
    },
    {
      name: KNOWLEDGE_TOOL_NAMES.search,
      prepareArguments: prepareKnowledgeArguments,
      label: 'Search knowledge',
      description: 'Search the conversation knowledge bases independently of any project folder. First call list_knowledge_bases, then pass its reference objects in targets (including source: local for local bases). query should contain concise domain keywords. knowledgeBaseId alone is only for legacy remote calls.',
      parameters: Type.Object({
        knowledgeBaseId: Type.Optional(Type.String()),
        target: Type.Optional(KNOWLEDGE_REFERENCE_SCHEMA),
        targets: Type.Optional(KNOWLEDGE_REFERENCES_SCHEMA),
        query: Type.String(),
        topK: Type.Optional(Type.Integer({ minimum: 1 })),
        documentIds: Type.Optional(Type.Array(Type.String(), { minItems: 1 })),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, KNOWLEDGE_TOOL_NAMES.search, params, requestHost, signal),
    },
    {
      name: KNOWLEDGE_TOOL_NAMES.read,
      prepareArguments: prepareKnowledgeArguments,
      label: 'Read knowledge document',
      description: 'Read parsed text of an enabled knowledge document. documentId must be a real file_id or documentId returned by search_knowledge, never the knowledge-base ID. Use target for the same source-aware knowledge reference as the search. A backend/API failure does not mean the conversation binding is missing.',
      parameters: Type.Object({
        knowledgeBaseId: Type.Optional(Type.String()),
        target: Type.Optional(KNOWLEDGE_REFERENCE_SCHEMA),
        documentId: Type.String(),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, KNOWLEDGE_TOOL_NAMES.read, params, requestHost, signal),
    },
    {
      name: KNOWLEDGE_TOOL_NAMES.graph,
      prepareArguments: prepareKnowledgeArguments,
      label: 'Query knowledge graph',
      description: 'Query a bounded knowledge graph subgraph. Local targets explicitly return local_knowledge.graph_unavailable until local graph support is available.',
      parameters: Type.Object({
        knowledgeBaseId: Type.Optional(Type.String()),
        target: Type.Optional(KNOWLEDGE_REFERENCE_SCHEMA),
        targets: Type.Optional(KNOWLEDGE_REFERENCES_SCHEMA),
        keyword: Type.Optional(Type.String()),
        maxDepth: Type.Optional(Type.Number()),
        maxNodes: Type.Optional(Type.Number()),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, KNOWLEDGE_TOOL_NAMES.graph, params, requestHost, signal),
    },
  ], { source: 'fox-knowledge', execution: 'host', trusted: true })
}

export function createMcpTools(requestHost) {
  return defineFoxTools([
    {
      name: 'list_mcp_tools',
      label: 'List extension tools',
      description: 'List enabled persistent stdio/HTTP MCP and OpenAPI extension sources with their validated tool catalog. Use this before calling an extension tool.',
      parameters: Type.Object({ query: Type.Optional(Type.String()) }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'list_mcp_tools', params, requestHost, signal),
    },
    {
      name: 'call_mcp_tool',
      label: 'Call extension tool',
      description: 'Call one validated MCP or OpenAPI tool through Fox after list_mcp_tools. Calls require explicit user approval and pass declarative lifecycle Hooks.',
      parameters: Type.Object({
        serverId: Type.String(),
        tool: Type.String(),
        arguments: Type.Optional(Type.Record(Type.String(), Type.Unknown())),
      }),
      execute: (toolCallId, params, signal) => executeHostTool(toolCallId, 'call_mcp_tool', params, requestHost, signal),
    },
  ], { source: 'fox-mcp', execution: 'host', trusted: true })
}
