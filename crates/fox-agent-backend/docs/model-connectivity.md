# 受管模型连接与 pi 错误结算（运行时 0.1.3）

外部 Agent 仍在 srt 中运行。此修改不关闭沙箱，不扩大所有 localhost 端口，也不修改系统 WFP。

## 地址选择与网络授权

- 私有原生配置优先。pi 从私有 `settings.json` 的 `defaultProvider/defaultModel` 选择 `models.json` 中的模型级或供应商级 `baseUrl`。未选中的供应商不获新增权限。
- Fox 供应商配置只在原生配置缺失、统一登录会生成该配置时作为选择来源。默认供应商与原生 Magpie 不同时，原生文件不会被覆盖。
- 私有 HOME 可由 Agent 写入，所以其中的文件和 `.fox-managed.json` 都不能授予新增网络权限。实际端点必须匹配 Host 当前选择的供应商地址、显式 Host 工程配置，或用户确认导入时的目标记录。
- 导入前从真实 HOME 解析可信快照。用户需选择所有存在的模型地址与选择文件。导入后把快照中的目标写到沙箱外的 `state_root/model-endpoint-grants.json`；不从目标 HOME 重读来发放权限。只导入凭据文件不能为遗留的模型配置补授权。
- 授权绑定所选网络目标。主题、未选中的供应商和凭据轮换不会使同一目标的网络授权失效。目标 host、port 或协议改变后需要新的 Host 授权。
- 端点按 `host:port` 精确放行，包括默认端口。支持 `localhost`、IPv4 和带方括号的 IPv6。动态表达式、userinfo、非法端口、含糊选择和项目中的模型路由覆盖会明确失败。错误不输出完整 URL、请求正文或凭据。
- Host 网络限制仍可收紧授权；已选中的端点被收紧规则排除时，在启动前明确失败。

旧的原生登录若没有 Host 目标记录，会返回 `agent_backend_model_endpoint_unapproved`。修复步骤是确认真实 HOME 中的原生模型和设置，然后在登录卡片重新导入；已有目标文件需要用户明确选择覆盖。原文件会按原有导入规则备份。不会静默切换到另一个供应商。

## 代理与连接检查

pi 使用 srt 已提供的 HTTP/HTTPS 代理。沙箱启动清空回环地址的 `NO_PROXY/no_proxy`；固定的 pi dispatcher 补丁在 Host 要求代理时也清空代理绕过规则。Windows 的实际隔离仍由 srt/WFP 执行，Linux/macOS 的隔离仍由原有沙箱执行。

dsh 0.2.1-rc.2 的原始代理模块强制回环地址直连。受管补丁仅允许 `FOX_MODEL_ENDPOINT` 指定的精确协议、主机和端口使用 srt 代理；其他本机控制端点保留上游直连规则，仍受 OS 沙箱限制。

pi 首次 prompt 在沙箱内对已授权 origin 做一次无凭据 HEAD 检查，不请求模型、不跟随重定向。HTTP 401/404 只证明传输可达，不代表模型或凭据有效。成功结果在该进程中复用；失败结果不永久缓存。每次 prompt 用 pi 的 `getState` 核对实际模型 origin，防止载入历史或模型切换后端点漂移。

凭据探测返回 `modelConnectivity=not_checked`。Host 直连的协议验证缓存也不代表沙箱模型可达。允许列表阻止、传输不可达、凭据拒绝、限流和模型错误分别报告。

Host 在后续 turn 前比较本地配置摘要。配置变化会先结束旧进程，再用新策略重建。pi 在新建或载入原生 session 后，通过稳定 ACP `session/set_config_option` 应用当前所选 provider/model，保留该 session 的历史，避免反复载入旧模型。原进程未确认结束时不会重建。

## 结算

固定 pi-acp 0.0.34 补丁观察模型 error、message_end、turn_end、agent_end 和重试结果，在 `agent_settled` 结算。最终模型失败走合法 JSON-RPC `-32603`；不新增非法 ACP stopReason。

重试后成功正常结束；部分输出后模型失败仍失败；取消保留 cancelled；工具失败属于工具结果，单独的工具失败不会伪装成模型失败。错误只携带固定分类和通用说明，不发送供应商错误正文。

Host 另有空交付兜底。只有启动信息、usage、reasoning 或空白内容的 `end_turn` 不会写成成功。有效回答、资源输出或成功完成的工具交付可正常完成。部分文本仍保留，但失败终态不会改成完成。

## 固定补丁与打包

`scripts/windows/runtime_patches.py` 只处理构建副本。它分别检查 pi-acp 0.0.34、pi 0.84.2 和 dsh-http-proxy 0.2.1-rc.2 的上游 SHA-256，然后应用可审阅的文本补丁并复制小型适配模块。未知版本和未知源字节会被拒绝。

`stage` 自动应用补丁。`pack` 和 Host 启动均根据源码中的 `runtime-assets/model-adapters/pins.json` 检查结果字节，运行时自己的 manifest 不能授予信任。旧 pi 运行时缺少补丁时提示安装 0.1.3；无须改变会话权限模式。已安装目录不会被原地修改。

## 验证边界（2026-10-25）

定向 Rust 测试覆盖端点解析、Host 授权来源、原生优先、配置失效、模型切换后载入历史、错误分类、空交付与清理。Node 测试覆盖补丁后的真实 pi-acp class 与事件序列。Python 测试覆盖包布局、路径预算、版本和源哈希拒绝。

独立 Windows 实测使用已安装 srt 0.0.80、实际 pi 0.84.2、候选 pi-acp 和本机无凭据脚本模型：正常文本和 end_turn、401 authentication、空允许列表 network_allowlist 且服务零请求均通过。显式 NO_PROXY=localhost 仍能通过指定代理。稳定模型选择 ACP 方法也已实测成功。

独立 dsh 代理模块实测：指定模型端口代理 HTTP 200，其他本机端口保持直连并由 WFP 拒绝。该结果不等于完整 dsh 模型、桌面 UI、真实 Magpie 模型或 Linux/macOS 验收。候选包完成组装和路径预算检查；安装验收另列。

本轮独立证据保存在 `D:/python/projects/Fox/sandbox-investigation-20261025/`，含 `result.json`、`pi-result.json`、`pi-result-blocked.json`、`dsh-result.json` 与 `package-pi-result.json`。这些测试没有读取真实模型凭据，也没有改动运行中的 Fox 或 Magpie。
