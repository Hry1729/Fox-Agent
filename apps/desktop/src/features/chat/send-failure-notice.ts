// 发送没开始时只进通知中心：标题和正文是一句人话，错误码与原始信息只在开发者模式的详情里。
// 文案句子复用 agent-backend-failure 的映射，这里只决定标题、入口和要不要带上错误码。
import type { AgentBackendId } from '@/features/conversations/model/agent-backend'
import { agentBackendFailureHint } from '@/features/conversations/model/agent-backend-failure'
import { notify } from '@/features/notifications'

const MODEL_CONNECTION_CODES = new Set(['model.connection_required', 'model.connection_failed', 'model.connection_choose_address'])
const LOGIN_CODES = new Set([
  'agent_backend_not_installed',
  'agent_backend_missing_credentials',
  'agent_backend_exited_on_start',
])

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
  let title = '发送未开始'
  let workspaceView: string | undefined
  let entityId: string | undefined
  if (normalized === 'model.connection_unverified') {
    title = '模型连接需要重新测试'
    workspaceView = 'model-service'
  } else if (MODEL_CONNECTION_CODES.has(normalized)) {
    title = '还没有可用的模型连接'
    workspaceView = 'model-service'
  } else if (LOGIN_CODES.has(normalized)) {
    title = `${agentBackendShortName(options.backend)} 还没有登录`
    if (options.conversationId) {
      workspaceView = 'chat'
      entityId = options.conversationId
    } else {
      workspaceView = 'settings-ai'
    }
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
    ? [sentence, [normalized !== 'unknown' ? normalized : '', raw].filter(Boolean).join('：')].filter(Boolean).join('\n')
    : sentence
  return {
    title,
    description: detail,
    mergeKey: `send-not-started:${normalized}`,
    workspaceView,
    entityId,
  }
}

export function notifySendNotStarted(
  code: string | null | undefined,
  message: string | null | undefined,
  options: SendNotStartedOptions = {},
): string {
  const notice = sendNotStartedNotice(code, message, options)
  return notify.error(notice.title, {
    description: notice.description,
    mergeKey: notice.mergeKey,
    workspaceView: notice.workspaceView,
    entityId: notice.entityId,
  })
}
