// 发送没开始：马上弹一条看得见的 toast（带「去设置」类按钮），同时记一条到通知中心。
// 标题和正文是一句人话，错误码与原始信息只在开发者模式的详情里。
// 文案句子复用 agent-backend-failure 的映射，这里只决定标题、入口和要不要带上错误码。
import type { AgentBackendId } from '@/features/conversations/model/agent-backend'
import { agentBackendFailureHint } from '@/features/conversations/model/agent-backend-failure'
import { notify } from '@/features/notifications'
import { readDeveloperUi } from '@/features/chat/developer-ui'

const MODEL_CONNECTION_CODES = new Set(['model.connection_required', 'model.connection_failed', 'model.connection_choose_address', 'model.connection_unverified', 'model.connection_unreachable'])
// An external Agent that is not set up yet (UI 1103: no login card above the
// composer any more; one notice with「去设置」 to 设置 → Agent).
const NOT_SET_UP_CODES = new Set([
  'agent_backend_missing_credentials',
  'agent_backend_exited_on_start',
  'agent_backend_pi_source_choice_required',
  'model.connection_required',
  'model.connection_choose_address',
])

/** 13.37: Fox's refusal without a model service, as the Host words it (`FOX_MODEL_SERVICE_MISSING`). */
export const FOX_MODEL_SERVICE_MISSING_TITLE = '还没有配置模型服务'
export const FOX_MODEL_SERVICE_MISSING_BODY = '请先到「设置 → 模型服务」添加并测试一个供应商，再发送。'
/** 13.38 B4-5：模型服务地址连不上（拒绝连接、找不到地址、超时），发送前就拒绝，不建 Run。 */
export const MODEL_UNREACHABLE_TITLE = '连不上模型服务'
export const MODEL_UNREACHABLE_BODY = '模型服务的地址现在连不上：请到「设置 → 模型服务」检查地址，或确认服务已经启动，再发送。草稿已保留。'
export interface SendNotStartedNotice {
  title: string
  description: string
  mergeKey: string
  workspaceView?: string
  entityId?: string
}

export interface SendNotStartedOptions {
  developerUi?: boolean
  conversationId?: string | null
  backend?: AgentBackendId | string | null
}

/** 把 composer 读到的 `code: message` 拆开。拆不开就当成未知码。 */
export function parseSendFailureReason(reason: string | null | undefined): { code: string; message: string } {
  const text = reason?.trim() ?? ''
  if (!text) return { code: 'unknown', message: '' }
  const separator = text.indexOf(': ')
  if (separator > 0) {
    const code = text.slice(0, separator).trim()
    if (/^[A-Za-z][A-Za-z0-9_.]*$/.test(code) && (code.includes('.') || code.includes('_'))) {
      return { code, message: text.slice(separator + 2).trim() }
    }
  }
  return { code: 'unknown', message: text }
}

export function agentBackendShortName(backend: AgentBackendId | string | null | undefined): string {
  if (backend === 'deepseek') return 'dsh'
  if (backend === 'pi') return 'pi'
  if (backend === 'codex') return 'Codex'
  if (backend === 'fox') return 'Fox'
  return '这个 Agent'
}

export function sendNotStartedNotice(
  code: string | null | undefined,
  message: string | null | undefined,
  options: SendNotStartedOptions = {},
): SendNotStartedNotice {
  const normalized = code?.trim() || 'unknown'
  const raw = message?.trim() ?? ''
  const hint = agentBackendFailureHint(normalized)
  const sentence = hint?.detail || '消息没有发出去，草稿已保留。'
  const external = Boolean(options.backend && options.backend !== 'fox')
  const name = agentBackendShortName(options.backend)
  let title = '发送未开始'
  let description = sentence
  let workspaceView: string | undefined
  let entityId: string | undefined
  if (normalized === 'agent_backend_user_disabled') {
    title = `${name} 已停用`
    description = '该 Agent 已停用，可在设置中启用。'
    workspaceView = 'settings-ai'
  } else if (external && normalized === 'agent_backend_not_installed') {
    title = `${name} 还没安装`
    description = `这台电脑上还没有装好 ${name}。装好后到设置里检查它的状态。`
    workspaceView = 'settings-ai'
  } else if (external && normalized === 'agent_backend_pi_source_choice_required') {
    title = 'Pi 的配置来源需要确认'
    description = '到设置里的 Pi 选择用 Fox 模型服务还是本机 ~/.pi，再发送。'
    workspaceView = 'settings-ai'
  } else if (normalized === 'model.connection_unreachable') {
    // 13.38 B4-5: the address cannot be reached at all, for Fox and every
    // external Agent alike. The address itself only shows in developer mode.
    title = MODEL_UNREACHABLE_TITLE
    description = MODEL_UNREACHABLE_BODY
    workspaceView = 'model-service'
  } else if (normalized === 'model.connection_unverified') {
    // 13.37 B3-1: an edited provider (e.g. a new Anthropic address) needs a
    // new connection test. This is the model service's step, for Fox and for
    // every external Agent alike, so it is never「<Agent> 还没配置好」.
    title = '模型连接需要重新测试'
    workspaceView = 'model-service'
  } else if (external && normalized === 'model.connection_choose_address') {
    title = '模型服务有多个地址'
    description = userFacingError(raw.replace(/^[a-z_.]+:\s*/, ''), false, sentence)
    workspaceView = 'model-service'
  } else if (external && NOT_SET_UP_CODES.has(normalized)) {
    title = `${name} 还没配置好`
    description = `先在电脑上装好并配置 ${name}，或在模型服务里配好供应商，再到设置里检查。`
    workspaceView = 'settings-ai'
  } else if (normalized === 'model.connection_required') {
    // Fox without a model service (P0-1): the Host's sentence.
    title = FOX_MODEL_SERVICE_MISSING_TITLE
    description = FOX_MODEL_SERVICE_MISSING_BODY
    workspaceView = 'model-service'
  } else if (MODEL_CONNECTION_CODES.has(normalized)) {
    title = '还没有可用的模型连接'
    workspaceView = 'model-service'
  } else if (normalized === 'agent_backend_not_installed' || normalized === 'agent_backend_missing_credentials' || normalized === 'agent_backend_exited_on_start') {
    title = `${name} 还没配置好`
    workspaceView = 'settings-ai'
  } else if (normalized === 'run.conversation_busy') {
    title = '上一条还在处理'
  } else if (normalized === 'conversation.not_found') {
    title = '这个对话已不存在'
  } else if (normalized === 'agent_backend_project_required') {
    title = '项目文件夹不可访问'
    workspaceView = 'settings-projects'
  } else if (normalized === 'agent_backend_attachments_unsupported') {
    title = '这个 Agent 不能带附件'
  } else if (normalized === 'agent_backend_turn_busy') {
    title = '上一轮还没结束'
  } else if (normalized === 'run.create_failed') {
    title = '发送未开始'
  } else if (hint) {
    title = hint.label
  }
  const detail = options.developerUi
    ? [description, [normalized !== 'unknown' ? normalized : '', raw].filter(Boolean).join('：')].filter(Boolean).join('\n')
    : description
  return {
    title,
    description: detail,
    mergeKey: `send-not-started:${normalized}`,
    workspaceView,
    entityId,
  }
}

/** 错误码、环境变量、绝对路径、网关名。普通模式碰到这些就改用人话。 */
const INTERNAL_SURFACE = /(?:\b[a-z][a-z0-9]*(?:[._][a-z0-9_]+){1,}\b)|(?:\b[A-Z][A-Z0-9]+_[A-Z0-9_]+\b)|(?:[A-Za-z]:\\)|(?:\/(?:home|Users|usr|var|tmp)\/)|(?:\.(?:dsh|pi)\/)|(?:\bMagpie\b)|(?:api\.deepseek)|(?:localhost:\d)|(?:127\.0\.0\.1:\d)|(?:\bECONN[A-Z]+\b)/

/**
 * 13.38（R1-3 / B4-2）：没有一个汉字、却有成句英文（三个以上英文单词）的文字是内部原文
 * （例如「Pi released its Run before a terminal result was recorded.」），普通模式不直接显示。
 */
const CJK = /[\u3400-\u9fff\uf900-\ufaff]/
const ENGLISH_WORDS = /[A-Za-z]{2,}(?:[^A-Za-z\u3400-\u9fff]+[A-Za-z]{2,}){2,}/
export function isInternalRawText(value: string): boolean {
  return INTERNAL_SURFACE.test(value) || (!CJK.test(value) && ENGLISH_WORDS.test(value))
}

/** 开发者模式保留原文；普通模式遇到调试信息或英文内部原文时换成 fallback。 */
export function userFacingError(text: string | null | undefined, developerUi: boolean, fallback: string): string {
  const value = text?.trim() ?? ''
  if (!value) return fallback
  if (developerUi) return value
  return isInternalRawText(value) ? fallback : value
}

/** 捕获到的异常转成界面文字：开发者模式原文，普通模式遇到调试信息改用 fallback。 */
export function userFacingCause(cause: unknown, fallback = '这次没有完成，请重试。'): string {
  const text = cause instanceof Error ? cause.message : cause == null ? '' : String(cause)
  return userFacingError(text, readDeveloperUi(), fallback)
}

export function notifySendNotStarted(
  code: string | null | undefined,
  message: string | null | undefined,
  options: SendNotStartedOptions = {},
): string {
  const notice = sendNotStartedNotice(code, message, options)
  // 13.37 (P0-1, B3-2): a refused send always shows a toast right away. The
  // notification-center copy alone only lit the red dot, and a second
  // identical refusal merged into it without any visible reaction.
  return notify.error(notice.title, {
    description: notice.description,
    mergeKey: notice.mergeKey,
    workspaceView: notice.workspaceView,
    entityId: notice.entityId,
    toastNow: true,
  })
}
