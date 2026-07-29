# Fox 第三阶段实施状态

> 2026-07-28 的稳定性、文档能力和竞品差距复审见 [FOX_PRODUCT_AUDIT_2026-07-27.md](./FOX_PRODUCT_AUDIT_2026-07-27.md)。

更新日期：2026-07-28

## 1. 阶段结论

第三阶段的计划内核心能力已经完成实现与自动验证。Fox 目前具备可版本化 Runtime 契约、知识图谱、Skills、MCP、多模态图片输入、Sidecar 单次恢复、数据备份恢复、诊断脱敏、大历史分页与全文搜索能力。

本阶段保持了第二阶段的稳定边界：SQLite 仍是历史事实来源，Yuxi 是可选远程能力，Runtime、Skill 与 MCP 都不能绕过 Fox 权限和审计层。

## 2. 已完成能力

### 知识库原文件下载与预览基础

- 知识库详情页已经提供 `下载原文件`，通过系统另存为对话框选择目标位置。
- Rust 从知识库服务读取原始响应并分块写入随机 `.part` 文件，完成后校验响应长度并使用 Windows 原子替换落盘。
- 文件名会清理路径分隔符、控制字符和 Windows 非法字符；取消或失败不会留下最终损坏文件。
- 下载使用独立无总读取超时的 HTTP Client，仍保留连接超时，避免大文件被普通 API 的 10 秒超时中断。
- 下载已支持进度事件与用户取消；进度按时间或字节阈值节流，主动取消按正常结束处理，不在界面显示成错误。
- SQLite 迁移 11 已建立知识库预览缓存元数据表。Fox 启动时重置上次异常退出遗留的租约；启动与日常维护都会清理超过 24 小时的 `.part`，并按 500 MB 默认上限淘汰未租用的 LRU 条目。
- 缓存租约恢复只在应用启动时执行，普通清理不会重置活动租约；缓存仓储已具备写入、触摸、加租约和解租约接口，供后续预览器接入。
- Fox Rust 已增加鉴权 HEAD 元数据命令和最多 4 MB 的 Range 命令，严格校验 `source_revision`、`206`、`Content-Range`、返回长度与 `If-Range`，拒绝忽略 Range 而返回 `200` 的服务端。
- 知识库详情页已接入统一预览状态：`loading`、`ready`、`failed` 和 `unsupported`，失败可重试，服务端能力未就绪时可明确降级到解析内容。
- 图片预览通过 Fox 本地缓存按 4 MB 分段读取，缓存未命中时由 Rust 顺序获取原件 Range；单图上限 64 MB，切换文档时旧请求不能覆盖新文档，Blob URL 会在切换或卸载时释放。
- 文本、代码、JSON、Markdown、CSV 和 TSV 已有基础预览。文本类单次最多读取原件前 2 MB；CSV/TSV 支持带引号的分隔符和换行，并限制为最多 500 行、100 列，避免异常文件拖垮页面。
- Parsed 内容现在与 `documentId` 绑定提交，快速切换资源时不会把上一文件的正文渲染到当前文件。
- 当前知识库服务已经完成 Fox 所需的原件元数据、版本标识和严格 Range 网关改造，Fox 已使用真实 MinIO 文档完成 PDF、DOCX、电子表格和 PPTX 联调。
- PDF 已接入 PDF.js 单页查看器，原件通过 Fox 受控缓存读取，支持分页、缩放、文档搜索和渲染取消；重型 PDF 查看器与 Worker 均独立分包，不进入首屏主包。

### Runtime 与能力策略

- Capability Manifest v1 和统一 Tool Catalog 已固定。
- Rust 校验协议名、JSONL 版本、Runtime 名称、Manifest 版本和工具目录。
- Fake Runtime 与 Pi Runtime 共享黑盒契约测试。
- 前端读取能力声明：非视觉 Runtime 不显示图片入口；不支持动态模型切换时只显示当前模型，不提供切换控件。
- OpenAI-compatible 与 Anthropic Messages 均支持 Fox 原生图片内容块。
- 图片限制：PNG/JPEG/WebP/GIF，单张 10 MB，单条 4 张，总像素 2500 万。

### Sidecar 稳定性与诊断

- 活动 Run 遇到崩溃会标记失败，不重放用户请求或工具。
- 有稳定 Session 时最多自动重启并恢复一次。
- 协议、JSONL 或 Manifest 契约错误直接熔断，不循环重启。
- 诊断包含 Runtime/协议/能力、Session、Run、恢复次数和脱敏错误。
- Windows Sidecar 和 MCP 子进程均静默启动。

### 知识图谱

- 支持节点搜索、标签筛选、深度和节点数量限制。
- 支持真实关系边、邻居展开、节点详情和来源文档跳转。
- Fox Agent 可调用 `query_knowledge_graph`。
- 图谱查询受当前会话绑定知识库授权约束。

### Skills

- 扫描 `<Fox data>/skills/*/SKILL.md`。
- 校验 YAML 元数据与所需工具。
- 支持按 Agent 启用或禁用，并注入 Runtime 指令。
- Skill 只作为指令包，不能携带任意执行权限，也不能扩权。

### MCP

- 支持 stdio JSON-RPC 的 `initialize`、`tools/list`、`tools/call`。
- 每次请求独立进程，15 秒超时，最多 200 个工具。
- 配置与凭证分离，环境值保存在 Windows Credential Manager。
- MCP 工具调用经过 Fox 审批、schema 校验和工具审计。
- 设置页支持新增、编辑、删除、测试、启停与工具目录预览。

### 数据治理与大历史

- 初次仅加载最近 120 条消息，可继续加载更早消息及关联事件、工具、附件和产物。
- SQLite FTS5 同时搜索会话标题、项目、Agent 和消息正文。
- 超过 128 KB 的工具结果保存摘要。
- 诊断 JSON 不包含凭证、对话正文或项目文件内容。
- 自定义 `.foxbackup` 包含一致性 SQLite 快照、附件、Runtime Session 和 Skills，不包含凭证。
- 每个条目带 SHA-256；拒绝路径穿越、重复路径、损坏/截断包、单条超过 512 MB 和总量超过 4 GB。
- 恢复在下次启动前应用，支持路径重定位、数据库完整性检查和失败回滚。
- Runtime Session 是尽力恢复，不保证跨 Pi 版本兼容。
- 清理支持孤立附件、失败附件记录和过期 Runtime Session。
- 删除会话同步清理 Fox 管理目录中的附件、未导出产物、Runtime Session 和可用的 Yuxi 远程线程；项目目录中的已导出文件不删除。

## 3. 实现决策

- MCP 第三阶段仅支持 stdio，不开放网络型 Server。
- MCP 每个请求使用独立子进程，优先隔离和失败收敛。
- Sidecar 每次崩溃链路只允许一次自动恢复。
- 备份恢复采用重启前排队恢复与 rollback 目录。
- 会话固定绑定 Agent；切换 Agent 创建新会话。
- Fox Runtime 不支持会话中动态切换模型，Yuxi 仅在其明确允许时开放模型覆盖。

## 4. 自动验证

- Rust：`117/117` 通过；其中知识库预览缓存测试覆盖统计、租约、LRU、路径边界、空闲缓存清理、活动缓存保护、孤儿文件回收、上限持久化、异常配置钳制、调小上限后的即时淘汰、预览操作生命周期、带原扩展名的安全缓存路径、下载文件注册表，以及下载后直接打开的文件类型安全策略。
- 最新前端 Bun：`33/33` 通过，覆盖知识库格式路由、完整缓存和 Range 数据源、分块读取取消、Office ZIP 安全边界、PDF 页码与缩放、表格约束、运行事件恢复和引用定位归一化。
- Agent Runtime：`44/44` 通过。
- 前端 TypeScript 与 Vite 生产构建通过。
- Runtime Sidecar 构建成功。
- Tauri release 主程序构建成功。
- NSIS 安装包构建成功。
- `cargo fmt --check` 与 `git diff --check` 通过。
- 备份端到端测试验证会话、消息、附件、Skills、Runtime Session、路径重定位及凭证排除。
- MCP 假服务测试覆盖握手、列举、调用、非法 schema、超时和未知工具。

## 5. 构建产物

- 主程序：`apps/desktop/src-tauri/target/release/fox-desktop.exe`
- NSIS：`apps/desktop/src-tauri/target/release/bundle/nsis/Fox_0.1.0_x64-setup.exe`
- 最新 NSIS 构建时间：2026-07-27 11:52；大小 88.32 MB；SHA-256 `BBE70F43DFE0885F5660F36E4D50F45966091009C8243A0F1BB2B6EE0E9CAAC9`。
- 最新主程序大小 74.82 MB；SHA-256 `98C1B341E9BEE42D523F19C42DBC1705C91CC5751D08759AB6B7E01B3CCE69B3`。

完整双格式构建中 WiX `light.exe` 在本机生成 MSI 时失败，release 主程序和 NSIS 均不受影响。目录内已有旧 MSI，不应将其视为本次构建产物。

## 6. 已知非阻塞项

- 知识库当前已有文件树、解析内容、引用跳转、原文件下载、本机打开、预览缓存和真实原件网关联调闭环。
- PDF.js、DOCX 和 XLSX/XLS/ODS 本地预览已经完成。Office 派生 PDF、Preview Job 与 LibreOffice 转换链路已于 2026-07-26 退役，后续只维护原件网关和浏览器端成熟预览库。
- Vite 仍提示部分 chunk 大于 500 KB。当前知识库页面与各文件查看器已经动态导入；后续性能专项将继续拆分主工作台和设置路由、限制 Shiki 语言及主题集合，并隔离 Mermaid、图谱布局、PPTX 和 WASM 依赖。本项不阻塞当前安装包验收。
- 大 Chunk 的分阶段拆分方案、验收指标，以及预置头像目录和自定义头像后续项已统一登记在 `FOX_PERFORMANCE_AND_PRODUCT_BACKLOG.md`。
- Rust 仍有 2 条既有 dead-code 警告，不影响运行与构建。
- Pi Session 跨版本恢复仅为 best effort。
- 远程 Fox Runtime、本地模型、跨设备同步、长任务和子 Agent 仍需按真实需求单独立项。
- 本地阅读位置、书签和阅读笔记已由 SQLite migration 13 实现；它们不再属于缺失项。未完成的是文本选择高亮、多人评论和跨设备同步。
- 知识库工具返回值已在 Runtime 宿主边界限额，避免长解析文档重复进入 `content/details` 后造成过大的工具消息。
- 知识库列表搜索已接入真实筛选，遗留的“打开管理后台”入口已删除。

## 7. 2026-07-23 知识库预览缓存进展

- Fox 图片原件预览已经接入 500 MB 本地 LRU 缓存，不再只依赖每次打开时重复读取全部远程 Range。
- 缓存键只使用知识库 ID、文档 ID、知识库服务返回的 `source_revision` 和 Fox 预览规格；Fox 不自行推导源版本。
- 同一缓存键使用进程内异步互斥，避免并发预览重复下载或覆盖同一文件。
- 缓存写入使用随机 `.part`、完整长度校验、flush/sync 和原子替换；`.part` 不会被当作有效缓存。
- WebView 只持有不透明缓存键。缓存读取要求活跃租约，并再次校验文件位于 Fox 管理目录内，不暴露本地路径或任意文件读取能力。
- SQLite UPSERT 现会保留已有租约计数，避免并发写入把活跃租约重置为零。
- 图片完成 Blob 装载后立即释放租约；未来 PDF.js 将复用同一缓存分段读取命令，并在查看器生命周期内持有租约。
- Yuxi 后端仍未修改。所需 HEAD、Range、`source_revision`、权限和真实 MinIO 流式契约见 [YUXI_KNOWLEDGE_ORIGINAL_FILE_GATEWAY_REQUIREMENTS.md](./YUXI_KNOWLEDGE_ORIGINAL_FILE_GATEWAY_REQUIREMENTS.md)。

## 8. 2026-07-23 预览器路由与数据源进展

- 预览类型已从笼统的 `office` 拆分为 `docx`、`spreadsheet`、`presentation` 和 `legacy-office`，避免不同格式共用错误的占位逻辑。
- 新增统一 `KnowledgePreviewSource`：负责元数据、缓存租约、严格范围读取、完整分段读取和幂等释放，PDF.js、DOCX 与电子表格查看器将共享该数据源。
- 图片查看器已迁移到统一数据源，不再自行复制缓存获取、分段读取和租约释放逻辑。
- PPTX/ODP 在存在 parsed 内容时会明确展示“内容预览”，并标注不代表原始幻灯片版式；没有解析内容时才显示不支持状态。
- 旧版 DOC/PPT 二进制格式明确要求下载原文件，不会误交给 DOCX 或现代演示文稿渲染器。
- `pdfjs-dist`、`docx-preview`、`@js-preview/excel` 和 `@aiden0z/pptx-renderer` 已安装并按查看器动态加载；重型查看器不进入首屏主包。
- Excel 已从自绘 SheetJS 表格迁移到成熟的 `@js-preview/excel` 1.7.14（MIT）。原文件仍经过缓存读取、64 MB 上限与 ZIP bomb 检查，右键菜单和编辑器被禁用，卸载时调用 `destroy()`。
- PPTX 原候选 `pptx-react-viewer` 2.0.0-2.5.0 均错误发布了 `pptx-viewer-mcp@workspace:*`，已排除。现已接入 `@aiden0z/pptx-renderer` 1.2.4（Apache-2.0）：只挂载当前幻灯片，提供上一页/下一页，启用惰性幻灯片和媒体解析；失败时保留解析文本分页提纲作为回退。

## 9. 2026-07-24 本地预览缓存管理闭环

- 设置页“数据与诊断”已增加预览缓存统计，展示总占用、可清理空间、活动文件数量和 500 MB 当前上限。
- 缓存上限已通过 SQLite 迁移 12 持久化，可选择 250 MB、500 MB、1 GB、2 GB 或 5 GB；调小后立即运行 LRU 收敛，重启后仍保留选择。
- 缓存租约释放后会立即重新检查空间上限，避免刚关闭的大文件一直等待到下次启动或手动维护才被淘汰。
- 用户可刷新统计并清理未使用缓存；清理前有确认提示，完成后会重新读取实际空间数据。
- 手动清理只处理 `lease_count = 0` 的条目，正在预览的活动文件及数据库记录会保留。
- 启动和日常维护会识别数据库之外的孤儿 `.cache` 文件；缓存文件删除失败时留下的孤儿可在后续维护中恢复清理。
- 清理只接受 Fox 管理目录内的 `.cache` 文件，拒绝路径逃逸和 `.part` 文件，不会触碰用户通过“下载原文件”保存到其他位置的文件。
- 本轮没有修改 Yuxi 后端。远程原件联调仍需要知识库服务实现鉴权 HEAD/Range、稳定 `source_revision`、严格 `206`/`Content-Range` 和真实 MinIO 流式读取。

## 10. 2026-07-24 预览数据源策略拆分

- `KnowledgePreviewSource` 已明确区分 `range` 与 `cache` 两种策略，避免所有格式共用“打开即完整下载”的行为。
- PDF 原自定义 `PDFDataRangeTransport` 在部分文档翻到第二页后可能无法完整交付后续资源，已改为先进入 Fox 受控缓存，再把完整原件交给 PDF.js；界面仍只挂载和渲染当前页 Canvas。
- 图片、DOCX 和电子表格使用完整缓存数据源，因为图片 Blob、DOCX ZIP 与工作簿解析需要完整原件；缓存仍受大小、路径和租约保护。
- Range 数据源校验服务端 Range 能力、同一 `source_revision`、单次 4 MB 上限和 JavaScript 安全整数范围。
- Range source 的核心已与 Tauri 桥接解耦并具备纯单元测试；接口在类型层不提供 `readAll()`，避免未来查看器误把大 PDF 一次性装入内存。
- Range source 仍保留给严格按段读取场景；PDF.js 不再使用 WebView 自定义 Range 传输，避免异步字节段交付造成后续页面空白。搜索任务继续使用代次控制，切换搜索、文件或页面卸载后不会提交过期结果。

## 11. 2026-07-24 完整缓存预览取消闭环

- 图片、DOCX 和电子表格的完整缓存获取增加独立操作生命周期：前端先注册操作，再发起缓存获取，最后幂等结束操作。
- 文件切换或查看器卸载会通过 `AbortSignal` 请求取消；Rust 在联网前、等待同一缓存键锁后、每个 4 MB Range 分块前后检查取消状态。
- 取消后不会建立缓存租约，随机 `.part` 文件会被删除，并返回不可重试的 `knowledge.preview_cancelled`，避免后台继续下载旧文件。
- 注册、取消和结束使用强引用令牌，解决“取消先于缓存命令启动”时信号丢失的问题；缓存命令使用 Drop guard 覆盖成功、失败和提前返回的清理路径。
- 前端只有在注册成功后才执行兜底结束，避免极低概率 UUID 冲突时误删另一个操作的令牌。
- 本轮仍未修改 Yuxi 后端。Fox 取消的是自身后续分块请求；已经发出的单次 HTTP Range 请求需要等待响应或网络超时后才能结束。

## 12. 2026-07-24 下载完成操作与本地读取取消

- 下载成功提示现在提供“打开文件”和“打开所在文件夹”，无需用户手动寻找刚保存的文件。
- Fox 只允许操作当前进程内成功下载并注册的文件；路径会再次规范化并校验为仍然存在的普通文件，不能借该命令打开任意本地路径。
- “打开文件”使用保守白名单，仅允许常见文档、文本和图片格式；可执行文件、安装包、脚本、源码、压缩包、无扩展名和未知格式只能在资源管理器中定位，避免下载后直接触发危险内容。
- 完整缓存数据源按 4 MB 分块读取，并在每块读取前后响应 `AbortSignal`；切换文档时不仅会停止远程缓存获取，也会停止尚未完成的本地缓存读取。
- PDF 页码、缩放与通用表格虚拟窗口约束已经提取为纯交互模型并完成单元测试；工作簿内部渲染交由成熟预览库维护。
- 本轮没有修改 Yuxi 后端。真实远程原件联调仍需要知识库服务提供鉴权 HEAD、严格 Range、稳定 `source_revision` 和 MinIO 对象流式响应。

## 13. 2026-07-24 PDF 与 Office 快速预览完成

- PDF.js 已接入 Fox 受控缓存，支持真正的单页挂载、分页、缩放、全文搜索、搜索结果跳页、高清 Canvas 渲染和渲染任务取消；搜索切换使用代次控制，旧搜索不会覆盖新结果。
- DOCX 使用 `docx-preview` 渲染正文、图片、表格、页眉页脚、脚注和尾注；渲染完成后只显示当前 `section.docx`，支持按钮和边界滚轮翻页；评论、修订和 altChunk 默认关闭，预览内链接不会直接导航。
- XLSX、XLS 和 ODS 使用 `@js-preview/excel` 本地只读渲染，支持多工作表、格式化值、公式缓存结果、合并单元格及固定行列标题；库代码与样式均在打开电子表格时懒加载。
- Office 快速预览输入上限为 64 MB。ZIP Office 文档会在进入第三方解析器前检查中央目录、加密标志、条目数、单条目展开大小、总展开大小、压缩比、分卷和 ZIP64，异常文件不会继续在 WebView 中展开。
- Excel 查看器关闭右键菜单和编辑器，传入第三方库前复制独立 `ArrayBuffer`，切换文档或卸载时调用 `destroy()` 并释放缓存租约。
- PDF.js、DOCX、Excel、PPTX 查看器和 PDF Worker 均按文件类型动态加载；生产构建通过，现有主包体积告警仍属于项目原有的全局拆包任务。
- 当时的自动验证：前端 Bun `34/34`、Rust `116/116`、TypeScript/Vite 生产构建、`cargo fmt --check` 和 `git diff --check` 通过；后续迁移的验证结果记录在对应更新条目中。
- 本轮没有修改 Yuxi 后端。Fox 端 A-C 阶段能力已就绪；真实远程联调仍等待知识库服务实现 [原文件网关契约](./YUXI_KNOWLEDGE_ORIGINAL_FILE_GATEWAY_REQUIREMENTS.md)。LibreOffice 派生 PDF 属于后端阶段 D，不在本轮 Fox-only 实现范围内。

## 14. 2026-07-24 桌面打开与精确引用完成

- 知识库文档工具栏增加“本机打开”。用户主动触发后，Fox 先通过现有鉴权 Range 链路确保完整原件进入受控缓存，再调用系统默认应用；前端不接收、不传递任意本地文件路径。
- 新写入的缓存文件在保留 `.cache` 安全标识的同时保留经过约束的原扩展名，使 Windows 能正确选择 DOCX、XLSX、PDF、图片等默认应用。旧版无扩展名缓存会在空闲时自然失效并按新格式重建。
- 本机打开沿用文档与图片白名单；可执行文件、安装包、脚本、宏文档、压缩包和未知格式仍被拒绝直接启动，只能下载后由用户自行处理。
- 外部应用打开后，同一缓存键在当前 Fox 进程内只保留一份保护租约，避免 LRU 在应用读取期间删除文件，也避免重复点击无限累积租约；下一次 Fox 启动会按既有恢复流程重置运行态租约。
- 引用来源内部保留 `chunk_id/page/anchor/excerpt`：PDF 优先跳转到指定页，没有页码时搜索引用文本；文本预览自动搜索并滚动到匹配；DOCX 有真实分页时按页码打开，并继续定位、高亮引用段落。对于被 `docx-preview` 渲染为单个连续页面的文档，Fox 会按引用正文滚动到对应位置。界面不向普通用户展示内部 chunk ID。
- 普通页面导航、返回、新建会话和切换 Agent 会清理引用定位状态，避免旧引用错误影响后续打开的文档。
- 自动验证：前端 Bun `35/35`、Agent Runtime `44/44`、Rust `117/117`、TypeScript/Vite 生产构建、`cargo fmt --check` 与 `git diff --check` 通过。
- 本轮没有修改 Yuxi 后端。阶段 D 的版式 PDF 仍需要知识库服务实现 Preview Job、派生内容 Range 接口和一次性隔离 LibreOffice Worker。

## 15. 2026-07-24 Fox 版式预览客户端（已退役）

- 原件 `HEAD` 响应提供版本标识与严格 Range 能力；Office 派生 PDF 与版式转换已退役，Fox 改为在本地使用成熟前端预览库读取原件。
- 该节记录的 Preview Job 客户端、派生 PDF 和转换模式已经删除，不再属于 Fox 的运行链路。
- PDF、DOCX、XLSX 和 PPTX 均读取受控缓存中的原件并交给浏览器端成熟库；PDF、DOCX 与 PPTX 的 UI 始终只展示当前页。
- 文件或模式切换会停止旧轮询并忽略迟到结果；派生元数据未完成校验前不会进入 ready 查看状态。
- 后端契约、隔离策略、缓存身份和验收要求已记录在 [YUXI_KNOWLEDGE_LAYOUT_PREVIEW_REQUIREMENTS.md](./YUXI_KNOWLEDGE_LAYOUT_PREVIEW_REQUIREMENTS.md)。
- 文档标题栏的更多操作已补齐重新加载预览和复制文件信息；重新加载同时刷新 parsed 内容并重建查看器，复制内容包含完整的文件识别与诊断字段。
- 自动验证：前端 Bun `38/38`、契约验收脚本 `4/4`、Agent Runtime `44/44`、Rust `125/125`、TypeScript/Vite 生产构建通过。
- 已增加独立 HTTP 契约验收脚本和可选 Rust 真实服务测试。2026-07-25 对本地 Yuxi 0.7.0 的只读探测确认：健康接口正常、原件 GET 会先执行 Bearer 鉴权，但原件 HEAD 尚未实现（`405`），阶段 D 路由尚未注册（`404`）。
- 知识库后端不再需要 Preview Job、派生内容接口或 LibreOffice Worker。退役原因与兼容策略见 [Office 版式转换退役记录](./YUXI_KNOWLEDGE_LAYOUT_PREVIEW_REQUIREMENTS.md)。

## 16. 2026-07-26 文件预览与引用导航收尾

- PDF 第二页及后续页面白屏问题已修复。查看器保持完整 PDF 文档生命周期，只挂载当前页 Canvas，并禁用在当前 WebView 中不稳定的 OffscreenCanvas/ImageDecoder 路径。
- DOCX、XLS/XLSX/ODS 与 PPTX 已分别使用 `docx-preview`、`@js-preview/excel` 和 `@aiden0z/pptx-renderer` 完成真实原件只读预览；PDF 使用 `pdfjs-dist`。
- 知识库详情正文采用统一的自适应预览卡片，外层页面不出现重复滚动条，长内容在查看器内部滚动；最大宽度保留阅读约束，窗口最大化时仍会响应可用空间。
- 引用卡片可以打开对应知识库与文档。PDF 按页码打开；DOCX 优先匹配引用文字并滚动、高亮，在存在多个 `section.docx` 时同步切换到对应页。内部 chunk ID 不再显示在引用弹窗和定位提示中。
- 对话右侧“文件”面板已收敛为项目文件树，不再在窄侧栏中重复展示只读代码预览。文件点击后的主内容区打开方式留待独立交互方案确认。
- 最新验证：前端 Bun `33/33`、TypeScript 与 Vite 生产构建通过。大 chunk 优化已登记为后续性能专项，本轮不改动已稳定的预览与对话加载边界。
- 发布回归：Agent Runtime `44/44`、Rust `119/119`、Runtime Sidecar smoke test、`cargo fmt --check`、`git diff --check` 和 NSIS release 构建全部通过。Rust 仍仅有 2 条既有 dead-code 警告。
