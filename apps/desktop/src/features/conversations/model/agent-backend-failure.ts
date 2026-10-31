// Plan C4: start and failure codes of an external Agent (pi / Codex / dsh)
// -> one actionable hint (install, log in, upgrade, retry, configure). The
// Host decides and reports the code (`fox_agent_backend::external::failure_codes`
// and the launcher's own codes); this only says what the user can do about
// it. Nothing here retries, falls back to the built-in Fox route, or picks
// another backend.
import type { ConversationDetail } from './types'

export type AgentBackendFailureAction = 'install' | 'login' | 'upgrade' | 'retry' | 'configure' | 'admin'

export interface AgentBackendFailureHint {
  code: string
  action: AgentBackendFailureAction
  /** Button-sized label of what to do. */
  label: string
  /** One sentence: where to do it. */
  detail: string
}

const ACTION_LABELS: Record<AgentBackendFailureAction, string> = {
  install: '安装外部运行时',
  login: '登录',
  upgrade: '升级外部运行时',
  retry: '重新发送',
  configure: '修改 Agent 配置',
  admin: '联系管理员',
}

const HINTS: Record<string, { action: AgentBackendFailureAction; detail: string }> = {
  agent_backend_not_installed: { action: 'install', detail: '这个 Agent 没有安装在 Fox 外部运行时里：运行 Fox 安装包里的「外部运行时」安装（或修复）后重试。' },
  agent_backend_spawn_failed: { action: 'install', detail: 'Agent 进程无法启动：重新安装（修复）Fox 外部运行时后重试。' },
  agent_backend_pi_gate_missing: { action: 'install', detail: 'pi 缺少 Fox 审批拦截扩展：重新安装（修复）Fox 外部运行时后重试。' },
  agent_backend_runtime_adapter_missing: { action: 'install', detail: 'pi 缺少已验证的模型错误适配：重新安装（修复）Fox 外部运行时后重试。' },
  agent_backend_fox_tools_unavailable: { action: 'install', detail: 'Fox 工具桥（交付文件、知识库、提问）无法准备：修复 Fox 外部运行时；仍不行请重启 Fox。' },
  agent_backend_missing_credentials: { action: 'login', detail: '这个项目里还没有这个 Agent 的登录：在输入框上方的登录卡片里完成登录或从真实 HOME 导入。' },
  'model.connection_required': { action: 'login', detail: '还没有可用的模型连接：在后端栏补充或验证当前供应商的模型连接。' },
  agent_backend_exited_on_start: { action: 'login', detail: 'Agent 启动后立即退出，多数是未登录或运行时损坏：先完成登录；仍失败请重新安装外部运行时。' },
  agent_backend_version_mismatch: { action: 'upgrade', detail: '已安装的 Agent 版本与本版 Fox 锁定的不一致：升级（或重新安装）Fox 外部运行时。' },
  agent_backend_protocol_mismatch: { action: 'upgrade', detail: 'Agent 的 ACP 协议版本与本版 Fox 不匹配：升级 Fox 外部运行时。' },
  agent_backend_protocol_error: { action: 'upgrade', detail: 'Agent 拒绝了 Fox 的启动握手：升级 Fox 外部运行时到与本版 Fox 配套的版本。' },
  agent_backend_fox_tools_unsupported: { action: 'upgrade', detail: '已安装的 pi-gate 不支持 Fox 工具：升级 Fox 外部运行时。' },
  agent_backend_start_timeout: { action: 'retry', detail: 'Agent 没有在限定时间内完成启动：重新发送；反复出现时检查安全软件拦截或重新安装外部运行时。' },
  agent_backend_exited: { action: 'retry', detail: 'Agent 进程在这一轮中途退出：重新发送；反复出现时查看诊断里的后端输出。' },
  agent_backend_response_timeout: { action: 'retry', detail: 'Agent 长时间没有进展：检查模型连接后重新发送。' },
  agent_backend_session_failed: { action: 'retry', detail: 'Agent 无法打开会话：重新发送；仍失败请查看诊断里的后端输出。' },
  agent_backend_full_access_revoked: { action: 'retry', detail: '完全访问已撤销，本轮没有执行：重新发送，下一轮按撤销后的权限运行。' },
  agent_backend_stop_unconfirmed: { action: 'retry', detail: '原 Agent 进程未确认结束：重启 Fox 后再发送。' },
  agent_backend_fox_tools_rejected: { action: 'configure', detail: 'Codex 自己的配置里已有同名 MCP 服务器（fox_host_business_tools）：在 Codex 配置里删除或改名后重新发送。' },
  agent_backend_disabled: { action: 'admin', detail: '这个 Agent 已被管理员关闭（FOX_EXTERNAL_BACKENDS）。' },
}

/** The hint for a failure code; null for a code that has no user action. */
export function agentBackendFailureHint(code: string | null | undefined): AgentBackendFailureHint | null {
  if (!code) return null
  const hint = HINTS[code]
  if (!hint) return null
  return { code, action: hint.action, label: ACTION_LABELS[hint.action], detail: hint.detail }
}

/**
 * The last run's failure of an external conversation, when it carries one of
 * the codes above (a Fox conversation, a running or successful last run, or
 * an unknown code: null).
 */
export function lastAgentBackendFailure(detail: ConversationDetail | null | undefined): AgentBackendFailureHint | null {
  const backend = detail?.conversation.agentBackend
  if (!detail || !backend || backend === 'fox') return null
  const run = detail.lastRun
  if (!run || run.status !== 'failed') return null
  return agentBackendFailureHint(run.errorCode)
}
