# 桌面集成验收记录 · SDK 原型（2026-10-27 深夜）

构建：`e9c82ae4`。桌面 `apps/desktop/src-tauri/target/debug/fox-desktop.exe` sha256 `30d03426…76f6`；sidecar `services/agent-runtime/dist/fox-agent-runtime-x86_64-pc-windows-msvc.exe` sha256 `d5cd38c4…e723`。

隔离目录 `%TEMP%\fox-fixture-check\data`（未用 `--reset`，夹具保留）。数据目录、`FOX_KERNEL_MODE=legacy`、WebView2 CDP `9222` 均显式设置；CDP 目标为 `title=Fox, url=http://127.0.0.1:1421/`，即该实例自身的 WebView。

发送路径不再靠猜测：用 CDP 定位唯一可见 `textarea`，经原型 value setter 写入并回读断言，再点编辑器容器内的发送按钮（不解除 disabled、不绕过提交流程）。此前 PostMessage 路线失效的原因即事件被 Chromium 丢弃。

## 通过

| # | 条件 | 证据 |
| --- | --- | --- |
| 1 | 正常回答 | 会话 `4997e46a…`（专家 A），Run `ae2dd49d…` `completed`，model `sensenova-6.8-flash-lite`，答案给出 37,500 元与条款 A-7 |
| 2 | 真实资料工具与引用 | 同轮 `tool_calls`：`list_knowledge_bases` → `search_knowledge` → `read_knowledge_document` 均 `completed`；正文引用 A-7，界面显示"完成 3 次检索"与资料库 id `e7b7b384…` |
| 3 | 重启恢复同一 Pi session | 结束进程重启后追问"把刚才的问题原样复述一遍"：Run `c8d61bc5…` `completed`，`runtime_session_id` 仍为 `c6f6307c…`；映射 `format_version=2`、`status=ready`、`native_session_ref=01a17f0f…`；会话文件只有一条且增长到 91,258 字节；回答逐字复述上一轮问题并给出 37,500/A-7 |
| 4 | 两位专家隔离 | 会话 `4b41d7db…`（专家 B）问城北押金：Run 完成、2 次检索只落在资料库 `07766d8a…`（B 的库），回答明确拒绝并说明该库排除城北项目，**未出现 37,500**；两个会话各自持有独立 native session（B 为 `f89b549b…`） |
| 5 | 损坏会话连续重试两次 | 备份后把 A 的 JSONL 改成 38 字节垃圾：第 1 次 `runtime.start_failed` / `session.native_corrupt`；第 2 次 `session.native_unavailable`（同一行、未新建、未请求运行时）；文件仍为 38 字节未被覆盖；映射行原地转为 `unavailable`，未替换；两次都只显示"运行失败/查看错误详情/重试"，没有出现空上下文。随后已用备份还原文件并把该行恢复为 `ready` |

## 未通过 / 未做

- 第 6 项两项生命周期能力**未实现**：明确的"重置上下文"入口；删除会话时清理原生 JSONL（当前只删旧 `session_path`，原生文件会成孤儿）。
- 界面上的"引用可定位"没有点开跳转验证，只验证了正文引用与工具调用记录。
- `2b139fda` 的输入区静态 `Fox` 标签尚未在桌面复核外观。
- 404 记为当前未复现、历史根因未定；并发 live 测试失败归属未定。

## 复跑

```powershell
node scripts/start-desktop-dev.mjs
$env:FOX_DATA_DIR="$env:TEMP\fox-fixture-check\data"; $env:FOX_KERNEL_MODE="legacy"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
apps/desktop/src-tauri/target/debug/fox-desktop.exe
node <cdp driver> "<会话标题片段>" "<问题>"
```
