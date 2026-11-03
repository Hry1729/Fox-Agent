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
  agent_backend_fox_tools_unavailable: { action: 'install', detail: 'Fox 工具桥（交付文件、知识库、提问）无法准备，或 pi 的 Fox 工具扩展没有连上工具桥：重启 Fox 后重试；仍不行请修复 Fox 外部运行时。' },
  agent_backend_missing_credentials: { action: 'login', detail: '这个 Agent 还没有可用的登录：到「设置 → 模型服务」填写并测试当前供应商，Fox 会把它的 Key 交给这个 Agent。' },
  agent_backend_project_required: { action: 'configure', detail: '项目文件夹不可访问：请重新选择一个可以打开的项目后再发送。' },
  agent_backend_attachments_unsupported: { action: 'retry', detail: '这个 Agent 不能带附件发送：去掉附件后再发。' },
  agent_backend_turn_busy: { action: 'retry', detail: '上一轮还没结束：等它完成或先取消，再发送。' },
  'run.create_failed': { action: 'retry', detail: '消息没有发出去，草稿已保留。' },
  // 13.37: the Host's own sentences (FOX_MODEL_SERVICE_MISSING / login_gap_failure), word for word.
  'model.connection_required': { action: 'login', detail: '还没有配置模型服务：请先到「设置 → 模型服务」添加并测试一个供应商，再发送。' },
  'model.connection_failed': { action: 'login', detail: '模型连接没有通过：请到模型服务里检查供应商和地址后再发送。' },
  'model.connection_unreachable': { action: 'login', detail: '模型服务的地址现在连不上：请到「设置 → 模型服务」检查地址，或确认服务已经启动，再发送。草稿已保留。' },
  'model.connection_unverified': { action: 'login', detail: '模型服务的配置改过了，当前地址还没有通过连接测试：请到「设置 → 模型服务」重新测试连接，通过后再发送。' },
  'model.connection_choose_address': { action: 'login', detail: '当前供应商有多个地址都能用于这个 Agent：到「设置 → 模型服务」，在要用的地址上打开「用作默认协议」后再发送。' },
  agent_backend_exited_on_start: { action: 'login', detail: 'Agent 启动后立即退出，多数是未登录或运行时损坏：先完成登录；仍失败请重新安装外部运行时。' },
  agent_backend_version_mismatch: { action: 'upgrade', detail: '已安装的 Agent 版本与本版 Fox 锁定的不一致：升级（或重新安装）Fox 外部运行时。' },
  agent_backend_protocol_mismatch: { action: 'upgrade', detail: 'Agent 的 ACP 协议版本与本版 Fox 不匹配：升级 Fox 外部运行时。' },
  agent_backend_protocol_error: { action: 'upgrade', detail: 'Agent 拒绝了 Fox 的启动握手：升级 Fox 外部运行时到与本版 Fox 配套的版本。' },
  agent_backend_fox_tools_unsupported: { action: 'upgrade', detail: '已安装的 pi-gate 不支持 Fox 工具：升级 Fox 外部运行时。' },
  agent_backend_start_timeout: { action: 'retry', detail: 'Agent 没有在限定时间内完成启动：重新发送；反复出现时检查安全软件拦截或重新安装外部运行时。' },
  agent_backend_exited: { action: 'retry', detail: 'Agent 进程在这一轮中途退出：重新发送；反复出现时查看诊断里的后端输出。' },
  agent_backend_workspace_acl_denied: { action: 'retry', detail: 'dsh 没能给项目目录加上写入授权（Windows 拒绝访问）：多半是项目目录或其中的文件属于管理员等其他账户。请在该目录的「属性 → 安全 → 高级」里把所有者改为当前用户（含子项），或给当前用户完全控制，然后重新发送。' },
  agent_backend_response_timeout: { action: 'retry', detail: 'Agent 长时间没有进展：检查模型连接后重新发送。' },
  agent_backend_session_timeout: { action: 'retry', detail: 'Agent 长时间没有打开会话，本次启动已停止：重新发送；仍失败请查看诊断里的后端输出。' },
  agent_backend_session_failed: { action: 'retry', detail: 'Agent 无法打开会话：重新发送；仍失败请查看诊断里的后端输出。' },
  agent_backend_full_access_revoked: { action: 'retry', detail: '完全访问已撤销，本轮没有执行：重新发送，下一轮按撤销后的权限运行。' },
  agent_backend_stop_unconfirmed: { action: 'retry', detail: '原 Agent 进程未确认结束：重启 Fox 后再发送。' },
  agent_backend_fox_tools_rejected: { action: 'configure', detail: 'Codex 自己的配置里已有同名 MCP 服务器（fox_host_business_tools）：在 Codex 配置里删除或改名后重新发送。' },
  agent_backend_pi_source_choice_required: { action: 'configure', detail: 'Pi 的配置来源需要确认：到「设置 → Agent」的 Pi 里选择用 Fox 模型服务还是本机 ~/.pi。' },
  'run.conversation_busy': { action: 'retry', detail: '这个对话还在处理上一条消息：等它结束或先停止，再发送。' },
  'conversation.not_found': { action: 'retry', detail: '这个对话已不存在或已归档：新建一个对话再发送。' },
  agent_backend_user_disabled: { action: 'configure', detail: '该 Agent 已停用，可在设置中启用。' },
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
