- # Fox Local Workspace Search 原生技术方案

  ## —— 参考 zvec-grep 思路，基于 Fox 现有能力原生实现

  ## 1. 方案结论

  本项目不再定义为：

  > “将 zvec-grep 接入 Fox。”

  而正式定义为：

  > **建设 Fox Local Workspace Search（本地工作区搜索）能力。**

  zvec-grep 的角色调整为：

  > **参考实现 + 功能规格书 + P0 Benchmark 对照组**

  Fox 正式产品路线采用：

  > **Native Workspace Search**

  即直接基于 Fox 当前已经存在的：

  ```
  Tauri / Rust
  +
  Node pi-agent runtime
  +
  SQLite FTS5
  +
  zvec-rust
  +
  ONNX Local Embedding
  +
  Local Knowledge ingestion/index pipeline
  +
  MCP / Tool / Permission infrastructure
  ```

  增加本地工作区生命周期和搜索能力。

  核心原则：

  ```
  不增加第二套知识基础设施
  
  不增加第二套 embedding runtime
  
  不增加第二套 vector index
  
  不增加第二套 document parser
  
  不把 zvec-grep 变成正式运行时依赖
  ```

  最终新增的核心能力主要是：

  ```
  Workspace Manager
  
  Folder Scan
  
  Folder Watcher
  
  Exact / Regex Search
  
  Workspace Source Lifecycle
  
  Hybrid Search
  
  RRF
  
  Agent Search Tool
  ```

  # 2. 产品目标

  Fox 当前“本地知识”主要属于：

  > **Import-based Knowledge**

  即：

  ```
  用户选择文件
      ↓
  导入
      ↓
  解析
      ↓
  Chunk
      ↓
  Embedding
      ↓
  SQLite / FTS5
  +
  zvec
      ↓
  Knowledge Search
  ```

  Local Workspace Search 要解决另一类需求：

  > **让用户授权一个真实工作目录，Fox 持续感知目录中的内容和变化，并可以直接搜索。**

  例如用户授权：

  ```
  D:\工程技术部资料
  
  D:\PLC程序
  
  D:\Projects
  
  D:\个人技术资料
  ```

  然后可以直接问 Fox：

  ```
  %I00017 在哪里出现过？
  
  哪个项目实现过 GE PLC 自动重连？
  
  找一下以前写过的小车定位异常方案。
  
  reconnect_plc() 在哪里被调用？
  
  有没有和“设备突然不知道自己在哪了”相关的资料？
  ```

  因此产品上明确区分：

  ```
  Local Knowledge
  =
  用户主动导入、明确沉淀的知识
  
  Local Workspace
  =
  用户授权、持续同步的工作目录
  ```

  两者产品语义不同。

  但底层：

  ```
  Parser
  Normalizer
  Chunking
  Embedding
  FTS
  Vector Index
  Source Tracking
  ```

  尽量全部复用。

  # 3. 当前 Fox 已具备的能力

  现有仓库已经具备：

  ```
  Rust Backend
  
  SQLite
  
  SQLite FTS5
  
  zvec-rust
  
  ONNX Runtime
  
  Tokenizer
  
  Local Embedding Model Management
  
  Local Knowledge Import
  
  Document Parsing
  
  Chunking
  
  Embedding Generation
  
  Vector Generation
  
  Index Generation / Task Lifecycle
  
  Source Tracking
  
  Open Local File
  
  MCP Client
  
  Node pi-agent Runtime
  
  Tool Adapter
  
  Read-only Tool Policy
  
  Validation Policy
  ```

  因此 Fox 并不是缺少一整套 zvec-grep。

  真正新增的工程范围集中在：

  ```
  1. Workspace 生命周期
  
  2. 文件夹扫描
  
  3. 增量同步
  
  4. Exact Search
  
  5. Hybrid Fusion
  
  6. Agent 暴露
  ```

  # 4. 总体架构

  最终正式架构：

  ```
                           Fox Desktop
                                │
                           React UI
                                │
                              Tauri
                                │
                          Rust Backend
                                │
                ┌───────────────┴───────────────┐
                │                               │
          Local Knowledge                 Local Workspace
                │                               │
         Imported Membership            Mounted Membership
                │                               │
                └──────────────┬────────────────┘
                               │
                        Shared Source Core
                               │
              ┌────────────────┼────────────────┐
              │                │                │
            Parser          Chunker         Source Store
              │                │                │
              └────────────────┼────────────────┘
                               │
                    Shared Search Infrastructure
                               │
             ┌─────────────────┼─────────────────┐
             │                 │                 │
        SQLite FTS5         zvec-rust      Exact Search
           BM25              Vector        Literal/Regex
             │                 │                 │
             └────────────┬────┴─────────────────┘
                          │
                    Provider-level RRF
                          │
                          ▼
                WorkspaceSearchProvider
                          │
                          ▼
                NativeWorkspaceSearch
                          │
                          ▼
               search_local_workspace
                          │
                          ▼
                  Node pi-agent
                          │
                          ▼
                      Fox Agent
  ```

  # 5. zvec-grep 的正式定位

  zvec-grep 不进入 Fox 正式 Product Dependency Graph。

  其作用仅限：

  ```
  Reference Implementation
  
  Hybrid Search Reference
  
  Exact / Semantic Routing Reference
  
  Workspace UX Reference
  
  Retrieval Benchmark Baseline
  ```

  建议放在：

  ```
  experiments/
  └── zvec-grep-benchmark/
  ```

  或者：

  ```
  benchmarks/
  └── providers/
      └── zvec_grep/
  ```

  明确标记：

  ```
  non-production
  throwaway
  benchmark-only
  ```

  P0 可以通过现有 MCP Client 临时验证：

  ```
  Fox MCP Client
       ↓
  zvec-grep
       ↓
  Workspace Search
  ```

  但不做：

  ```
  Installer Integration
  
  Process Lifecycle Integration
  
  Auto Update
  
  正式配置系统
  
  正式 Workspace UI
  
  正式权限集成
  ```

  目的只有一个：

  > **建立搜索效果基线。**

  # 6. 正式 Provider 设计

  正式代码只保留：

  ```
  WorkspaceSearchProvider
          │
          ▼
  NativeWorkspaceSearch
  ```

  建议 trait：

  ```
  pub trait WorkspaceSearchProvider {
      async fn search(
          &self,
          request: WorkspaceSearchRequest,
      ) -> Result<Vec<RetrievalHit>, WorkspaceSearchError>;
  
      async fn status(
          &self,
          workspace_id: &str,
      ) -> Result<WorkspaceStatus, WorkspaceSearchError>;
  }
  ```

  底层具体使用：

  ```
  FTS5
  
  zvec
  
  Exact Search
  
  RRF
  ```

  都被 Provider 层屏蔽。

  上层：

  ```
  UI
  
  Tauri Commands
  
  Node Runtime
  
  Agent Tool
  ```

  都不感知具体搜索引擎实现。

  # 7. Source 与 Membership 模型

  这是本方案非常重要的一项修正。

  必须避免：

  ```
  同一个文件
  既被导入 Local Knowledge
  又位于 Workspace
  ↓
  重复解析
  重复 embedding
  重复索引
  重复检索结果
  ```

  因此引入：

  > **Single Source, Multiple Memberships**

  核心模型：

  ```
  Physical File
       │
       ▼
     Source
       │
  canonical source_id
       │
   ┌───┴───────────────┐
   │                   │
  Local Knowledge    Workspace
  Membership         Membership
   │                   │
   └─────────┬─────────┘
             │
       Shared Content
             │
        Shared Chunks
             │
         Shared Index
  ```

  例如：

  ```
  D:\manuals\RTG_manual.pdf
  ```

  可能同时属于：

  ```
  local_knowledge
  
  workspace:engineering
  ```

  但：

  ```
  Parse = 1 次
  
  Chunk = 1 次
  
  Embedding = 1 次
  
  Vector = 1 份
  ```

  # 8. Source ID

  建议 Source 使用：

  ```
  canonical_path
  +
  filesystem identity
  ```

  作为主要识别依据。

  不要只用普通字符串路径。

  需要处理：

  ```
  Path normalization
  
  Case normalization
  
  Windows drive semantics
  
  Rename
  
  Move
  
  Symlink
  
  Junction
  ```

  可以进一步维护：

  ```
  source_id
  
  canonical_path
  
  file_identity
  
  content_hash
  
  size
  
  mtime
  
  created_at
  
  updated_at
  ```

  # 9. Workspace Manager

  新增：

  ```
  WorkspaceManager
  ```

  负责：

  ```
  Add Workspace
  
  Remove Workspace
  
  Enable / Disable
  
  Manual Refresh
  
  Scan Status
  
  Index Status
  
  Exclude Rules
  
  Membership Management
  ```

  建议数据模型：

  ```
  workspace
  
  id
  name
  root_path
  canonical_root_path
  
  enabled
  
  scan_status
  index_status
  
  file_count
  
  last_scan_at
  last_indexed_at
  
  created_at
  updated_at
  ```

  状态：

  ```
  READY
  
  SCANNING
  
  INDEXING
  
  STALE
  
  ERROR
  
  DISABLED
  ```

  # 10. Workspace UI

  示意：

  ```
  本地工作区
  
  ┌─────────────────────────────────────┐
  │ 工程技术部资料                      │
  │ D:\工程技术部资料                   │
  │                                     │
  │ 12,486 files                        │
  │ ✓ 索引已更新                        │
  │                                     │
  │ [刷新] [设置] [移除]                │
  └─────────────────────────────────────┘
  
  ┌─────────────────────────────────────┐
  │ Fox Projects                        │
  │ D:\python\projects\Fox              │
  │                                     │
  │ 4,283 files                         │
  │ ✓ 索引已更新                        │
  └─────────────────────────────────────┘
  
              + 添加工作区
  ```

  # 11. Workspace 安全模型

  必须坚持：

  > **Explicit Folder Allowlist**

  只有用户主动授权的目录允许被 Fox 搜索。

  例如：

  ```
  Allowed
  
  D:\Projects
  
  D:\Engineering
  
  D:\PLC
  ```

  不能因为 Workspace 位于：

  ```
  D:\
  ```

  就让 Agent 任意访问整个磁盘。

  # 12. Canonical Path Security

  不能简单使用：

  ```
  path.starts_with(workspace_root)
  ```

  判断权限。

  必须：

  ```
  requested path
       ↓
  canonicalize
       ↓
  resolve symlink / junction
       ↓
  resolve final target
       ↓
  check canonical workspace boundary
       ↓
  allow / deny
  ```

  需要防止：

  ```
  D:\Work\secret-link
         ↓
  C:\Users\User\.ssh
  ```

  这样的路径逃逸。

  # 13. 特殊文件系统

  第一版优先保证：

  ```
  Local Fixed NTFS Drive
  ```

  可靠运行。

  下列类型建议显式检测：

  ```
  UNC Path
  
  SMB / NAS
  
  Network Drive
  
  OneDrive Placeholder
  
  Cloud Sync Folder
  
  Removable Drive
  ```

  初期可：

  ```
  Unsupported
  
  或
  
  Experimental
  ```

  不要默认认为所有文件系统都能提供稳定 watcher 语义。

  # 14. Secret Excludes

  默认排除：

  ```
  .env
  .env.*
  
  *.pem
  *.key
  *.pfx
  *.p12
  
  credentials.*
  secrets.*
  
  id_rsa
  id_ed25519
  
  .git
  node_modules
  target
  dist
  build
  
  Fox Runtime Directory
  Fox Index Directory
  ```

  并支持：

  ```
  Global Excludes
  
  +
  
  Per Workspace Excludes
  ```

  例如：

  ```
  *.log
  
  archive/**
  
  backup/**
  
  tmp/**
  ```

  # 15. 索引目录

  所有 Workspace Runtime / Index 必须存放到 Fox 自身目录。

  例如：

  ```
  %LOCALAPPDATA%\Fox\data\workspaces\
  ```

  结构：

  ```
  workspaces/
  └── {workspace_id}/
      ├── runtime/
      ├── metadata/
      ├── state/
      └── index/
  ```

  禁止：

  ```
  D:\Engineering\.zvec-grep
  ```

  或者任何：

  ```
  Workspace Root 内部索引目录
  ```

  避免：

  ```
  污染 Git Repository
  
  OneDrive / NAS 同步
  
  PLC 工程目录变化
  
  watcher 自己监听自己
  
  备份污染
  ```

  # 16. Index Fingerprint

  Workspace Index 必须明确记录索引版本。

  因为 Fox 已支持：

  ```
  Embedding Model Change
  ```

  未来还可能调整：

  ```
  Parser
  
  Chunking
  
  Index Schema
  ```

  建议记录：

  ```
  embedding_model_id
  
  embedding_model_version
  
  embedding_dimension
  
  chunking_version
  
  parser_version
  
  index_schema_version
  ```

  生成：

  ```
  index_fingerprint
  ```

  例如：

  ```
  hash(
      model_id
      + model_version
      + chunk_version
      + parser_version
      + schema_version
  )
  ```

  如果 Fingerprint 变化：

  ```
  READY
   ↓
  STALE
   ↓
  REBUILD_REQUIRED
  ```

  并复用 Fox 已有：

  ```
  generation
  
  index task
  
  rebuild
  ```

  机制。

  # 17. P0：只验证 Retrieval

  P0 不建设完整 Workspace 产品。

  P0 的目标只有：

  > **证明 Fox Native Search 的搜索质量足够好。**

  因此 P0 不要求：

  ```
  Realtime Watcher
  
  Incremental File Events
  
  复杂 Workspace UI
  
  Tree-sitter
  
  Unified Retrieval
  
  Reranker
  ```

  P0 最小链路：

  ```
  Add Folder
       ↓
  Manual Scan
       ↓
  Existing Parse
       ↓
  Existing Chunk
       ↓
  FTS5
  +
  Existing ONNX + zvec
  +
  Exact Search
       ↓
  RRF
       ↓
  RetrievalHit
  ```

  # 18. P0 Golden Set

  整个功能开发第一步：

  > **建立 Retrieval Golden Set**

  先完成：

  ```
  30 条
  ```

  跑通评测系统。

  然后扩展：

  ```
  50~100 条
  ```

  真实工程问题。

  问题类型至少包括：

  ```
  Exact
  
  PLC Address
  
  Error Code
  
  Filename
  
  Code Symbol
  
  Keyword
  
  Semantic
  
  Cross-file
  
  Negative Query
  ```

  # 19. Golden Set 示例

  例如：

  ```
  %I00017 出现在哪些文件？
  
  DB398.DBX4.1 在哪里使用？
  
  reconnect_plc() 在哪里定义？
  
  哪个地方调用 reconnect_plc？
  
  找小车定位故障资料。
  
  以前哪个项目处理过 GE PLC 断线自动重连？
  
  设备走着走着突然不知道自己在哪了。
  
  项目里有没有 DB999？
  ```

  最后一个属于：

  ```
  Negative Query
  ```

  Ground Truth 必须记录：

  ```
  Expected File
  
  Expected Chunk
  
  Expected Line / Section
  
  Relevant Files
  
  Should Return No Result
  ```

  # 20. Negative Query

  评测集建议：

  ```
  Positive Query
  70~80%
  
  Negative Query
  20~30%
  ```

  目的是验证：

  > **搜不到的时候，系统能不能正确返回“没有可靠结果”。**

  避免：

  ```
  Query:
  DB999 在哪里？
  
  系统实际没有 DB999
  
  Vector Search:
  返回 DB998 / DB900 / DB99
  
  LLM:
  开始编造
  ```

  需要增加：

  ```
  No Result Threshold
  
  Minimum Confidence Policy
  ```

  # 21. P0 Benchmark 指标

  Retrieval：

  ```
  Recall@5
  
  Recall@10
  
  MRR
  
  Top1 Hit Rate
  
  False Positive Rate
  
  No-answer Precision
  ```

  性能：

  ```
  P50 Search Latency
  
  P95 Search Latency
  
  Initial Index Time
  
  Index Size
  
  Peak Memory
  
  Idle Memory
  ```

  桌面成本：

  ```
  Installer Size Delta
  
  Runtime Dependency Delta
  ```

  Agent：

  ```
  Answer Accuracy
  
  Citation Accuracy
  
  Unsupported Answer Rate
  ```

  # 22. P0-A：zvec-grep Reference

  目的：

  > **建立外部参考基线。**

  完成最小：

  ```
  Folder
   ↓
  Index
   ↓
  Search
   ↓
  Exact / Hybrid
   ↓
  File Path
   ↓
  Line Number
  ```

  使用同一个 Golden Set。

  不做任何正式产品化。

  # 23. P0-B：Fox Native Spike

  产品主线。

  实现：

  ```
  Folder
    ↓
  Manual Scan
    ↓
  Existing Parse / Chunk
    ↓
  ┌──────────────┐
  │ Exact Search │
  ├──────────────┤
  │ FTS5 / BM25  │
  ├──────────────┤
  │ zvec Vector  │
  └──────────────┘
         ↓
        RRF
         ↓
   RetrievalHit
  ```

  Embedding：

  ```
  Existing ONNX Runtime
  ```

  不增加第二套模型。

  # 24. Exact Search

  P0 需求是：

  ```
  Literal
  
  Regex
  
  Glob
  
  Case-sensitive
  
  Case-insensitive
  
  Line Number
  
  Context
  ```

  适用于：

  ```
  %I00017
  
  DB398.DBX4.1
  
  connect_plc()
  
  E_STOP_CHAIN
  
  someFunctionName
  ```

  # 25. Exact Search 实现不提前锁死

  P0 可以直接使用：

  ```
  ripgrep
  ```

  作为成熟 baseline。

  但正式产品不提前决定：

  ```
  rg.exe
  ```

  是否永久打包。

  Production 候选：

  ```
  Option A
  
  Bundled ripgrep
  
  
  Option B
  
  Native Rust Search
  
  walkdir / ignore
  +
  regex
  +
  aho-corasick
  ```

  P1 前通过：

  ```
  Performance
  
  Binary Size
  
  Feature Completeness
  
  Maintenance Cost
  ```

  决定。

  因此正式抽象应该是：

  ```
  ExactSearchEngine
  ```

  而不是：

  ```
  RipgrepEngine
  ```

  # 26. BM25

  复用：

  ```
  SQLite FTS5
  ```

  承担关键词搜索：

  ```
  小车 定位 故障
  
  GE PLC 通讯
  
  编码器 异常
  
  自动 重连
  ```

  BM25 解决：

  > **哪些文本最符合用户使用的关键词。**

  # 27. Semantic Search

  继续使用：

  ```
  Existing ONNX Embedding
  +
  zvec-rust
  ```

  例如：

  ```
  用户：
  设备突然不知道自己在哪了
  
  文档：
  定位编码器异常导致位置反馈丢失
  ```

  Vector Search 用于发现：

  ```
  Semantic Similarity
  ```

  # 28. Provider 内 Hybrid Fusion

  P0 的 RRF 属于：

  > **Intra-provider Fusion**

  即：

  ```
  NativeWorkspaceSearch
  
  BM25 ────┐
           ├── RRF
  Vector ──┘
  ```

  不要和未来：

  ```
  Local Knowledge
  
  Workspace
  
  Graph
  
  Database
  ```

  之间的跨源融合混为一谈。

  # 29. RRF

  公式：

  ```
  RRF(d) = Σ 1 / (k + rank_i(d))
  ```

  初期不需要：

  ```
  Score normalization
  
  复杂 Weight Tuning
  
  LLM Reranker
  ```

  原因是：

  ```
  BM25 Score
  
  Vector Similarity
  ```

  不在同一数值尺度。

  RRF 可以直接融合 ranking。

  # 30. Query Classifier

  P0 不做复杂 Router。

  只做：

  ```
  WorkspaceQueryClassifier
  ```

  确定性规则：

  ```
  PLC 地址
  
  Error Code
  
  函数名
  
  代码 Symbol
  
  明显路径
  
  文件名
  
  固定字符串
  
  正则表达式
  
  → Exact
  ```

  普通自然语言：

  ```
  → Hybrid
  ```

  例如：

  ```
  %I00017
  
  → exact
  哪个地方调用 reconnect_plc
  
  → exact / hybrid fallback
  以前有没有处理过 PLC 自动重连
  
  → hybrid
  ```

  不使用 LLM 做 Router。

  # 31. Exact → Hybrid Fallback

  建议增加一个简单策略：

  ```
  Exact Search
      ↓
  Result Count = 0
      ↓
  Optional Hybrid Fallback
  ```

  例如：

  ```
  reconnectPLC
  ```

  代码实际可能写成：

  ```
  reconnect_plc
  ```

  这时 exact 失败后可以让 semantic / lexical 补救。

  但必须区分：

  ```
  Exact hit
  
  Fallback hit
  ```

  避免给用户造成“精确搜索命中”的错觉。

  # 32. RetrievalHit

  P0 就定义统一结构。

  建议：

  ```
  pub struct RetrievalHit {
      pub source_type: String,
      pub source_id: String,
  
      pub title: String,
      pub path: Option<String>,
  
      pub content: String,
  
      pub line_start: Option<u32>,
      pub line_end: Option<u32>,
      pub page: Option<u32>,
  
      pub rank: usize,
      pub score: Option<f32>,
  
      pub retrieval_method: String,
  
      pub metadata: serde_json::Value,
  }
  ```

  当前：

  ```
  source_type = workspace
  ```

  以后：

  ```
  workspace
  
  local_knowledge
  
  database
  
  graph
  ```

  # 33. Agent Tool

  Node pi-agent Runtime 只暴露：

  ```
  search_local_workspace
  ```

  继续走现有：

  ```
  host-tools.mjs
  
  tool-adapter.mjs
  
  read-only-tool
  
  validation-policy
  ```

  能力。

  输入建议：

  ```
  query
  
  workspace_ids?
  
  mode:
  auto | exact | hybrid
  
  limit?
  
  file_types?
  
  glob?
  ```

  默认：

  ```
  mode = auto
  ```

  # 34. Agent 不允许管理 Workspace

  Agent 不允许调用：

  ```
  add_workspace
  
  remove_workspace
  
  change_root
  
  delete_index
  
  rebuild_index
  
  change_embedding_model
  ```

  这些全部属于：

  ```
  UI
  +
  Rust Backend Management Layer
  ```

  Agent 只：

  ```
  Search
  
  Read Result
  ```

  # 35. Source Citation

  返回结果必须可追溯。

  例如：

  ```
  根据 ge_driver.py 中的 reconnect 逻辑，
  PLC 通讯异常后会重新进入连接流程。
  ```

  显示：

  ```
  ge_driver.py
  
  D:\Projects\collector\ge_driver.py
  
  Lines 126–157
  ```

  支持：

  ```
  打开文件
  
  打开所在目录
  
  复制路径
  ```

  未来可增加：

  ```
  Open in VS Code
  ```

  # 36. P0 Decision Gate

  P0 的核心问题：

  > **Native Search 是否已经足够好？**

  例如：

  ```
                        zvec-grep       Fox Native
  
  Recall@5                 92%              90%
  
  Top1                      81%              80%
  
  P95                      180ms             90ms
  
  Idle Memory              +400MB            +60MB
  
  Runtime                  Node               None
  ```

  如果 Native 与 zg 差距很小：

  > 直接采用 Native。

  如果：

  ```
  zvec-grep = 93%
  
  Native = 65%
  ```

  再拆解差距：

  ```
  Chunking
  
  Embedding
  
  Code Structure
  
  Hybrid Strategy
  
  File Parsing
  
  Query Classification
  ```

  只吸收缺失能力。

  而不是直接引入整个 zvec-grep。

  # 37. P0 验收建议

  初步阈值：

  ```
  Recall@5 ≥ 85%
  
  Top1 Hit Rate ≥ 70%
  
  Exact Search P95 ≤ 200ms
  
  Hybrid P95 ≤ 500ms
  ```

  同时满足：

  ```
  No Remote Dependency
  
  No Workspace Pollution
  
  No Unauthorized Path Access
  
  Agent Read-only
  
  Result Traceable
  ```

  具体性能阈值根据员工电脑真实硬件再调整。

  # 38. P1：Workspace Productization

  P0 通过后才进入完整 Workspace 产品化。

  新增：

  ```
  Workspace Manager
  
  Directory Watcher
  
  Incremental Scan
  
  Incremental Index
  
  Membership Update
  
  Index Status
  
  Error Recovery
  
  Exclude Rules
  
  Citation
  
  Open File
  ```

  # 39. Directory Watcher

  P1 开始实现。

  流程：

  ```
  Filesystem Event
         ↓
  Workspace Watcher
         ↓
  Debounce
         ↓
  Batch Changes
         ↓
  State Compare
         ↓
  Incremental Update
  ```

  处理：

  ```
  Create
  
  Modify
  
  Delete
  
  Rename
  ```

  避免：

  ```
  IDE Save
  ↓
  10 个 File Event
  ↓
  重复 embedding
  ```

  因此必须：

  ```
  Debounce
  
  Batching
  
  Deduplication
  
  Retry
  ```

  # 40. Periodic Reconciliation

  Watcher 不能作为唯一事实来源。

  建议增加：

  ```
  Startup Reconciliation
  
  +
  
  Low-frequency Reconciliation
  ```

  比较：

  ```
  path
  
  size
  
  mtime
  
  必要时 content_hash
  ```

  修复：

  ```
  missed filesystem event
  
  offline change
  
  network/cloud delayed change
  ```

  # 41. Incremental Index

  只处理：

  ```
  Changed File
  ```

  流程：

  ```
  File Changed
      ↓
  Source Update
      ↓
  Parse
      ↓
  Chunk Diff
      ↓
  Embedding
      ↓
  FTS Update
      ↓
  Vector Update
  ```

  禁止：

  ```
  目录中一个文件变化
  ↓
  整目录重建
  ```

  # 42. P1.5：Code Intelligence

  不放进 P0。

  先用：

  ```
  Exact
  
  BM25
  
  Vector
  ```

  评测。

  如果：

  ```
  Function Search
  
  Class Search
  
  Symbol Search
  
  Code Context
  ```

  仍明显不足，再引入：

  ```
  Tree-sitter
  ```

  增加：

  ```
  Function Chunk
  
  Class Chunk
  
  Symbol Metadata
  
  Language Metadata
  ```

  P1.5 不做过度代码图谱化。

  # 43. P2：复用现有 Document Pipeline

  Workspace 中如果出现：

  ```
  PDF
  
  DOCX
  
  PPTX
  
  XLSX
  ```

  不要新增：

  ```
  Workspace Parser
  ```

  第二条解析通路。

  必须：

  ```
  Workspace Source
        ↓
  Existing Local Knowledge Parser
        ↓
  Existing Normalizer
        ↓
  Existing Chunker
        ↓
  Existing Embedding
        ↓
  Shared Index
  ```

  即：

  > **不同的是 Source Lifecycle，不是 Knowledge Processing。**

  # 44. Local Knowledge 与 Workspace 的底层关系

  最终：

  ```
                      Source Layer
                           │
                      Physical File
                           │
                   Shared Content Core
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
      Parse              Chunk             Index
        │                  │                  │
        └──────────────────┼──────────────────┘
                           │
                Membership / Collection
                           │
               ┌───────────┴───────────┐
               │                       │
         Local Knowledge          Workspace
  ```

  这比：

  ```
  Local Knowledge Pipeline
  
  Workspace Pipeline
  ```

  两套平行系统更合理。

  # 45. P3：Unified Retrieval

  直到出现两个独立检索来源：

  ```
  Local Knowledge
  
  Local Workspace
  ```

  以后，才建设：

  ```
  Cross-source Retrieval
  ```

  结构：

  ```
  User Query
      ↓
  Retrieval Router
      │
      ├── Local Knowledge
      │
      └── Local Workspace
              ↓
         RetrievalHit
              ↓
       Cross-source RRF
              ↓
            Agent
  ```

  这属于：

  > **Level 2 Fusion**

  与 P0 Workspace 内部：

  ```
  BM25 + Vector → RRF
  ```

  完全区分。

  # 46. 暂不增加 Reranker

  P3 第一版仍然：

  ```
  RRF only
  ```

  只有评测证明：

  ```
  RRF quality insufficient
  ```

  再增加：

  ```
  Local Reranker
  ```

  否则会增加：

  ```
  模型体积
  
  内存
  
  推理延迟
  
  部署复杂度
  ```

  与 Fox local-first 目标冲突。

  # 47. Future Retrieval

  只有对应系统真实落地后再加入：

  ```
  Equipment Database
  
  Fault Database
  
  Business Data
  
  Graph Database
  ```

  未来可能形成：

  ```
  Lexical
  
  Semantic
  
  Structured
  
  Relational
  ```

  多路 Retrieval。

  但这些不参与当前 Workspace Search 的工程决策。

  # 48. 最终 Roadmap

  ```
  P0 — Retrieval Feasibility
  │
  ├── Golden Set
  │
  ├── Negative Queries
  │
  ├── zvec-grep Benchmark
  │
  ├── Native Manual Scan
  │
  ├── Exact Search
  │
  ├── FTS5
  │
  ├── Existing ONNX + zvec
  │
  ├── Provider-level RRF
  │
  └── Decision Gate
  │
  ▼
  P1 — Productization
  │
  ├── Workspace Manager
  ├── Membership Model
  ├── Canonical Path Security
  ├── Directory Watcher
  ├── Incremental Index
  ├── Reconciliation
  ├── Exclude Rules
  ├── Index Fingerprint
  ├── Citation
  └── Agent Tool
  │
  ▼
  P1.5 — Code Intelligence
  │
  └── Tree-sitter
  │
  ▼
  P2 — Shared Document Processing
  │
  └── Workspace 复用 Local Knowledge Pipeline
  │
  ▼
  P3 — Unified Retrieval
  │
  ├── Local Knowledge
  │
  ├── Workspace
  │
  └── Cross-source RRF
  │
  ▼
  Future
  │
  ├── Equipment Data
  ├── Business Data
  └── Graph Retrieval
  ```

  # 49. 本方案明确不做什么

  当前阶段不做：

  ```
  把 zvec-grep 作为正式 sidecar
  
  引入第二套 Node runtime
  
  引入第二套 embedding 模型
  
  引入第二套 vector index
  
  重新做 PDF / Office pipeline
  
  复杂 LLM Retrieval Router
  
  本地 reranker
  
  Neo4j
  
  多数据库统一检索
  
  代码知识图谱
  ```

  这些能力只有在真实需求或评测数据证明必要时再增加。

  # 50. 最终技术决策

  ## Decision

  Fox Local Workspace Search 正式采用：

  > **Native Rust Architecture**

  复用：

  ```
  SQLite FTS5
  
  zvec-rust
  
  Existing ONNX Embedding
  
  Local Knowledge Parser / Chunker
  
  Index Generation
  
  Source Tracking
  
  Existing Tool / Permission System
  ```

  新增：

  ```
  Workspace Manager
  
  Source Membership
  
  Folder Scan
  
  Watcher
  
  Incremental Sync
  
  Exact Search
  
  Provider-level Hybrid RRF
  
  Canonical Path Security
  
  Index Fingerprint
  
  search_local_workspace
  ```

  zvec-grep：

  ```
  Benchmark only
  
  Reference only
  
  Not production dependency
  ```

  # 51. 最终一句话定义

  本项目的本质不是：

  > **“给 Fox 增加一个新的 RAG。”**

  也不是：

  > **“把 zvec-grep 集成到 Fox。”**

  而是：

  > **把 Fox 已有的本地知识、FTS5、zvec-rust 和 ONNX 搜索能力，从“导入文件”扩展到“持续理解用户授权的工作目录”。**

  最终让 Fox 同时理解：

  ```
  公司沉淀的知识
  
  +
  
  员工自己正在工作的资料
  
  +
  
  项目代码和配置
  ```

  让：

  > **“员工自己的智能伙伴”**

  真正拥有对员工本地工作环境的理解能力。
