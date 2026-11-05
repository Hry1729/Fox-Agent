import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  ERROR_DIAGNOSTIC_CLASS,
  ERROR_DIAGNOSTIC_MAX_HEIGHT,
  ERROR_DIAGNOSTIC_STYLE,
} from '../src/components/assistant-ui/elements/error-diagnostic'
import {
  deriveRunCancellations,
  deriveRunFailures,
  diagnosticTitle,
  hasRunFailureDiagnostic,
  runFailureDetailText,
  runFailureUserMessage,
  RUN_FAILURE_GENERIC_MESSAGE,
  type RunFailureInput,
} from '../src/features/conversations/model/run-failure'
import { isInternalRawText, userFacingError } from '../src/features/chat/send-failure-notice'

function persistedRun(overrides: Partial<{
  id: string
  status: string
  errorCode: string | null
  errorMessage: string | null
}> = {}) {
  return {
    id: 'run-1',
    status: 'failed',
    errorCode: null,
    errorMessage: null,
    ...overrides,
  }
}

describe('deriveRunFailures', () => {
  test('derives a failure from a persisted failed run with code and message', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({
        id: 'run-failed',
        status: 'failed',
        errorCode: 'kernel.context_compaction_insufficient',
        errorMessage: '上下文精简空间不足',
      })],
      events: [],
    })

    expect([...failures.keys()]).toEqual(['run-failed'])
    expect(failures.get('run-failed')).toEqual({
      kind: 'failed',
      code: 'kernel.context_compaction_insufficient',
      message: '上下文精简空间不足',
    })
  })

  test('derives a failure even when every assistant message of the run is completed', () => {
    // The reducer/projection write `completed` for each assistant message, so the
    // failure must not depend on message.status. The derivation input has no
    // messages at all; they are included here only to document the scenario.
    const assistantMessages = Array.from({ length: 13 }, (_, index) => ({
      id: `assistant-${index}`,
      runId: 'run-1',
      role: 'assistant' as const,
      status: 'completed',
    }))

    const input = {
      persistedRuns: [persistedRun({
        id: 'run-1',
        status: 'failed',
        errorCode: 'kernel.context_compaction_insufficient',
        errorMessage: '上下文精简空间不足',
      })],
      events: [],
      messages: assistantMessages,
    } as RunFailureInput

    const failures = deriveRunFailures(input)
    expect(assistantMessages.every((message) => message.status === 'completed')).toBe(true)
    expect(failures.size).toBe(1)
    expect(failures.get('run-1')?.kind).toBe('failed')
  })

  test('treats a cancelled run as cancelled, not as a failure', () => {
    const input: RunFailureInput = {
      persistedRuns: [persistedRun({ id: 'run-cancelled', status: 'cancelled', errorCode: null, errorMessage: null })],
      events: [],
    }

    expect(deriveRunFailures(input).size).toBe(0)
    expect([...deriveRunCancellations(input)]).toEqual(['run-cancelled'])
  })

  test('derives from persisted state alone when the terminal event is absent from events', () => {
    // Covers re-entering a conversation, app restart, and a terminal event that
    // fell outside the loaded pagination window.
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({
        id: 'run-paginated',
        status: 'interrupted',
        errorCode: 'runtime.interrupted',
        errorMessage: '运行被中断',
      })],
      events: [],
    })

    expect(failures.get('run-paginated')).toEqual({
      kind: 'interrupted',
      code: 'runtime.interrupted',
      message: '运行被中断',
    })
  })

  test('derives from events alone when no persisted run state is available', () => {
    const failures = deriveRunFailures({
      persistedRuns: [],
      events: [{
        runId: 'run-live',
        eventType: 'run.failed',
        event: { type: 'run.failed', code: 'provider.timeout', message: '模型响应超时' },
      }],
    })

    expect(failures.get('run-live')).toEqual({
      kind: 'failed',
      code: 'provider.timeout',
      message: '模型响应超时',
    })
  })

  test('keeps a single record per run id', () => {
    const failures = deriveRunFailures({
      persistedRuns: [
        persistedRun({ id: 'run-1', status: 'failed', errorCode: 'e1', errorMessage: '第一次' }),
        persistedRun({ id: 'run-1', status: 'failed', errorCode: 'e2', errorMessage: '第二次' }),
      ],
      events: [
        { runId: 'run-1', eventType: 'run.failed', event: { type: 'run.failed', code: 'evt', message: '事件' } },
        { runId: 'run-1', eventType: 'run.failed', event: { type: 'run.failed', code: 'evt2', message: '事件2' } },
      ],
    })

    expect(failures.size).toBe(1)
  })

  test('lets the persisted terminal state override the event stream', () => {
    const input: RunFailureInput = {
      persistedRuns: [persistedRun({ id: 'run-1', status: 'failed', errorCode: 'persisted', errorMessage: '持久化' })],
      events: [{ runId: 'run-1', eventType: 'run.failed', event: { type: 'run.failed', code: 'event', message: '事件' } }],
    }

    expect(deriveRunFailures(input).get('run-1')).toEqual({
      kind: 'failed',
      code: 'persisted',
      message: '持久化',
    })
  })

  test('a persisted interrupted run wins over an older run.failed event', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({
        id: 'run-interrupted',
        status: 'interrupted',
        errorCode: 'run_control.binding_integrity_failed',
        errorMessage: '运行控制绑定校验失败，此 Run 已中断，不能继续',
      })],
      events: [{
        runId: 'run-interrupted',
        eventType: 'run.failed',
        event: { type: 'run.failed', code: 'provider.timeout', message: '模型响应超时' },
      }],
    })

    expect(failures.get('run-interrupted')).toEqual({
      kind: 'interrupted',
      code: 'run_control.binding_integrity_failed',
      message: '运行控制绑定校验失败，此 Run 已中断，不能继续',
    })
  })

  test('falls back to a default message and code when the source omits them', () => {
    const failures = deriveRunFailures({
      events: [{ runId: 'run-1', eventType: 'run.interrupted', event: { type: 'run.interrupted' } }],
    })

    expect(failures.get('run-1')).toEqual({
      kind: 'interrupted',
      code: 'unknown',
      message: '运行中断',
    })
  })
})

describe('run failure stderr diagnostic', () => {
  const tail = 'mock: DEEPSEEK_API_KEY=<redacted>\nmock: request headers {"authorization":"Bearer <redacted>","x-api-key":"<redacted>"}\nmock: Authorization: Bearer <redacted>\nmock: Error: Connection error (last line)'

  test('a failed run event carries the redacted stderr tail into the failure', () => {
    const failures = deriveRunFailures({
      persistedRuns: [],
      events: [{
        runId: 'run-ext',
        eventType: 'run.failed',
        event: {
          type: 'run.failed',
          code: 'agent_backend_prompt_failed',
          message: 'prompt failed: Internal error',
          diagnostic: { kind: 'stderr_tail', source: 'prompt_failed', redacted: true, maxBytes: 4096, stderrTail: tail },
        },
      }],
    })
    const failure = failures.get('run-ext')!
    expect(failure.stderrTail).toBe(tail)
    expect(failure.stderrTailSource).toBe('prompt_failed')
    const text = runFailureDetailText(failure)
    expect(text.startsWith('agent_backend_prompt_failed: prompt failed: Internal error')).toBe(true)
    expect(text).toContain('后端 stderr 尾部（已脱敏，最多 4 KiB；本轮请求失败）：')
    expect(text.endsWith('Error: Connection error (last line)')).toBe(true)
    expect(text).not.toContain('sk-')
  })

  test('the persisted run row wins for code and message but keeps the tail from the event', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({ id: 'run-ext', status: 'failed', errorCode: 'agent_backend_exited', errorMessage: '外部后端进程意外退出' })],
      events: [{
        runId: 'run-ext',
        eventType: 'run.failed',
        event: { type: 'run.failed', code: 'agent_backend_exited', message: 'x', diagnostic: { source: 'backend_exited', stderrTail: tail } },
      }],
    })
    expect(failures.get('run-ext')).toEqual({
      kind: 'failed',
      code: 'agent_backend_exited',
      message: '外部后端进程意外退出',
      stderrTail: tail,
      stderrTailSource: 'backend_exited',
    })
  })

  test('without a diagnostic the failure and its detail text are unchanged', () => {
    const failures = deriveRunFailures({
      persistedRuns: [],
      events: [{ runId: 'run-plain', eventType: 'run.failed', event: { type: 'run.failed', code: 'kernel.x', message: 'boom', diagnostic: { stderrTail: '   ' } } }],
    })
    const failure = failures.get('run-plain')!
    expect(failure).toEqual({ kind: 'failed', code: 'kernel.x', message: 'boom' })
    expect('stderrTail' in failure).toBe(false)
    expect(runFailureDetailText(failure)).toBe('kernel.x: boom')
  })
})

describe('error detail rendering of long stderr tails', () => {
  const longLine = 'mock: long ' + Array.from({ length: 150 }, (_, i) => `/segment-${String(i).padStart(4, '0')}`).join('') + ' end-of-long-line'
  const cjkLine = 'mock: 错误：无法连接本机网关（127.0.0.1:3425），请检查 Magpie 是否在运行；路径 C:\\Users\\张三\\.dsh'
  const src = (path: string) => readFileSync(join(import.meta.dir, '..', path), 'utf8')

  test('the detail text keeps a ~2000-byte line and a CJK line whole', () => {
    const text = runFailureDetailText({ code: 'agent_backend_prompt_failed', message: 'x', stderrTail: `${longLine}\n${cjkLine}`, stderrTailSource: 'prompt_failed' })
    expect(longLine.length).toBeGreaterThan(1900)
    expect(text.split('\n')).toContain(longLine)
    expect(text.split('\n')).toContain(cjkLine)
    expect(text).not.toContain('[redacted]')
  })

  test('the diagnostic block wraps anywhere and scrolls past a maximum height', () => {
    expect(ERROR_DIAGNOSTIC_STYLE.whiteSpace).toBe('pre-wrap')
    expect(ERROR_DIAGNOSTIC_STYLE.overflowWrap).toBe('anywhere')
    expect(ERROR_DIAGNOSTIC_STYLE.maxHeight).toBe(ERROR_DIAGNOSTIC_MAX_HEIGHT)
    expect(ERROR_DIAGNOSTIC_STYLE.overflowY).toBe('auto')
    const css = src('src/components/assistant-ui/elements/fox-elements.css')
    const rule = css.split('\n').find((line) => line.includes(`.${ERROR_DIAGNOSTIC_CLASS}`)) ?? ''
    for (const declaration of ['white-space: pre-wrap', 'overflow-wrap: anywhere', `max-height: ${ERROR_DIAGNOSTIC_MAX_HEIGHT}`, 'overflow-y: auto']) {
      expect(rule).toContain(declaration)
    }
  })

  test('timeline, thread error card and assistant message all render the tail through the wrapping block', () => {
    const errorState = src('src/components/assistant-ui/upstream/elements/error-state.tsx')
    expect(errorState).toContain('<pre className={ERROR_DIAGNOSTIC_CLASS} style={ERROR_DIAGNOSTIC_STYLE}')
    expect(errorState.match(/<pre\b/g)?.length).toBe(1)
    expect(src('src/features/chat/demo/fox-demo-prompts.tsx')).toMatch(/<ErrorState [^>]*diagnostic=\{diagnostic \|\| presentation\.detail\}/)
    // 13.38（R1-3 / B4-2）：诊断（含错误码与英文原文）只在开发者模式给出。
    expect(src('src/features/chat/thread/parts/fox-error.tsx')).toMatch(/<ErrorState [^>]*diagnostic=\{developerUi \? runFailureDetailText\(failure\) : undefined\}/)
  })
})

describe('run failure JSON-RPC error data diagnostic', () => {
  const dataText = '{\n"request": {\n"headers": {\n"authorization": "Bearer <redacted>",\n"x-api-key": "<redacted>"\n}\n},\n"env": "OPENAI_API_KEY=<redacted>",\n"hint": "upstream rejected the credential"\n}'
  const tail = 'mock: upstream said 401 (stderr line)'
  const failed = (event: Record<string, unknown>) => deriveRunFailures({
    persistedRuns: [],
    events: [{ runId: 'run-data', eventType: 'run.failed', event: { type: 'run.failed', code: 'agent_backend_prompt_failed', message: 'prompt failed: Authentication required (JSON-RPC -32000)', ...event } }],
  }).get('run-data')!

  test('data only (new Host: diagnostics array + legacy diagnostic = the data)', () => {
    const entry = { kind: 'rpc_error_data', source: 'rpc_error_data', redacted: true, maxBytes: 4096, text: dataText }
    const failure = failed({ diagnostic: entry, diagnostics: [entry] })
    expect(failure.rpcErrorData).toBe(dataText)
    expect('stderrTail' in failure).toBe(false)
    expect(hasRunFailureDiagnostic(failure)).toBe(true)
    const text = runFailureDetailText(failure)
    expect(text).toContain('后端错误数据（已脱敏，最多 4 KiB）：')
    expect(text).not.toContain('stderr 尾部')
    expect(text.endsWith('"hint": "upstream rejected the credential"\n}')).toBe(true)
  })

  test('stderr tail and data are both kept, each under its own title', () => {
    const stderr = { kind: 'stderr_tail', source: 'prompt_failed', redacted: true, maxBytes: 4096, stderrTail: tail }
    const data = { kind: 'rpc_error_data', source: 'rpc_error_data', redacted: true, maxBytes: 4096, text: dataText }
    const failure = failed({ diagnostic: stderr, diagnostics: [stderr, data] })
    expect(failure.stderrTail).toBe(tail)
    expect(failure.stderrTailSource).toBe('prompt_failed')
    expect(failure.rpcErrorData).toBe(dataText)
    const text = runFailureDetailText(failure)
    const stderrAt = text.indexOf('后端 stderr 尾部（已脱敏，最多 4 KiB；本轮请求失败）：')
    const dataAt = text.indexOf('后端错误数据（已脱敏，最多 4 KiB）：')
    expect(stderrAt).toBeGreaterThan(0)
    expect(dataAt).toBeGreaterThan(stderrAt)
    expect(text.split('\n')[0]).toBe('agent_backend_prompt_failed: prompt failed: Authentication required (JSON-RPC -32000)')
  })

  test('an old Host event (single stderr diagnostic, no diagnostics array) still works', () => {
    const failure = failed({ diagnostic: { source: 'backend_exited', stderrTail: tail } })
    expect(failure).toEqual({ kind: 'failed', code: 'agent_backend_prompt_failed', message: 'prompt failed: Authentication required (JSON-RPC -32000)', stderrTail: tail, stderrTailSource: 'backend_exited' })
  })

  test('the persisted run row keeps the data from the event; empty entries are ignored', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({ id: 'run-data', status: 'failed', errorCode: 'agent_backend_session_failed', errorMessage: '外部后端无法打开会话' })],
      events: [{ runId: 'run-data', eventType: 'run.failed', event: { type: 'run.failed', diagnostics: [{ kind: 'stderr_tail', stderrTail: '  ' }, { source: 'rpc_error_data', text: dataText }] } }],
    })
    expect(failures.get('run-data')).toEqual({ kind: 'failed', code: 'agent_backend_session_failed', message: '外部后端无法打开会话', rpcErrorData: dataText })
    expect(hasRunFailureDiagnostic({ rpcErrorData: '  ' })).toBe(false)
    expect(diagnosticTitle('rpc_error_data')).toBe('后端错误数据（已脱敏，最多 4 KiB）：')
    expect(diagnosticTitle('session_failed')).toBe('后端 stderr 尾部（已脱敏，最多 4 KiB；会话打开失败）：')
  })
})

describe('A0 retired Kernel execution path', () => {
  test('an interrupted historical Kernel Run reads as interrupted and points to a new Run', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({ id: 'run-kernel', status: 'interrupted', errorCode: 'executor_route_retired', errorMessage: '执行路径已退役' })],
    })
    expect(failures.get('run-kernel')).toEqual({ kind: 'interrupted', code: 'executor_route_retired', message: '执行路径已退役。已中断，可在新 Run 中继续' })
    expect(runFailureDetailText(failures.get('run-kernel')!)).toBe('executor_route_retired: 执行路径已退役。已中断，可在新 Run 中继续')
  })

  test('the hint survives a row without a stored reason', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({ id: 'run-kernel', status: 'interrupted', errorCode: 'executor_route_retired', errorMessage: null })],
    })
    expect(failures.get('run-kernel')?.message).toBe('执行路径已退役。已中断，可在新 Run 中继续')
  })
})

describe('13.38 R1-3 / B4-2: no English internal text in normal mode', () => {
  test('error codes map to Chinese hints; the English release message never shows', () => {
    expect(runFailureUserMessage({ kind: 'failed', code: 'provider.request_failed', message: 'socket hang up' })).toBe('模型服务连接中断，可重试。')
    expect(runFailureUserMessage({ kind: 'interrupted', code: 'runtime.released_without_terminal', message: 'Pi released its Run before a terminal result was recorded.' }))
      .toBe('运行在得到结果之前中断了，可重试。')
    expect(runFailureUserMessage({ kind: 'failed', code: 'provider.timeout', message: 'timeout' })).toBe('模型服务连接中断，可重试。')
    expect(runFailureUserMessage({ kind: 'failed', code: 'something.new', message: 'The model request failed.' })).toBe(RUN_FAILURE_GENERIC_MESSAGE)
    expect(runFailureUserMessage({ kind: 'interrupted', code: 'unknown', message: 'Some English text here' })).toBe('运行中断了，可重试。')
    expect(runFailureUserMessage({ kind: 'failed', code: 'unknown', message: '超过 10 分钟执行上限' })).toBe('超过 10 分钟执行上限')
  })

  test('userFacingError hides English sentences in normal mode and keeps them for developers', () => {
    const raw = 'Pi released its Run before a terminal result was recorded.'
    expect(userFacingError(raw, false, '兜底')).toBe('兜底')
    expect(userFacingError(raw, true, '兜底')).toBe(raw)
    expect(userFacingError('web_read blocks private, local, reserved, and link-local addresses', false, '兜底')).toBe('兜底')
    expect(userFacingError('模型服务连接中断，可重试。', false, '兜底')).toBe('模型服务连接中断，可重试。')
    expect(userFacingError('请检查 Codex 登录', false, '兜底')).toBe('请检查 Codex 登录')
    expect(isInternalRawText('OK')).toBe(false)
  })

  test('missing vector model notices retain the reason and remedy in normal mode', () => {
    const raw = '向量模型文件缺失（model.onnx）。请到「设置 → 资料库」重新下载或导入模型，再重试检索。'
    expect(userFacingError(raw, false, '兜底')).toBe('向量模型文件缺失。请到「设置 → 资料库」重新下载或导入模型，再重试检索。')
    expect(userFacingError(raw, true, '兜底')).toBe(raw)
    expect(userFacingError(`${raw} C:\\private\\model.onnx`, false, '兜底')).toBe('兜底')
    expect(userFacingError('model.onnx load failed', false, '兜底')).toBe('兜底')
  })

  test('Pi connection interruptions retain the Host remedy and developer code', () => {
    const failure = { kind: 'failed' as const, code: 'agent_backend_model_connection_interrupted', message: '模型服务连接中断，请检查网络或服务状态后重试。' }
    expect(runFailureUserMessage(failure)).toBe(failure.message)
    expect(runFailureDetailText(failure)).toBe(`${failure.code}: ${failure.message}`)
  })

  test('the thread error card passes the mapped message in normal mode', () => {
    const part = readFileSync(join(import.meta.dir, '..', 'src/features/chat/thread/parts/fox-error.tsx'), 'utf8')
    expect(part).toContain('detail={developerUi ? failure.message : runFailureUserMessage(failure)}')
  })
})
