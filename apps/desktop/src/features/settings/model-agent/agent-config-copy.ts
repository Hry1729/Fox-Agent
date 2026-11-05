import type { AgentConfigState } from './agent-config-types'
export const STATE_LABELS: Record<AgentConfigState, string> = {
  ready: '已就绪', configured_unverified: '已配置，未验证', native_importable: '发现可导入配置',
  installed_unconfigured: '已安装，未配置', not_installed: '未安装', read_failed: '读取失败',
}
const ERRORS: Record<string, string> = {
  'agent_config.read_failed': '没能读取这个 Agent 的配置，请重试。',
  'agent_backend_not_installed': 'Fox 运行时里没有这个 Agent。',
  'agent_config.system_only': '电脑上装了这个 Agent，但 Fox 需要用自带的运行时启动它。',
  'agent_backend_pi_gate_missing': '这个 Agent 的运行时不完整，Fox 无法启动它。',
  'agent_backend_runtime_adapter_missing': '这个 Agent 的运行时不完整，Fox 无法启动它。',
  'agent_binding.provider_not_found': '选择的服务已被删除或停用，请重新选择。',
  'agent_binding.endpoint_not_found': '这个服务没有该 Agent 能用的入口。',
  'agent_binding.protocol_mismatch': '这个 Agent 不能使用这种协议。',
  'agent_binding.missing_key': '这个服务还没有填写 API Key。',
  'agent_binding.model_required': '请选择一个模型。',
  'agent_binding.native_required': '请先导入电脑上已有的登录配置。',
  'model.connection_required': '请先选择模型服务或导入登录配置。',
  'agent_verify.model_failed': '模型请求失败，请检查地址、模型和 Key。',
  'agent_verify.agent_no_output': 'Agent 没有输出。',
  'agent_verify.tool_failed': '工具调用没有完成。',
  'agent_verify.timeout': '验证超时，请稍后重试。',
  'agent_verify.busy': '正在验证，请稍候。',
  'agent_verify.cancelled': '已停止验证。',
  'agent_verify.config_changed': '配置已变化，请重新验证。',
  'agent_import.nothing_selected': '请至少选择一项。',
  'agent_backend_source_changed': '电脑上的配置刚刚变了，请重新预览。',
  'model.duplicate_base_url': '这个服务地址已经添加过了。',
}
export function codeToText(code: string): string { return ERRORS[code] ?? '操作没有完成，请重试。' }
export function configErrorText(error: unknown): string {
  return codeToText(error && typeof error === 'object' && 'code' in error ? String(error.code) : '')
}
