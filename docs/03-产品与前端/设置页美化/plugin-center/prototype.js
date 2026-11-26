const ROOT = '../../../../apps/desktop/public'
const ME = `${ROOT}/avatars/defaults/fox_big_smile.png`
const FOX = `${ROOT}/avatars/fox-assistant.png`
const EXPERT_AVATAR_BASE = `${ROOT}/avatars/experts`
const DEV_COMPANION = `${EXPERT_AVATAR_BASE}/Fox_SoftwareDeveloper.png`
const WRITER_COMPANION = `${EXPERT_AVATAR_BASE}/Fox_ContentWriter.png`

/* Extra Lucide & Fox icons needed for Main Workspace Sidebar & Experts */
const EXTRA_ICONS = {
  'square-kanban': '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M8 7v7"/><path d="M12 7v4"/><path d="M16 7v9"/>',
  'cat-paw': '<g transform="translate(12.634 13.41) rotate(-10)"><path d="M-5.792 3.379C-5.792-1.126-2.735-2.735 0-2.735C2.735-2.735 5.792-1.126 5.792 3.379C5.792 6.275 4.023 7.401 2.414 7.401C1.287 7.401 0.805 6.758 0 6.758C-0.805 6.758-1.287 7.401-2.414 7.401C-4.023 7.401-5.792 6.275-5.792 3.379Z"/><ellipse cx="-7.624" cy="-3.149" rx="2.333" ry="2.896" transform="rotate(-30 -7.624 -3.149)"/><ellipse cx="-2.9" cy="-7.292" rx="2.333" ry="2.896" transform="rotate(-10 -2.9 -7.292)"/><ellipse cx="2.9" cy="-7.292" rx="2.333" ry="2.896" transform="rotate(10 2.9 -7.292)"/><ellipse cx="7.624" cy="-3.149" rx="2.333" ry="2.896" transform="rotate(30 7.624 -3.149)"/></g>',
  'book-copy': '<path d="M5 7a2 2 0 0 0-2 2v11"/><path d="M5.803 18H5a2 2 0 0 0 0 4h9.5a.5.5 0 0 0 .5-.5V21"/><path d="M9 15V4a2 2 0 0 1 2-2h9.5a.5.5 0 0 1 .5.5v14a.5.5 0 0 1-.5.5H11a2 2 0 0 1 0-4h10"/>',
  bell: '<path d="M10.268 21a2 2 0 0 0 3.464 0"/><path d="M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326"/>',
  'message-circle-plus': '<path d="M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719"/><path d="M8 12h8"/><path d="M12 8v8"/>',
  copy: '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
  download: '<path d="M12 15V3"/><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/>',
  upload: '<path d="M12 3v12"/><path d="m17 8-5-5-5 5"/><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>',
  send: '<path d="M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11z"/><path d="m21.854 2.147-10.94 10.939"/>',
  history: '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>',
  'calendar-clock': '<path d="M21 7.5V6a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h3.5"/><path d="M16 2v4"/><path d="M8 2v4"/><path d="M3 10h5"/><path d="M17.5 17.5 16 16.3V14"/><circle cx="16" cy="16" r="6"/>',
  pause: '<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>',
  'user-plus': '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><line x1="19" x2="19" y1="8" y2="14"/><line x1="22" x2="16" y1="11" y2="11"/>',
  'check-check': '<path d="M18 6 7 17l-5-5"/><path d="m22 10-7.5 7.5L13 16"/>',
}

/* Unchanged Fox Settings Sidebar navigation (exact match to 设置页美化/index.html) */
const SETTINGS_NAV = [
  ['偏好', [['general', 'sliders-horizontal', '通用'], ['conv', 'message-circle-more', '对话'], ['companions', 'users', '伙伴']]],
  ['能力', [['models', 'boxes', '模型'], ['agent', 'bot', 'Agent'], ['plugins', 'blocks', '插件'], ['library', 'library-big', '资料库']]],
  ['系统', [['security', 'shield-check', '权限与安全'], ['data', 'database', '数据与关于']]],
]

const directory = {
  connector: { name: '连接器', icon: 'cable', action: '添加连接器', statuses: ['全部', '已配置', '待筛选', '已选候选'] },
  plugin: { name: '插件', icon: 'blocks', action: '管理搜索服务', statuses: ['全部', '已启用', '待启用'] },
  skill: { name: '技能', icon: 'puzzle', action: '导入技能', statuses: ['全部', '已启用', '待启用'] },
  tool: { name: '内置工具', icon: 'wrench', action: '插件策略', statuses: ['全部', '已启用', '已停用'] },
}

const groupIcons = {
  '全部分类': 'layers', '即时通讯': 'message-circle-more', '文档与网盘': 'folder-open',
  '项目与研发': 'git-branch', '邮件与日程': 'calendar', '设计与经营': 'pencil',
  '数据与检索': 'database', '集成平台': 'link', '已配置': 'cable',
  '内置工具': 'wrench', '网页搜索': 'globe', '办公技能': 'file-text', '本地技能': 'puzzle',
}

/* Plugins (Web Search & extension plugins — cleanly separated from Built-in Tools) */
const plugins = [
  { id: 'p-free', name: '免 Key 搜索', sub: '网页搜索', group: '网页搜索', mark: 'globe', publisher: 'Fox 内置', description: 'DuckDuckGo 被拦截或无结果时自动改用 Yahoo，开箱即用。' },
  { id: 'p-tavily', name: 'Tavily', sub: '网页搜索', group: '网页搜索', mark: 'globe', publisher: 'Tavily', description: '为 AI 优化的实时网页搜索与正文提取，需 API Key。' },
  { id: 'p-brave', name: 'Brave Search', sub: '网页搜索', group: '网页搜索', mark: 'brave', publisher: 'Brave', description: '使用独立网页索引进行实时搜索，需 API Key。' },
  { id: 'p-exa', name: 'Exa', sub: '网页搜索', group: '网页搜索', mark: 'globe', publisher: 'Exa', description: '按语义检索高质量网页与内容摘要，需 API Key。' },
  { id: 'p-searxng', name: 'SearXNG', sub: '自托管搜索', group: '网页搜索', mark: 'globe', publisher: '自托管实例', description: '连接自建 SearXNG 搜索服务，需实例地址。' },
]

/* Fox Built-in Core Tools (separated from Plugins) */
const tools = [
  { id: 'files', name: '文件工具', sub: '工作区核心', group: '内置工具', mark: 'folder-open', publisher: 'Fox 内置', description: '列出、搜索和读取工作区文件，受项目权限控制。' },
  { id: 'terminal', name: '终端工具', sub: '工作区核心', group: '内置工具', mark: 'square-terminal', publisher: 'Fox 内置', description: '执行工作区命令，按运行策略和审批规则确认权限。' },
  { id: 'agents', name: '协作工具', sub: '多助手协作', group: '内置工具', mark: 'users', publisher: 'Fox 内置', description: '委派子任务并获取其他助手与专家的工作结果。' },
]

const skills = [
  { id: 'fox-office-word', name: 'Word 文档', group: '办公技能', mark: 'word', publisher: 'Fox 内置', scopeLabel: '办公助手', description: '创建、检查和修改 Word 文档，保留排版样式与修订记录。', tool: 'Fox Office' },
  { id: 'fox-office-excel', name: 'Excel 表格', group: '办公技能', mark: 'excel', publisher: 'Fox 内置', scopeLabel: '办公助手', description: '读取、整理和修改 Excel 工作簿、公式与数据透视表。', tool: 'Fox Office' },
  { id: 'fox-office-ppt', name: 'PowerPoint 演示文稿', group: '办公技能', mark: 'powerpoint', publisher: 'Fox 内置', scopeLabel: '办公助手', description: '生成、编辑和优化幻灯片结构与视觉排版。', tool: 'Fox Office' },
  { id: 'meeting-minutes', name: '会议纪要整理', group: '本地技能', mark: 'file-text', publisher: '个人', scopeLabel: '个人', description: '将录音转写或聊天记录整理成带行动项的会议纪要。', file: 'SKILL.md' },
  { id: 'code-review', name: '代码评审规范', group: '本地技能', mark: 'git-branch', publisher: '个人', scopeLabel: '个人', description: '按团队约定检查 PR 变更、边界条件与可维护性问题。', file: 'SKILL.md' },
  { id: 'prototype-audit', name: '界面一致性检查', group: '本地技能', mark: 'layers', publisher: '个人', scopeLabel: '个人', description: '核对页面配色、圆角、控件高度与 Fox Paper 主题规范。', file: 'SKILL.md' },
]

const office = {
  id: 'fox-office', name: 'Fox Office', sub: '内置 MCP', group: '已配置', mark: 'file-text', publisher: 'Fox 内置',
  description: 'Fox 内置连接器，为 Word、Excel 和 PowerPoint 工具提供本机接口。',
  transport: 'Fox 内置 MCP',
}

/* Pre-installed connectors shown in 设置 · 插件 (matching Codex installed list feel) */
const seedInstalledConnectors = [
  { id: 'inst-github', fromCandidate: 'github', name: 'GitHub', sub: '远程 MCP', group: '已配置', mark: 'github', transport: 'HTTP MCP', target: 'https://api.githubcopilot.com/mcp/', description: '查询仓库、Issue、PR 与 Actions 运行状态。' },
  { id: 'inst-gmail', fromCandidate: 'gmail', name: 'Gmail', sub: 'Google Workspace', group: '已配置', mark: 'gmail', transport: 'HTTP MCP', target: 'https://workspace.googleapis.com/mcp/gmail', description: '搜索邮件、提取线索和整理讨论串。' },
  { id: 'inst-drive', fromCandidate: 'drive', name: 'Google Drive', sub: 'Google Workspace', group: '已配置', mark: 'googledrive', transport: 'HTTP MCP', target: 'https://workspace.googleapis.com/mcp/drive', description: '搜索云端文件并读取获授权的内容。' },
  { id: 'inst-feishu', fromCandidate: 'feishu', name: '飞书 / Lark', sub: 'CLI 适配', group: '已配置', mark: 'lark', transport: 'stdio MCP', target: 'lark-cli mcp', description: '搜索文档、读取多维表格和查询日历。' },
  { id: 'inst-figma', fromCandidate: 'figma', name: 'Figma', sub: '远程 MCP', group: '已配置', mark: 'figma', transport: 'HTTP MCP', target: 'https://mcp.figma.com/mcp', description: '读取组件、设计变量和画布内容。' },
  { id: 'inst-linear', fromCandidate: 'linear', name: 'Linear', sub: '远程 MCP', group: '已配置', mark: 'linear', transport: 'HTTP MCP', target: 'https://mcp.linear.app/sse', description: '查找问题、项目和迭代进度。' },
]

/* Tailored detail metadata for key items; all other catalog items use smart category templates */
const detailMeta = {
  gmail: {
    publisher: 'Google',
    capabilities: '检索、读取、起草回复',
    overview: '接入 Gmail 后，Fox 可以在对话中直接检索收件箱邮件、归纳往来讨论中的关键决策与未决问题，并帮助你起草得体准确的回复草稿。',
    prompts: [
      '总结最近 5 封关于「项目评审」的邮件，提炼已确认的结论、遗留问题和下一步跟进事项',
      '帮我起草一封礼貌而明确的邮件回复给客户，列出我们本周将交付的 3 项清单',
      '把最近的线上反馈邮件串整理成行动追踪表，标明负责人、截止日期和对应邮件摘要',
    ],
    tools: [
      ['search_messages', '按发件人、主题、标签或时间范围检索邮件'],
      ['read_thread', '读取完整邮件会话上下文与附件摘要'],
      ['create_draft', '生成邮件草稿，确认后再发送'],
    ],
  },
  github: {
    publisher: 'GitHub',
    capabilities: '仓库检索、Issue、PR 与 Actions',
    overview: '通过官方 GitHub MCP，Fox 可直接查看代码仓库、审查 Pull Request 差异、跟踪 Issue 进度与检查 CI/Actions 运行日志。',
    prompts: [
      '汇总本周主仓库合并的 PR，按新功能、修复和优化整理一份发版说明',
      '查看最近失败的 GitHub Actions 构建日志，定位报错原因并给出修复建议',
      '检索所有带 bug 标签且超过 7 天未回复的 Issue，按优先级排个处理顺序',
    ],
    tools: [
      ['list_pull_requests', '查询仓库中的 Pull Request 状态与评审记录'],
      ['get_issue', '读取 Issue 详情、标签与历史讨论'],
      ['list_workflow_runs', '检查 GitHub Actions 流水线结果与失败日志'],
    ],
  },
  feishu: {
    publisher: '飞书开放平台',
    capabilities: '云文档、多维表格、日历查询',
    overview: '通过飞书 / Lark 接入，Fox 能检索企业知识库文档、读取和汇总多维表格记录，并结合日历安排生成会议准备材料。',
    prompts: [
      '搜索飞书知识库里最新的《Q4 产品路线图》，提炼各模块的关键里程碑',
      '读取需求池多维表格中状态为「进行中」的条目，按负责人汇总进度风险',
      '查看我明天的飞书日程，为下午的评审会生成一份 10 分钟议程提纲',
    ],
    tools: [
      ['search_docs', '检索飞书云文档与知识库页面'],
      ['query_bitable', '按条件读取多维表格数据与视图'],
      ['list_calendar_events', '获取指定时间段内的日历会议'],
    ],
  },
  linear: {
    publisher: 'Linear',
    capabilities: '交互式、问题跟踪、写入',
    overview: '连接 Linear 后，Fox 可以快速梳理迭代进度、检索关联 Issue、更新任务状态并把会议讨论直接拆解为结构化工单。',
    prompts: [
      '统计当前 Cycle 中未完成的高优先级 Issue，标出可能延期的阻塞项',
      '把刚才的架构讨论结论拆分成 4 个 Linear 任务，补全验收标准',
      '查找我名下处于 In Review 的工单，整理一份今日站会同步内容',
    ],
    tools: [
      ['search_issues', '按团队、状态、负责人检索 Linear Issue'],
      ['create_issue', '创建带有优先级和标签的新工单'],
      ['update_issue_state', '更新任务状态与流转记录'],
    ],
  },
  drive: {
    publisher: 'Google',
    capabilities: '云盘检索、文档读取',
    overview: '连接 Google Drive 后，Fox 能够跨目录检索云端文件、提取获授权文档中的关键数据，并在对话中随时引用上下文。',
    prompts: [
      '在 Google Drive 中查找最近更新的年度预算文件，提炼核心开销变化',
      '对比共享文件夹里两个版本的合作方案，列出条款差异清单',
      '整理「客户案例」目录下的资料，生成一份面向销售团队的速查摘要',
    ],
    tools: [
      ['search_files', '按名称、类型或修改时间检索云盘文件'],
      ['read_file_content', '读取已授权文档和表格的正文内容'],
    ],
  },
  figma: {
    publisher: 'Figma',
    capabilities: '设计稿读取、组件与设计变量',
    overview: '通过 Figma MCP，Fox 可以直接读取画布结构、组件层级与 Design Tokens，辅助前端还原样式与核对设计规范。',
    prompts: [
      '解析当前 Figma 页面的间距、圆角与配色变量，对照 Fox 主题 Token 列出差异',
      '读取设置页设计稿中的表单组件状态，生成一份前端验收清单',
      '检查这个 Figma 组件库的命名规范，列出与现有代码不一致的组件',
    ],
    tools: [
      ['get_file_nodes', '读取指定画布节点与布局属性'],
      ['get_variables', '提取设计变量、颜色与字体规范'],
    ],
  },
  'fox-office': {
    publisher: 'Fox 内置',
    capabilities: 'Word / Excel / PPT 本机读写',
    overview: 'Fox 内置的 Office 连接器，为文档、表格与演示文稿技能提供稳定的本地解析与编辑能力，无需上传至第三方云端。',
    prompts: [
      '读取当前目录下的季度汇报.docx，检查格式统一性并输出修订建议',
      '汇总 3 个部门的预算.xlsx，生成一张按科目归类的对比表',
      '根据项目方案大纲生成一份 10 页结构的演示文稿框架',
    ],
    tools: [
      ['word_read_write', '读取并保留样式修改 Word 文档'],
      ['excel_analyze', '读取工作表、公式并写回汇总结果'],
      ['ppt_compose', '创建与调整幻灯片页面结构'],
    ],
  },
  'p-tavily': {
    publisher: 'Tavily',
    capabilities: '实时联网搜索、正文提取',
    overview: '已整合进插件中心的网页搜索能力。启用后，Fox 通用助手与专家在需要实时信息时将自动通过 Tavily 检索网页并附带引用来源。',
    prompts: [
      '联网查询本周发布的最新开源模型基准评测结果，整理对比表',
      '检索 MCP 协议近期关于 OAuth 授权规范的更新要点并附上原文链接',
      '查找主流桌面 AI 工具的插件管理交互方案，总结共性设计',
    ],
    tools: [
      ['tavily_search', '按时效性与相关度执行联网网页搜索'],
      ['tavily_extract', '抓取指定网页正文并转为结构化摘要'],
    ],
  },
}

const categoryDefaults = {
  '即时通讯': {
    capabilities: '消息检索、对话路由、协作同步',
    prompts: (name) => [
      `汇总 ${name} 中今天与「发版」相关的关键消息，整理出待办事项和负责人`,
      `搜索 ${name} 里最近一周关于客户需求的讨论，提炼高频反馈`,
      `起草一条清晰简短的项目进度同步消息，准备发送到 ${name} 频道`,
    ],
    tools: [['search_messages', '按会话或关键词检索历史消息'], ['send_message', '在获授权的会话中回复或推送摘要']],
  },
  '文档与网盘': {
    capabilities: '文档检索、内容读取、知识整理',
    prompts: (name) => [
      `在 ${name} 中查找最新的产品需求文档，提炼核心目标与变更点`,
      `整理 ${name} 指定目录下的资料清单，按主题归类并生成目录摘要`,
      `读取 ${name} 中的会议记录，提取所有需要本周跟进的行动项`,
    ],
    tools: [['search_documents', '检索已授权空间内的文档与文件'], ['read_document', '读取文档正文、表格或结构化元数据']],
  },
  '项目与研发': {
    capabilities: '代码与工单查询、研发状态跟踪',
    prompts: (name) => [
      `查看 ${name} 中当前迭代未关闭的高优问题，梳理阻塞原因`,
      `总结 ${name} 最近的变更与部署状态，生成一份团队同步简报`,
      `围绕本次需求检索 ${name} 里的历史上下文，避免遗漏依赖项`,
    ],
    tools: [['list_items', '按项目、状态或标签列出研发条目'], ['get_detail', '获取工单、仓库或流水线的详细信息']],
  },
  '邮件与日程': {
    capabilities: '邮件检索、日程查询、回复起草',
    prompts: (name) => [
      `查看 ${name} 中本周的关键会议与往来邮件，整理每日重点清单`,
      `汇总最近与合作方在 ${name} 中的沟通要点，起草跟进回复`,
      `核对我明天的空闲时段，推荐两个适合安排 45 分钟评审的时间窗口`,
    ],
    tools: [['search_records', '按时间与主题检索邮件或日程事件'], ['draft_or_schedule', '起草邮件或整理会议议程建议']],
  },
  '设计与经营': {
    capabilities: '设计资产读取、客户与业务数据查询',
    prompts: (name) => [
      `从 ${name} 读取最新的项目资产与状态，整理成跨团队同步摘要`,
      `按优先级汇总 ${name} 中的近期客户记录，标出需要重点跟进的条目`,
      `核对 ${name} 里的核心指标与变更记录，生成一份周度简报`,
    ],
    tools: [['query_records', '查询获授权的业务实体或设计资产'], ['get_summary', '获取条目详情与近期变更日志']],
  },
  '数据与检索': {
    capabilities: '数据查询、实时检索、结构化提取',
    prompts: (name) => [
      `通过 ${name} 查询最新相关信息，整理成带来源引用的要点摘要`,
      `检查 ${name} 中的核心数据表或检索结果，输出关键趋势对比`,
      `围绕「行业动态」用 ${name} 执行定向检索，过滤重复内容后汇总`,
    ],
    tools: [['query_data', '执行受控的数据或网页检索查询'], ['extract_content', '将检索结果整理为结构化上下文']],
  },
  '网页搜索': {
    capabilities: '实时联网搜索、正文提取',
    prompts: (name) => [
      `使用 ${name} 检索近期关于 MCP 协议更新的官方公告并总结要点`,
      `通过 ${name} 对比主流桌面端 AI 应用的插件管理体验，附来源链接`,
      `查询最新版本的 API 变更说明，核对当前配置是否需要调整`,
    ],
    tools: [['web_search', '按关键词与时效性检索公开网页'], ['fetch_snippet', '提取高相关度网页摘要与引用链接']],
  },
  '内置工具': {
    capabilities: '工作区内置工具、受权限策略保护',
    prompts: (name) => [
      `使用 ${name} 协助检查当前项目目录结构与近期改动`,
      `结合当前工作区上下文，通过 ${name} 完成一次只读排查`,
      `按项目安全策略调用 ${name} 执行常规开发检查`,
    ],
    tools: [['execute_builtin', '在授权范围内执行内置工具能力']],
  },
  '办公技能': {
    capabilities: '文档规范、自动化生成与修订',
    prompts: (name) => [
      `使用 ${name} 技能把这份方案大纲整理为正式可交付文档`,
      `按团队排版规范检查现有文件并输出修改建议`,
      `根据数据摘要生成结构清晰的汇报内容`,
    ],
    tools: [],
  },
  '本地技能': {
    capabilities: 'SKILL.md 本地技能指令集',
    prompts: (name) => [
      `启用 ${name} 技能处理本次任务并按约定格式输出结论`,
      `用 ${name} 技能复核当前草稿，指出遗漏项与改进点`,
      `按 ${name} 的步骤清单拆解并执行接下来的工作`,
    ],
    tools: [],
  },
}

const DEFAULT_AVATARS = [
  'fox_big_smile.png', 'fox_calm.png', 'fox_coffee.png', 'fox_confident.png',
  'fox_love.png', 'fox_peace.png', 'fox_wave.png', 'fox_listen_music.png',
  'fox_excited.png', 'fox_determined.png', 'fox_laughing.png', 'fox_celebrate.png',
]

const COMPANION_PRESETS = [
  { label: '日常助理', name: '小狐助理', duty: '梳理每日待办、跟进日程安排与起草常用消息' },
  { label: '写作搭档', name: '文字搭档', duty: '长期打磨周报、产品文案与汇报材料，记住我的行文风格' },
  { label: '调研助手', name: '情报小狐', duty: '定期追踪竞品动态、核验一手资料并沉淀调研笔记' },
  { label: '技术顾问', name: '代码搭档', duty: '协助审查代码改动、排查偶发问题并维护架构决策记录' },
]

const SEED_COMPANIONS = [
  { id: 'cmp-default', name: 'Fox 助手', avatar: FOX, duty: '默认长期伙伴，熟悉我的常用项目与工作偏好。', pinned: true, isDefault: true, inWork: true, status: 'idle', time: '20:42', preview: '今天的两份原型已整理完毕，需要我汇总修改点吗？', model: '跟随默认模型', perm: '操作前询问', dir: 'D:\\Fox\\companions\\default' },
  { id: 'cmp-dev', name: '研发搭档', avatar: DEV_COMPANION, duty: '负责代码审查、分支巡检与回归验证。', pinned: true, inWork: true, status: 'action', time: '20:15', preview: '等待你确认：是否运行 desktop 单元测试并生成报告', model: 'DeepSeek · reasoner', perm: '操作前询问', dir: 'D:\\Fox\\companions\\dev-partner' },
  { id: 'cmp-writer', name: '文档助手', avatar: WRITER_COMPANION, duty: '长期整理汇报材料、会议纪要与发版说明。', pinned: true, inWork: true, status: 'running', time: '19:50', preview: '正在整理《v0.9 专家与伙伴改版说明.docx》…', model: '跟随默认模型', perm: '目录内自动', dir: 'D:\\Fox\\companions\\doc-writer' },
  { id: 'cmp-pm', name: '产品参谋', avatar: `${EXPERT_AVATAR_BASE}/Fox_ProductManager.png`, duty: '追踪用户反馈、迭代需求与竞品方案。', pinned: false, inWork: false, status: 'action', time: '18:30', preview: '等待你确认：将 6 条插件中心反馈同步到 Linear', model: '跟随默认模型', perm: '操作前询问', dir: 'D:\\Fox\\companions\\pm-advisor' },
  { id: 'cmp-data', name: '数据小狐', avatar: `${EXPERT_AVATAR_BASE}/Fox_DataAnalyst.png`, duty: '周度留存分析与异常波动归因。', pinned: false, inWork: false, status: 'unread', time: '昨天', preview: '已生成本周留存对比表，点击查看 3 项关键变化。', model: '跟随默认模型', perm: '只读', dir: 'D:\\Fox\\companions\\data-fox' },
  { id: 'cmp-archived', name: '招聘助理', avatar: `${ROOT}/avatars/defaults/fox_coffee.png`, duty: '协助筛选简历与安排面试题纲。', pinned: false, inWork: false, archived: true, status: 'idle', time: '上周', preview: '归档于 11月16日', model: '跟随默认模型', perm: '只读', dir: 'D:\\Fox\\companions\\recruiting' },
]

const SEED_INBOX = [
  { id: 'inb-1', companionId: 'cmp-dev', kind: 'approval', time: '12 分钟前', title: '等待你确认执行命令', desc: '研发搭档已完成插件与专家页面的静态检查，准备运行桌面端回归测试以验证改动。', op: '终端工具 · npm run test:desktop -- --filter=plugins', resolved: false },
  { id: 'inb-2', companionId: 'cmp-pm', kind: 'approval', time: '2 小时前', title: '等待你确认写入外部连接器', desc: '产品参谋整理了 6 条来自用户反馈的专家中心优化项，申请创建为 Linear Issue。', op: 'Linear 连接器 · create_issue (6 条需求卡片)', resolved: false },
  { id: 'inb-3', companionId: 'cmp-writer', kind: 'running', time: '进行中', title: '正在生成文档', desc: '正在基于今日修改记录整理《Fox 专家与伙伴改版说明.docx》，预计 20 秒内完成。', op: 'Fox Office · 已完成 3 / 4 章节', resolved: false },
  { id: 'inb-4', companionId: 'cmp-data', kind: 'done', time: '昨天 17:40', title: '已交付周度数据简报', desc: '输出了《11月第3周功能使用统计.xlsx》，其中插件详情页完读率提升 28%。', op: '已保存至 workspace/reports/', resolved: true },
]

const SEED_COMPANION_TASKS = [
  { id: 'tsk-1', companionId: 'cmp-default', name: '每日晨间简报', objective: '汇总昨日未完成待办、今日飞书日程与重点邮件，生成 3 分钟速览。', schedule: '每天 09:00', nextRun: '明天 09:00', lastResult: '已完成 · 今天 09:00', status: 'ok', enabled: true },
  { id: 'tsk-2', companionId: 'cmp-dev', name: '主分支构建与单测巡检', objective: '检查桌面端构建状态与核心单测结果，若出现回归立即在私聊推送日志摘要。', schedule: '每 4 小时', nextRun: '今天 22:00', lastResult: '已完成 · 今天 18:00', status: 'ok', enabled: true },
  { id: 'tsk-3', companionId: 'cmp-writer', name: '周五团队进展周报草稿', objective: '收集本周合并记录与关键决策文档，按团队模板起草周报并等待审阅。', schedule: '每周五 17:00', nextRun: '周五 17:00', lastResult: '已完成 · 上周五 17:02', status: 'ok', enabled: true },
  { id: 'tsk-4', companionId: 'cmp-data', name: '周度留存波动监测', objective: '拉取核心页面使用指标，对比上周留存变化并标记异常维度。', schedule: '每周一 10:00', nextRun: '已暂停', lastResult: '已暂停 · 11月18日', status: 'paused', enabled: false },
]

const TASK_SCHEDULES = ['每天 09:00', '每个工作日 18:00', '每周五 17:00', '每 4 小时']

const SEED_NOTIFICATIONS = [
  {
    id: 'ntf-1',
    date: '2026-11-24',
    dateLabel: '今天 · 2026-11-24',
    time: '20:15',
    kind: 'approval',
    unread: true,
    avatar: DEV_COMPANION,
    sourceLabel: '研发搭档',
    title: '研发搭档等待你确认执行命令',
    body: '已完成插件中心与专家页的静态检查，准备运行桌面端回归单测（npm run test:desktop -- --filter=plugins）以验证本次界面改动，确认后立即开始执行。',
    actionLabel: '立即处理',
    actionType: 'inbox',
  },
  {
    id: 'ntf-2',
    date: '2026-11-24',
    dateLabel: '今天 · 2026-11-24',
    time: '19:52',
    kind: 'completed',
    unread: true,
    avatar: WRITER_COMPANION,
    sourceLabel: '文档助手',
    title: '《Fox 专家与伙伴改版说明.docx》已生成',
    body: '已基于今日 4 轮原型迭代整理完成改版说明，包含伙伴侧栏新增入口、新建伙伴弹窗布局、专家装配能力精简与插件详情页结构，文件已保存至伙伴专属工作目录。',
    expandable: true,
    actionLabel: '打开私聊',
    actionType: 'companion',
    targetId: 'cmp-writer',
  },
  {
    id: 'ntf-3',
    date: '2026-11-24',
    dateLabel: '今天 · 2026-11-24',
    time: '09:00',
    kind: 'completed',
    unread: false,
    avatar: FOX,
    sourceLabel: 'Fox 助手',
    title: '定时任务「每日晨间简报」已送达',
    body: '为你汇总了昨日未完成待办 3 项、今日飞书日程 2 场及 4 封重点邮件摘要。',
    actionLabel: '查看简报',
    actionType: 'companion',
    targetId: 'cmp-default',
  },
  {
    id: 'ntf-4',
    date: '2026-11-23',
    dateLabel: '昨天 · 2026-11-23',
    time: '18:20',
    kind: 'failed',
    unread: true,
    icon: 'triangle-alert',
    tone: 'warn',
    sourceLabel: '插件服务',
    title: 'Tavily 网页搜索连接超时，已自动回退',
    body: '检测到 Tavily 搜索接口响应超时，系统已临时切换至「免 Key 搜索」保证对话不中断，建议检查 API Key 额度或网络代理配置。',
    expandable: true,
    actionLabel: '前往配置',
    actionType: 'settings-plugin',
  },
  {
    id: 'ntf-5',
    date: '2026-11-23',
    dateLabel: '昨天 · 2026-11-23',
    time: '17:40',
    kind: 'completed',
    unread: false,
    avatar: `${EXPERT_AVATAR_BASE}/Fox_DataAnalyst.png`,
    sourceLabel: '数据小狐',
    title: '周度功能使用统计简报已交付',
    body: '输出《11月第3周功能使用统计.xlsx》，插件详情页完读率环比提升 28%。',
    actionLabel: '打开私聊',
    actionType: 'companion',
    targetId: 'cmp-data',
  },
  {
    id: 'ntf-6',
    date: '2026-11-21',
    dateLabel: '2026-11-21',
    time: '14:10',
    kind: 'system',
    unread: false,
    icon: 'sparkles',
    tone: 'brand',
    sourceLabel: 'Fox 更新',
    title: 'Fox 0.9.4 新能力已就绪',
    body: '支持在伙伴侧边栏集中管理所有定时任务与待确认审批，插件详情页新增交互式示例指令卡与一键试用能力，欢迎体验并反馈使用建议。',
    expandable: true,
    actionLabel: '查看专家',
    actionType: 'experts',
  },
]

/* Shared demo state */
const storageKey = 'fox-plugin-expert-companion-v8'
const KNOWLEDGE_BASES = [
  { id: 'Fox 前端设计规范', desc: 'Paper / Graphite / Mist 主题变量与组件规范 · 18 份文档' },
  { id: 'Fox 架构设计文档', desc: 'Desktop Host、MCP 协议与 Runtime 架构文档 · 26 份文档' },
  { id: '用户访谈与反馈库', desc: '历次版本可用性测试记录与高频反馈 · 34 份文档' },
  { id: '项目申报参考库', desc: '历史申报模板、指标口径与评审意见 · 12 份文档' },
  { id: '行业标准资料库', desc: '数据安全、工业通信与 AI 治理公开规范 · 41 份文档' },
]

let demo = {
  shortlist: ['notion', 'slack', 'wechat', 'qq'],
  connectors: seedInstalledConnectors.map((item) => ({ ...item })),
  skills: [],
  customExperts: [],
  expertOverrides: {},
  companions: SEED_COMPANIONS.map((c) => ({ ...c })),
  inbox: SEED_INBOX.map((i) => ({ ...i })),
  tasks: SEED_COMPANION_TASKS.map((t) => ({ ...t })),
  notifications: SEED_NOTIFICATIONS.map((n) => ({ ...n })),
  enabledMap: {
    'fox-office': true,
    'inst-github': true,
    'inst-gmail': true,
    'inst-drive': true,
    'inst-feishu': true,
    'inst-figma': true,
    'inst-linear': false,
    'p-free': true,
    'p-tavily': true,
    'p-brave': false,
    'p-exa': false,
    'p-searxng': false,
    files: true,
    terminal: true,
    agents: true,
    'fox-office-word': true,
    'fox-office-excel': true,
    'fox-office-ppt': true,
    'meeting-minutes': true,
    'code-review': true,
    'prototype-audit': false,
  },
  scopes: {
    'fox-office-word': ['通用助手', '办公助手'],
    'fox-office-excel': ['办公助手'],
    'fox-office-ppt': ['办公助手'],
    'meeting-minutes': ['通用助手'],
    'code-review': ['研发助手'],
    'prototype-audit': [],
  },
}

try {
  const saved = localStorage.getItem(storageKey)
  if (saved) demo = { ...demo, ...JSON.parse(saved) }
} catch {}

function saveDemo() {
  try { localStorage.setItem(storageKey, JSON.stringify(demo)) } catch {}
}

function allExperts() {
  const base = (window.EXPERT_SEEDS || EXPERT_SEEDS).map((exp) => {
    const ov = demo.expertOverrides?.[exp.id]
    return ov ? { ...exp, ...ov } : exp
  })
  const custom = (demo.customExperts || []).map((exp) => {
    const ov = demo.expertOverrides?.[exp.id]
    return ov ? { ...exp, ...ov } : exp
  })
  return [...custom, ...base]
}

function findExpertById(id) {
  return allExperts().find((exp) => exp.id === id) || allExperts()[0]
}

function expertAvatarSrc(exp) {
  if (!exp?.avatar) return FOX
  if (exp.avatar.startsWith('http') || exp.avatar.startsWith('..') || exp.avatar.startsWith('/')) return exp.avatar
  return `${EXPERT_AVATAR_BASE}/${exp.avatar}`
}

/* Frame definitions (like 设置页美化/index.html) */
const BASE_FRAME_STATE = {
  setTab: 'connector', setQuery: '',
  dirKind: 'connector', dirGroup: '', dirStatus: '全部', dirQuery: '',
  expCat: 'all', expEnv: 'all', expAvail: 'all', expQuery: '', expDetailTab: 'all',
  detailId: null, detailKind: null, detailFrom: 'directory',
  expertId: 'fox-frontend',
  chatExpertId: null, chatDraft: '',
  compSection: 'inbox', compActiveId: 'cmp-default', compShowArchived: false, compChatDraft: '',
  createCompModal: null, // null | 'blank' | 'expert'
  createCompName: '', createCompDuty: '', createCompAvatar: 'fox_big_smile.png',
  createCompExpertId: 'fox-frontend', createCompExpertCat: 'all', createCompExpertQuery: '',
  createTaskModal: false,
  taskCompanionId: 'cmp-default', taskName: '', taskSchedule: '每天 09:00', taskObjective: '',
  notifFilter: 'all', notifExpanded: {}, notifSidebar: 'main', notifReturn: null,
  menuOpen: null,
}

const FRAMES = [
  {
    id: 'notifications',
    title: '通知中心（右侧主内容区域）',
    desc: '点击左侧导航栏铃铛后不再覆盖侧边栏，直接在右侧主内容区域展示按日期分组的简洁消息流，支持未读/需处理筛选、长文案展开、单条操作与右上角关闭返回。',
    height: 820,
    initial: { ...BASE_FRAME_STATE, view: 'notifications', notifSidebar: 'main', notifReturn: { view: 'experts' } },
  },
  {
    id: 'companion-create-task',
    title: '新建定时任务弹窗（简约居中风格）',
    desc: '与「新建伙伴」保持一致的简约无分割线弹窗样式：点选执行伙伴头像胶囊、填写任务名、快捷选择执行频率与任务目标，右下角取消/创建。',
    height: 760,
    initial: { ...BASE_FRAME_STATE, view: 'companions', compSection: 'tasks', createTaskModal: true },
  },
  {
    id: 'companions',
    title: '伙伴页面（侧边栏扩展：待我处理 / 定时任务 / 全部伙伴）',
    desc: '点击左侧「新建伙伴」弹出「新建伙伴 / 从专家创建」快捷菜单；下方补齐「待我处理」「定时任务」「全部伙伴」三项全局入口。',
    height: 840,
    initial: { ...BASE_FRAME_STATE, view: 'companions', compSection: 'inbox', menuOpen: 'comp-create' },
  },
  {
    id: 'companion-create-blank',
    title: '新建伙伴弹窗（参考 Grok Bot 居中布局）',
    desc: '顶部居中大头像预览 → 起名输入框 → 圆形头像选择器 → 快捷角色模板，精简无冗余文案，右下角提供取消与创建按钮。',
    height: 760,
    initial: { ...BASE_FRAME_STATE, view: 'companions', compSection: 'chat', compActiveId: 'cmp-default', createCompModal: 'blank' },
  },
  {
    id: 'companion-create-expert',
    title: '从专家创建弹窗（宽松卡片布局 · 头像垂直居中）',
    desc: '独立弹窗，无顶部切换栏与副标题；宽裕留白卡片仅展示垂直居中的圆形头像、专家名与下方用处。',
    height: 780,
    initial: { ...BASE_FRAME_STATE, view: 'companions', compSection: 'chat', compActiveId: 'cmp-default', createCompModal: 'expert' },
  },
  {
    id: 'experts',
    title: '专家中心（主页面入口）',
    desc: '适配真实桌面窗宽：左上角三个下拉筛选（状态 / 环境 / 分类），卡片沿用 Fox 原布局（圆形头像 + 底部胶囊标签）并按领域分组；点击进详情页，悬停浮现「召唤对话」。',
    height: 860,
    initial: { ...BASE_FRAME_STATE, view: 'experts' },
  },
  {
    id: 'expert-detail-frontend',
    title: '专家详情页（前端开发专家示例）',
    desc: '「已装配能力」仅展示已启用的能力，未装配项不再干扰视线；点击「管理装配」可在右侧抽屉按需开关。',
    height: 880,
    initial: { ...BASE_FRAME_STATE, view: 'expert-detail', expertId: 'fox-frontend' },
  },
  {
    id: 'directory',
    title: '插件中心（主页面入口）',
    desc: '从主页面侧边栏「插件」进入的目录页：使用主页面侧边栏（高亮「插件」），工具与插件独立分栏，顶部栏精简对齐且无导出。',
    height: 860,
    initial: { ...BASE_FRAME_STATE, view: 'directory' },
  },
  {
    id: 'detail-gmail',
    title: '插件详情页（Gmail 示例）',
    desc: '使用主页面侧边栏（高亮「插件」），顶部带可点击面包屑导航、交互式示例指令卡、应用与工具列表及基础信息表。',
    height: 860,
    initial: { ...BASE_FRAME_STATE, view: 'detail', detailId: 'gmail', detailKind: 'connector', detailFrom: 'directory' },
  },
  {
    id: 'settings-plugins',
    title: '设置 · 插件',
    desc: '设置页中的插件管理：保留原设置侧栏，按「连接器 / 插件 / 技能 / 内置工具」清晰拆分，网页搜索收归插件标签页。',
    height: 780,
    initial: { ...BASE_FRAME_STATE, view: 'settings', setTab: 'connector', detailFrom: 'settings' },
  },
]

const frameState = new Map()
FRAMES.forEach((f) => {
  frameState.set(f.id, {
    ...f.initial,
    expanded: new Set(),
    history: [{
      view: f.initial.view,
      detailId: f.initial.detailId,
      detailKind: f.initial.detailKind,
      detailFrom: f.initial.detailFrom,
      expertId: f.initial.expertId,
      chatExpertId: f.initial.chatExpertId,
      chatDraft: f.initial.chatDraft,
    }],
    historyIdx: 0,
  })
})

const escapeHtml = (value = '') => String(value).replace(/[&<>"']/g, (ch) => ({
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
})[ch])

function icon(name, cls = '') {
  const body = EXTRA_ICONS[name] || window.FOX_ICONS?.[name] || window.FOX_DIRECTORY_ICONS?.[name] || window.FOX_ICONS?.cable || ''
  return `<svg class="i ${cls}" viewBox="0 0 24 24" aria-hidden="true">${body}</svg>`
}

function mark(name, size = '') {
  const brand = window.FOX_BRANDS?.[name]
  const sizeCls = size ? ` ${size}` : ''
  if (!brand) return `<span class="mk${sizeCls}">${icon(name)}</span>`
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${brand.width} ${brand.height}" fill="${brand.color}" color="${brand.color}">${brand.body}</svg>`
  return `<span class="mk brand${sizeCls}"><img alt="" src="data:image/svg+xml,${encodeURIComponent(svg)}"></span>`
}

function itemMark(item) {
  return window.FOX_BRANDS?.[item.mark] || window.FOX_ICONS?.[item.mark] || window.FOX_DIRECTORY_ICONS?.[item.mark]
    ? item.mark
    : groupIcons[item.group] || 'cable'
}

function installedConnectors() {
  return [office, ...demo.connectors]
}

function allSkills() {
  return [...skills, ...demo.skills]
}

function directoryItems(kind) {
  if (kind === 'connector') return [office, ...demo.connectors, ...connectors]
  if (kind === 'skill') return allSkills()
  if (kind === 'tool') return tools
  return plugins
}

function findConfiguredEntry(item) {
  if (!item) return null
  if (item.id === office.id) return office
  return demo.connectors.find((entry) => entry.id === item.id || entry.fromCandidate === item.id) || null
}

function isEnabled(item, kind) {
  if (kind === 'connector') {
    const cfg = findConfiguredEntry(item)
    return cfg ? demo.enabledMap[cfg.id] !== false : false
  }
  return demo.enabledMap[item.id] !== false && Boolean(demo.enabledMap[item.id])
}

function findItemById(id, preferredKind = 'connector') {
  const installed = installedConnectors().find((c) => c.id === id || c.fromCandidate === id)
  if (preferredKind === 'connector' && installed) {
    const base = installed.fromCandidate ? connectors.find((c) => c.id === installed.fromCandidate) : null
    return { item: base ? { ...base, ...installed, id: base.id } : installed, kind: 'connector' }
  }
  for (const kind of [preferredKind, 'connector', 'plugin', 'skill', 'tool']) {
    const list = directoryItems(kind)
    const found = list.find((item) => item.id === id)
    if (found) return { item: found, kind }
  }
  return null
}

function getPublisher(item) {
  const metaKey = item.fromCandidate || item.id
  if (detailMeta[metaKey]?.publisher) return detailMeta[metaKey].publisher
  if (item.publisher) return item.publisher
  if (item.source) {
    try {
      const host = new URL(item.source).hostname.replace(/^www\./, '')
      if (host.includes('github.com')) return new URL(item.source).pathname.split('/').filter(Boolean)[0] || 'GitHub'
      if (host.includes('google.com')) return 'Google'
      if (host.includes('microsoft.com')) return 'Microsoft'
      if (host.includes('atlassian.com')) return 'Atlassian'
      if (host.includes('octop.cloud')) return 'Octop'
      return host
    } catch { return '官方 / 社区' }
  }
  return 'Fox 本地配置'
}

function getDetailContent(item, kind) {
  const metaKey = item.fromCandidate || item.id
  const custom = detailMeta[metaKey]
  const cat = categoryDefaults[item.group] || categoryDefaults['数据与检索']
  const methodInfo = item.method ? accessMethods[item.method] : null
  const publisher = getPublisher(item)
  const capabilities = custom?.capabilities || (item.method === 'bot' ? '消息收发、群聊与私聊路由' : cat.capabilities)
  const overview = custom?.overview || `将 ${item.name} 接入 Fox 后，助手可以直接在对话中${item.description.replace(/[。.]$/, '')}，无需在多个应用间反复切换。`
  const prompts = custom?.prompts || (typeof cat.prompts === 'function' ? cat.prompts(item.name) : [])
  const tools = custom?.tools || (kind === 'skill' ? [] : cat.tools || [])
  return { publisher, capabilities, overview, prompts, tools, methodInfo }
}

function statusPill(item, kind) {
  const cfg = kind === 'connector' ? findConfiguredEntry(item) : null
  const active = isEnabled(item, kind)
  const picked = kind === 'connector' && item.method && demo.shortlist.includes(item.id)
  if (kind === 'connector') {
    if (cfg && active) return '<span class="pill ok">已启用</span>'
    if (cfg && !active) return '<span class="pill">已停用</span>'
    if (picked) return '<span class="pill warn">已选候选</span>'
    if (item.method === 'bot') return '<span class="pill info">消息通道</span>'
    return ''
  }
  return active ? '<span class="pill ok">已启用</span>' : ''
}

/* ---------- Fox Sidebars: Settings Sidebar vs Main Workspace Sidebar ---------- */
function renderSettingsSidebar() {
  return `<aside class="side">
    <div class="mode-row"><button type="button" class="back" data-action="go-directory">${icon('arrow-left')}返回</button></div>
    <div class="scroll">${SETTINGS_NAV.map(([label, items]) => `
      <section class="sgroup">
        <span class="sglabel">${label}</span>
        ${items.map(([k, ic, title]) => k === 'plugins'
          ? `<button type="button" class="row on" data-action="go-settings-home" aria-current="page">${icon(ic)}<span class="lbl">${title}</span></button>`
          : `<a href="../index.html?frame=${k}" class="row">${icon(ic)}<span class="lbl">${title}</span></a>`).join('')}
      </section>`).join('')}
    </div>
    <label class="compact-nav">设置页面
      <select name="settings-page" class="sel">${SETTINGS_NAV.map(([label, items]) => `
        <optgroup label="${label}">${items.map(([k, , title]) => `<option value="${k}" ${k === 'plugins' ? 'selected' : ''}>${title}</option>`).join('')}</optgroup>`).join('')}
      </select>
    </label>
    <div class="foot">
      <img src="${ME}" alt=""><b>Hury</b><span class="chev">${icon('chevron-down')}</span>
      <div class="grow"></div>
      <button type="button" class="ab" data-action="toggle-theme" aria-label="切换浅色或深色模式" title="切换浅色 / 深色">${icon('sun')}</button>
      <a class="ab" href="../index.html?frame=plugins" aria-label="打开设置总览" title="设置总览">${icon('settings')}</a>
    </div>
  </aside>`
}

function renderMainSidebar(st) {
  const isChat = st.view === 'chat'
  const isExperts = st.view === 'experts' || st.view === 'expert-detail'
  const isPlugins = st.view === 'directory' || st.view === 'detail'
  const isNotif = st.view === 'notifications'
  const unreadNotif = (demo.notifications || []).filter((n) => n.unread).length
  return `<aside class="side side-main" aria-label="主侧边栏">
    <div class="main-mode-row">
      <div class="mode-tabs" role="tablist" aria-label="工作区模式">
        <button type="button" class="mode-tab on" role="tab" aria-selected="true" data-action="go-experts">${icon('square-kanban')}工作</button>
        <button type="button" class="mode-tab" role="tab" aria-selected="false" data-action="go-companions">${icon('cat-paw')}伙伴</button>
        <span class="mode-tab" role="tab" aria-selected="false">${icon('book-copy')}资料</span>
      </div>
      <div class="mode-actions">
        <button type="button" class="ab" data-action="noop" data-label="全局搜索" aria-label="搜索" title="搜索 (Ctrl K)">${icon('search')}</button>
        <button type="button" class="ab bell-btn ${isNotif ? 'on' : ''}" data-action="open-notifications" data-sidebar="main" aria-label="通知中心" aria-current="${isNotif ? 'page' : 'false'}" title="通知中心">${icon('bell')}${unreadNotif ? `<span class="bell-dot" aria-hidden="true"></span>` : ''}</button>
      </div>
    </div>

    <div class="main-primary">
      <button type="button" class="row ${isChat ? 'on' : ''}" data-action="go-new-chat" ${isChat ? 'aria-current="page"' : ''}>${icon('message-circle-plus')}<span class="lbl">新建对话</span><kbd>Ctrl N</kbd></button>
      <button type="button" class="row group-gap ${isExperts ? 'on' : ''}" data-action="go-experts" ${isExperts ? 'aria-current="page"' : ''}>${icon('users')}<span class="lbl">专家</span></button>
      <button type="button" class="row ${isPlugins ? 'on' : ''}" data-action="go-directory" ${isPlugins ? 'aria-current="page"' : ''}>${icon('blocks')}<span class="lbl">插件</span></button>
    </div>

    <div class="scroll">
      <section class="main-sec" aria-label="伙伴">
        <div class="main-sec-head">
          <span>伙伴</span>
          <span class="main-sec-acts"><span>${icon('ellipsis')}</span></span>
        </div>
        ${(demo.companions || []).filter((c) => !c.archived && c.inWork).map((c) => `<button type="button" class="comp-row" style="border:0;width:100%;background:transparent;text-align:left" data-action="open-companion-chat" data-value="${escapeHtml(c.id)}"><img src="${escapeHtml(c.avatar)}" alt=""><span>${escapeHtml(c.name)}</span></button>`).join('')}
      </section>

      <section class="main-sec" aria-label="项目">
        <div class="main-sec-head">
          <span>项目</span>
          <span class="main-sec-acts"><span>${icon('plus')}</span><span>${icon('ellipsis')}</span></span>
        </div>
        <div class="proj-row">${icon('folder-open')}<span>Fox</span></div>
        <div class="proj-children">
          <div class="conv-row"><span>优化插件中心与连接器交互</span></div>
          <div class="conv-row"><span>整理设置页原型与规范</span></div>
        </div>
      </section>

      <section class="main-sec" aria-label="对话">
        <div class="main-sec-head">
          <span>对话</span>
          <span class="main-sec-acts"><span>${icon('plus')}</span><span>${icon('ellipsis')}</span></span>
        </div>
        <div class="conv-row"><span>周报摘要与行动项整理</span></div>
        <div class="conv-row"><span>对比主流 MCP 接入方案</span></div>
        <div class="conv-row"><span>检查 Gmail 连接器授权流程</span></div>
      </section>
    </div>

    <label class="compact-nav">工作区导航
      <select name="main-page" class="sel">
        <option value="experts" ${isExperts ? 'selected' : ''}>专家</option>
        <option value="plugins" ${isPlugins ? 'selected' : ''}>插件</option>
        <option value="companions">伙伴</option>
        <option value="chat" ${isChat ? 'selected' : ''}>新对话</option>
        <option value="settings">设置 · 插件</option>
      </select>
    </label>

    <div class="foot">
      <img src="${ME}" alt=""><b>Hury</b><span class="chev">${icon('chevron-down')}</span>
      <div class="grow"></div>
      <button type="button" class="ab" data-action="toggle-theme" aria-label="切换浅色或深色模式" title="切换浅色 / 深色">${icon('sun')}</button>
      <button type="button" class="ab" data-action="go-settings-home" aria-label="打开设置" title="设置 · 插件">${icon('settings')}</button>
    </div>
  </aside>`
}

function renderCompanionSidebar(st) {
  const list = (demo.companions || []).filter((c) => !c.archived)
  const archived = (demo.companions || []).filter((c) => c.archived)
  const pinned = list.filter((c) => c.pinned)
  const ordinary = list.filter((c) => !c.pinned)
  const pendingCount = (demo.inbox || []).filter((item) => !item.resolved && item.kind === 'approval').length
  const activeTaskCount = (demo.tasks || []).filter((t) => t.enabled).length

  const renderStatusDot = (c) => {
    if (c.status === 'action') return '<span class="comp-status-dot action" title="等待你确认">!</span>'
    if (c.status === 'running') return '<span class="comp-status-dot running" title="正在后台工作"></span>'
    if (c.status === 'unread') return '<span class="comp-status-dot unread" title="未读消息"></span>'
    return ''
  }

  return `<aside class="side side-main" aria-label="伙伴侧边栏">
    <div class="main-mode-row">
      <div class="mode-tabs" role="tablist" aria-label="工作区模式">
        <button type="button" class="mode-tab" role="tab" aria-selected="false" data-action="go-experts">${icon('square-kanban')}工作</button>
        <button type="button" class="mode-tab on" role="tab" aria-selected="true" data-action="go-companions">${icon('cat-paw')}伙伴</button>
        <span class="mode-tab" role="tab" aria-selected="false">${icon('book-copy')}资料</span>
      </div>
      <div class="mode-actions">
        <button type="button" class="ab" data-action="noop" data-label="搜索伙伴消息" aria-label="搜索" title="搜索 (Ctrl K)">${icon('search')}</button>
        <button type="button" class="ab bell-btn ${st.view === 'notifications' ? 'on' : ''}" data-action="open-notifications" data-sidebar="companions" aria-label="通知中心" aria-current="${st.view === 'notifications' ? 'page' : 'false'}" title="通知中心">${icon('bell')}${(demo.notifications || []).some((n) => n.unread) ? `<span class="bell-dot" aria-hidden="true"></span>` : ''}</button>
      </div>
    </div>

    <div class="main-primary">
      <div class="comp-create-wrap">
        <button type="button" class="row ${st.menuOpen === 'comp-create' ? 'on' : ''}" data-action="toggle-comp-create-menu" aria-expanded="${st.menuOpen === 'comp-create'}">
          ${icon('user-plus')}<span class="lbl">新建伙伴</span>
        </button>
        ${st.menuOpen === 'comp-create' ? `<div class="comp-create-menu" role="menu">
          <button type="button" class="comp-create-mi active" data-action="open-create-companion" data-mode="blank">${icon('user-plus')}<span>新建伙伴</span></button>
          <button type="button" class="comp-create-mi" data-action="open-create-companion" data-mode="expert">${icon('users')}<span>从专家创建</span></button>
        </div>` : ''}
      </div>
      <button type="button" class="row group-gap ${st.compSection === 'inbox' ? 'on' : ''}" data-action="comp-section" data-value="inbox" ${st.compSection === 'inbox' ? 'aria-current="page"' : ''}>
        ${icon('bell')}<span class="lbl">待我处理</span>${pendingCount ? `<span class="comp-nav-badge">${pendingCount}</span>` : ''}
      </button>
      <button type="button" class="row ${st.compSection === 'tasks' ? 'on' : ''}" data-action="comp-section" data-value="tasks" ${st.compSection === 'tasks' ? 'aria-current="page"' : ''}>
        ${icon('calendar-clock')}<span class="lbl">定时任务</span>${activeTaskCount ? `<span class="toolbar-count">${activeTaskCount}</span>` : ''}
      </button>
      <button type="button" class="row ${st.compSection === 'overview' ? 'on' : ''}" data-action="comp-section" data-value="overview" ${st.compSection === 'overview' ? 'aria-current="page"' : ''}>
        ${icon('users')}<span class="lbl">全部伙伴</span>
      </button>
    </div>

    <div class="scroll">
      ${st.compShowArchived ? `
        <div class="main-sec-head" style="margin-bottom:4px">
          <button type="button" class="back" style="padding-left:0;height:24px" data-action="toggle-comp-archived">${icon('arrow-left')}已归档 (${archived.length})</button>
        </div>
        ${archived.map((c) => `<button type="button" class="comp-chat-row" data-action="restore-companion" data-value="${escapeHtml(c.id)}" title="点击恢复该伙伴">
          <span class="comp-chat-av"><img src="${escapeHtml(c.avatar)}" alt="" style="opacity:.6"></span>
          <span class="comp-chat-copy">
            <span class="comp-chat-top"><b>${escapeHtml(c.name)}</b><time>恢复</time></span>
            <span class="comp-chat-prev">${escapeHtml(c.preview)}</span>
          </span>
        </button>`).join('')}
      ` : `
        <div class="comp-tile-grid" aria-label="置顶伙伴">
          ${pinned.map((c) => `<button type="button" class="comp-tile ${st.compSection === 'chat' && st.compActiveId === c.id ? 'on' : ''}" data-action="open-companion-chat" data-value="${escapeHtml(c.id)}">
            <span class="comp-tile-av"><img src="${escapeHtml(c.avatar)}" alt="">${renderStatusDot(c)}</span>
            <span class="comp-tile-name">${escapeHtml(c.name)}</span>
          </button>`).join('')}
        </div>

        <section class="main-sec" aria-label="伙伴私聊">
          <div class="main-sec-head"><span>全部私聊</span><span>${ordinary.length}</span></div>
          ${ordinary.map((c) => `<button type="button" class="comp-chat-row ${st.compSection === 'chat' && st.compActiveId === c.id ? 'on' : ''}" data-action="open-companion-chat" data-value="${escapeHtml(c.id)}">
            <span class="comp-chat-av"><img src="${escapeHtml(c.avatar)}" alt="">${renderStatusDot(c)}</span>
            <span class="comp-chat-copy">
              <span class="comp-chat-top"><b>${escapeHtml(c.name)}</b><time>${escapeHtml(c.time)}</time></span>
              <span class="comp-chat-prev ${c.status === 'action' ? 'wait' : ''}">${escapeHtml(c.preview)}</span>
            </span>
          </button>`).join('')}
        </section>
      `}
    </div>

    ${archived.length && !st.compShowArchived ? `<div class="comp-archive-row">
      <button type="button" class="row" data-action="toggle-comp-archived">${icon('archive')}<span class="lbl">已归档</span><span class="toolbar-count">${archived.length}</span></button>
    </div>` : ''}

    <label class="compact-nav">伙伴导航
      <select name="comp-page" class="sel">
        <option value="inbox" ${st.compSection === 'inbox' ? 'selected' : ''}>待我处理</option>
        <option value="tasks" ${st.compSection === 'tasks' ? 'selected' : ''}>定时任务</option>
        <option value="overview" ${st.compSection === 'overview' ? 'selected' : ''}>全部伙伴</option>
        <option value="chat" ${st.compSection === 'chat' ? 'selected' : ''}>伙伴私聊</option>
      </select>
    </label>

    <div class="foot">
      <img src="${ME}" alt=""><b>Hury</b><span class="chev">${icon('chevron-down')}</span>
      <div class="grow"></div>
      <button type="button" class="ab" data-action="toggle-theme" aria-label="切换浅色或深色模式" title="切换浅色 / 深色">${icon('sun')}</button>
      <button type="button" class="ab" data-action="go-settings-home" aria-label="打开设置" title="设置 · 插件">${icon('settings')}</button>
    </div>
  </aside>`
}

function renderTitlebar(st) {
  const canBack = st.historyIdx > 0
  const canFwd = st.historyIdx < st.history.length - 1
  return `<header class="titlebar">
    <div class="tb-left">
      <img src="${FOX}" alt="">
      <span>${icon('menu')}</span>
      <span>${icon('panel-left')}</span>
      <button type="button" class="${canBack ? '' : 'off'}" data-action="nav-back" ${canBack ? '' : 'disabled'} aria-label="后退">${icon('arrow-left')}</button>
      <button type="button" class="${canFwd ? '' : 'off'}" data-action="nav-forward" ${canFwd ? '' : 'disabled'} aria-label="前进">${icon('arrow-right')}</button>
    </div>
    <div class="wctl" aria-hidden="true"><span>${icon('minus')}</span><span>${icon('square')}</span><span>${icon('x')}</span></div>
  </header>`
}

/* ---------- View 1: Settings -> 插件 (Codex-style clean list, Tools & Plugins separated) ---------- */
function renderSettingsView(frameId, st) {
  const connList = installedConnectors()
  const skillList = allSkills()
  const tabDefs = [
    ['connector', '连接器', connList.length],
    ['plugin', '插件', plugins.length],
    ['skill', '技能', skillList.length],
    ['tool', '内置工具', tools.length],
  ]

  const currentKind = st.setTab
  const rawList = currentKind === 'connector'
    ? connList
    : currentKind === 'skill'
      ? skillList
      : currentKind === 'tool'
        ? tools
        : plugins
  const query = st.setQuery.trim().toLowerCase()
  const filtered = rawList.filter((item) => {
    if (!query) return true
    return [item.name, item.sub, item.group, item.description].join(' ').toLowerCase().includes(query)
  })

  const searchPlaceholder = currentKind === 'connector'
    ? '搜索连接器'
    : currentKind === 'skill'
      ? '搜索技能'
      : currentKind === 'tool'
        ? '搜索内置工具'
        : '搜索插件'

  const rowsHtml = filtered.length
    ? `<div class="set-list">${filtered.map((item) => {
        const on = isEnabled(item, currentKind)
        const targetId = item.fromCandidate || item.id
        const scopeText = currentKind === 'skill' ? (demo.scopes[item.id]?.length ? demo.scopes[item.id].join(' / ') : (item.scopeLabel || '未分配')) : ''
        return `<div class="set-row ${on ? '' : 'is-off'}">
          <button type="button" class="set-row-main" data-action="open-detail" data-value="${escapeHtml(targetId)}" data-kind="${currentKind}">
            ${mark(itemMark(item))}
            <span class="set-row-copy">
              <span class="set-row-head">
                <span class="set-row-name">${escapeHtml(item.name)}</span>
                ${item.sub ? `<span class="set-row-sub">${escapeHtml(item.sub)}</span>` : ''}
              </span>
              <span class="set-row-desc">${escapeHtml(item.description)}</span>
            </span>
          </button>
          <span class="set-row-ctrl">
            ${currentKind === 'skill' ? `<span class="set-scope">${escapeHtml(scopeText)}</span>` : ''}
            <button type="button" class="sw ${on ? 'on' : ''}" role="switch" aria-checked="${on}"
              aria-label="${on ? '停用' : '启用'} ${escapeHtml(item.name)}"
              data-action="toggle-enabled" data-value="${escapeHtml(item.id)}" data-kind="${currentKind}"><i></i></button>
            ${currentKind !== 'skill' ? `<button type="button" class="set-chev" data-action="open-detail" data-value="${escapeHtml(targetId)}" data-kind="${currentKind}" aria-label="查看 ${escapeHtml(item.name)} 详情">${icon('chevron-right')}</button>` : ''}
          </span>
        </div>`
      }).join('')}</div>`
    : `<div class="empty">${icon('search')}<h2>没有匹配的条目</h2><p>可以清空搜索词，或去插件中心发现更多扩展。</p>
        <button type="button" class="btn" data-action="go-directory">浏览目录</button></div>`

  return `<div class="sp">
    <header class="sp-head">
      <div>
        <h1>插件</h1>
        <p>管理连接器、插件、技能和内置工具。</p>
      </div>
      <div class="acts">
        <button type="button" class="btn lg" data-action="go-directory-kind" data-value="${currentKind}">${currentKind === 'connector' ? '连接器目录' : '浏览目录'}</button>
        <button type="button" class="btn primary lg" data-action="toggle-add-menu" aria-expanded="${st.menuOpen === 'add'}">添加${icon('chevron-down')}</button>
        ${st.menuOpen === 'add' ? `<div class="menu" role="menu">
          <button type="button" class="mi" data-action="open-add-connector">${icon('cable')}添加连接器</button>
          <button type="button" class="mi" data-action="open-add-skill">${icon('puzzle')}导入技能</button>
          <div class="ms"></div>
          <button type="button" class="mi" data-action="open-policy">${icon('shield-check')}插件策略与 Hook</button>
        </div>` : ''}
      </div>
    </header>

    <div class="set-bar">
      <div class="tab-pills" role="tablist" aria-label="扩展类型">
        ${tabDefs.map(([key, label, count]) => `<button type="button" class="tab-pill" role="tab"
          data-action="set-tab" data-value="${key}" aria-selected="${key === currentKind}">${label}<span>${count}</span></button>`).join('')}
      </div>
      <label class="inp search-box set-search">
        ${icon('search')}
        <input type="search" data-input="set-query" value="${escapeHtml(st.setQuery)}" placeholder="${searchPlaceholder}" aria-label="${searchPlaceholder}">
      </label>
    </div>

    ${rowsHtml}

    <div class="set-foot-hint">
      <span>网页搜索已整合进「插件」标签页统一启停，内置工具与插件分开管理。</span>
      <button type="button" class="btn ghost" data-action="open-policy">${icon('shield-check')}插件策略</button>
    </div>
  </div>`
}

/* ---------- View 2: Directory (主页面 -> 插件, Main Sidebar, no Export, neat top bar) ---------- */
function renderDirectoryView(frameId, st) {
  const settings = directory[st.dirKind]
  const items = directoryItems(st.dirKind)
  const groups = [...new Set(items.map((item) => item.group))]
  const terms = st.dirQuery.trim().toLowerCase().split(/\s+/).filter(Boolean)

  const filtered = items.filter((item) => {
    if (st.dirGroup && item.group !== st.dirGroup) return false
    if (st.dirStatus === '已配置' && item.method) return false
    if (st.dirStatus === '待筛选' && (!item.method || demo.shortlist.includes(item.id))) return false
    if (st.dirStatus === '已选候选' && !demo.shortlist.includes(item.id)) return false
    if (st.dirStatus === '已启用' && !isEnabled(item, st.dirKind)) return false
    if (st.dirStatus === '待启用' && isEnabled(item, st.dirKind)) return false
    if (st.dirStatus === '已停用' && isEnabled(item, st.dirKind)) return false
    const haystack = [item.name, item.id, item.group, item.description, item.aliases, accessMethods[item.method]?.join(' ')].join(' ').toLowerCase()
    return terms.every((term) => haystack.includes(term))
  })

  const overview = st.dirKind === 'connector' && !st.dirGroup && st.dirStatus === '全部' && !st.dirQuery.trim()
  const featured = overview ? featuredConnectors.map((id) => connectors.find((item) => item.id === id)).filter(Boolean) : []
  const sectionItems = overview ? filtered.filter((item) => !featuredConnectors.includes(item.id)) : filtered

  const renderCard = (item) => {
    const candidate = Boolean(item.method)
    const picked = candidate && demo.shortlist.includes(item.id)
    const targetId = item.fromCandidate || item.id
    const quick = st.dirKind === 'connector' && candidate && !findConfiguredEntry(item)
      ? `<button type="button" class="btn ghost icon ${picked ? 'saved' : ''}" data-action="quick-shortlist" data-value="${item.id}" aria-label="${picked ? '移出候选' : '加入候选'} ${escapeHtml(item.name)}" title="${picked ? '移出候选' : '加入候选'}">${icon(picked ? 'check' : 'plus')}</button>`
      : ''
    return `<li class="entry">
      <button class="entry-main" type="button" data-action="open-detail" data-value="${escapeHtml(targetId)}" data-kind="${st.dirKind}">
        ${mark(itemMark(item))}
        <span class="entry-copy">
          <span class="entry-title"><strong>${escapeHtml(item.name)}</strong>${statusPill(item, st.dirKind)}</span>
          <span class="entry-description">${escapeHtml(item.description)}</span>
        </span>
      </button>
      <span class="entry-tail">
        ${quick}
        <span class="entry-chev" aria-hidden="true">${icon('chevron-right')}</span>
      </span>
    </li>`
  }

  const renderSection = (title, list, collapsible = false) => {
    const canExpand = collapsible && list.length > 6
    const expanded = st.expanded.has(title)
    const visible = canExpand && !expanded ? list.slice(0, 6) : list
    return `<section class="grp">
      <div class="grp-head">
        <div class="grp-head-left"><h2>${escapeHtml(title)}</h2><span class="grp-count">${list.length} 项</span></div>
        ${canExpand ? `<button type="button" class="more-btn" data-action="expand" data-value="${escapeHtml(title)}">${expanded ? '收起' : `查看全部 ${list.length} 项`}${icon(expanded ? 'chevron-down' : 'chevron-right')}</button>` : ''}
      </div>
      <div class="grp-body"><ul class="entry-grid">${visible.map(renderCard).join('')}</ul></div>
    </section>`
  }

  const sections = groups.flatMap((group) => {
    const matches = sectionItems.filter((item) => item.group === group)
    return matches.length ? [renderSection(group, matches, overview)] : []
  })
  if (featured.length) sections.unshift(renderSection('优先推荐', featured))

  const addAction = st.dirKind === 'connector'
    ? 'open-add-connector'
    : st.dirKind === 'skill'
      ? 'open-add-skill'
      : st.dirKind === 'tool'
        ? 'open-policy'
        : 'go-settings-plugins-search'
  const addIcon = st.dirKind === 'tool' ? 'shield-check' : st.dirKind === 'plugin' ? 'settings' : 'plus'

  return `<div class="sp sp-wide">
    <header class="sp-head">
      <div>
        <h1>${st.dirKind === 'connector' ? '连接器目录' : '插件'}</h1>
        <p>${st.dirKind === 'connector' ? `共 ${connectors.length} 项候选。搜索应用名称，或按分类浏览。` : '浏览并管理扩展插件、技能与内置工具，点击任意条目查看详情。'}</p>
      </div>
      <div class="acts">
        ${st.dirKind === 'connector' ? `<a class="btn lg" href="连接器用户授权筛选.md">${icon('file-text')}首批接入名单</a>` : ''}
        <button type="button" class="btn primary lg" data-action="${addAction}">
          ${icon(addIcon)}${settings.action}
        </button>
      </div>
    </header>

    <div class="toolbar">
      <div class="toolbar-row">
        <div class="seg" role="tablist" aria-label="目录类型">
          ${Object.entries(directory).map(([key, value]) => `<button type="button" class="seg-btn" role="tab"
            data-action="dir-kind" data-value="${key}" aria-selected="${key === st.dirKind}">${icon(value.icon)}${value.name}</button>`).join('')}
        </div>
        <div class="toolbar-right">
          ${groups.length > 1 ? `<select class="sel" data-select="dir-group" aria-label="按分类筛选">
            <option value="">全部分类</option>
            ${groups.map((g) => `<option value="${escapeHtml(g)}" ${g === st.dirGroup ? 'selected' : ''}>${escapeHtml(g)}</option>`).join('')}
          </select>` : ''}
          <select class="sel" data-select="dir-status" aria-label="按状态筛选">
            ${settings.statuses.map((status) => {
              let label = status === '全部' ? '全部状态' : status
              if (status === '已选候选' && demo.shortlist.length) label = `已选候选 (${demo.shortlist.length})`
              if (status === '已配置') label = `已配置 (${installedConnectors().length})`
              return `<option value="${escapeHtml(status)}" ${status === st.dirStatus ? 'selected' : ''}>${escapeHtml(label)}</option>`
            }).join('')}
          </select>
          <label class="inp search-box dir-search">
            ${icon('search')}
            <input type="search" data-input="dir-query" value="${escapeHtml(st.dirQuery)}" placeholder="搜索${settings.name}" aria-label="搜索${settings.name}">
            <kbd aria-hidden="true">Ctrl K</kbd>
          </label>
        </div>
      </div>
    </div>

    ${sections.join('') || `<div class="empty">${icon('search')}<h2>没有匹配的${settings.name}</h2><p>可清除筛选后重新浏览。</p>
      <button type="button" class="btn" data-action="dir-reset">清除筛选</button></div>`}

    <footer class="directory-footer">
      <span>${overview ? `${connectors.length} 项候选连接器 · ${installedConnectors().length} 项已配置` : `共 ${filtered.length} 项`}</span>
      ${st.dirKind === 'connector' ? `<a class="doc-link" href="连接器接入评估.md">${icon('file-text')}开发接入评估</a>` : ''}
      <a class="doc-link" href="调研与接入.md">${icon('file-text')}调研与接入说明</a>
    </footer>
  </div>`
}

/* ---------- View 3: Detail Page (Codex-inspired, with Breadcrumb) ---------- */
function renderDetailView(frameId, st) {
  const found = findItemById(st.detailId, st.detailKind || 'connector')
  if (!found) return renderSettingsView(frameId, st)

  const { item, kind } = found
  const { publisher, capabilities, overview, prompts, tools: detailTools, methodInfo } = getDetailContent(item, kind)
  const cfg = kind === 'connector' ? findConfiguredEntry(item) : null
  const active = isEnabled(item, kind)
  const candidate = kind === 'connector' && Boolean(item.method)
  const picked = candidate && demo.shortlist.includes(item.id)

  let secondaryBtn = ''
  let primaryBtn = ''
  if (kind === 'connector') {
    if (cfg) {
      secondaryBtn = `<button type="button" class="btn lg" data-action="open-manage" data-value="${escapeHtml(cfg.id)}" data-kind="connector">${icon('settings')}管理</button>`
      primaryBtn = `<button type="button" class="btn primary lg" data-action="try-item" data-value="${escapeHtml(item.name)}">${icon('sparkles')}立即试用</button>`
    } else {
      secondaryBtn = `<button type="button" class="btn lg ${picked ? 'saved' : ''}" data-action="quick-shortlist" data-value="${escapeHtml(item.id)}">${icon(picked ? 'check' : 'plus')}${picked ? '已选候选' : '加入候选'}</button>`
      primaryBtn = `<button type="button" class="btn primary lg" data-action="open-connect" data-value="${escapeHtml(item.id)}">${icon('cable')}配置接入</button>`
    }
  } else if (kind === 'skill') {
    secondaryBtn = `<button type="button" class="btn lg" data-action="toggle-enabled" data-value="${escapeHtml(item.id)}" data-kind="skill">${icon('settings')}${active ? '停用技能' : '启用技能'}</button>`
    primaryBtn = `<button type="button" class="btn primary lg" data-action="try-item" data-value="${escapeHtml(item.name)}">${icon('sparkles')}立即试用</button>`
  } else if (kind === 'tool') {
    secondaryBtn = `<button type="button" class="btn lg" data-action="open-manage" data-value="${escapeHtml(item.id)}" data-kind="tool">${icon('shield-check')}运行边界</button>`
    primaryBtn = `<button type="button" class="btn primary lg" data-action="toggle-enabled" data-value="${escapeHtml(item.id)}" data-kind="tool">${icon('settings')}${active ? '停用工具' : '启用工具'}</button>`
  } else {
    secondaryBtn = `<button type="button" class="btn lg" data-action="open-manage" data-value="${escapeHtml(item.id)}" data-kind="plugin">${icon('settings')}配置</button>`
    primaryBtn = `<button type="button" class="btn primary lg" data-action="try-item" data-value="${escapeHtml(item.name)}">${icon('sparkles')}立即试用</button>`
  }

  const menuItems = []
  if (item.source) menuItems.push(`<a class="mi" href="${item.source}" target="_blank" rel="noopener">${icon('external-link')}查看官方文档</a>`)
  menuItems.push(`<button type="button" class="mi" data-action="copy-info" data-value="${escapeHtml(item.name)}">${icon('link')}复制接入信息</button>`)
  if (cfg) menuItems.push(`<button type="button" class="mi" data-action="test-connection">${icon('zap')}测试连接</button>`)
  if (cfg && cfg !== office) {
    menuItems.push('<div class="ms"></div>')
    menuItems.push(`<button type="button" class="mi danger" data-action="remove-connector" data-value="${escapeHtml(cfg.id)}">${icon('trash-2')}移除配置</button>`)
  }

  let capabilitySection = ''
  if (kind === 'skill') {
    const scope = demo.scopes[item.id] || []
    capabilitySection = `
      <section class="detail-sec">
        <div class="detail-sec-head"><h2>依赖应用</h2><span>1</span></div>
        <div class="app-entry">
          ${mark(itemMark(item))}
          <div class="tx">
            <b>${escapeHtml(item.tool || '本地技能包')}</b>
            <small>${item.tool ? '由 Fox Office 内置工具提供底层读写支持' : `来源文件：${escapeHtml(item.file || 'SKILL.md')}`}</small>
          </div>
          <span class="stat"><i class="dot ok"></i><b>已就绪</b></span>
        </div>
      </section>
      <section class="detail-sec">
        <div class="detail-sec-head"><h2>适用助手范围</h2><span>${scope.length} / 3 已启用</span></div>
        <div class="scope-card">
          ${['通用助手', '办公助手', '研发助手'].map((agent) => {
            const on = scope.includes(agent)
            return `<div class="srow">
              <div class="st"><b>${agent}</b><small>${on ? '该助手可在对话中自动调用此技能' : '未授权该助手使用此技能'}</small></div>
              <button type="button" class="sw ${on ? 'on' : ''}" role="switch" aria-checked="${on}"
                data-action="toggle-skill-scope" data-item="${escapeHtml(item.id)}" data-agent="${agent}"><i></i></button>
            </div>`
          }).join('')}
        </div>
      </section>`
  } else {
    const appSubtitle = cfg?.target ? `${cfg.transport} · ${cfg.target}` : item.description
    const stateLabel = active
      ? (kind === 'connector' ? '已连接' : '已启用')
      : (cfg || kind === 'tool' ? '已停用' : picked ? '待接入' : '未配置')
    const secTitle = kind === 'tool' ? '内置能力' : '应用'
    capabilitySection = `
      <section class="detail-sec">
        <div class="detail-sec-head"><h2>${secTitle}</h2><span>1</span></div>
        <div class="app-entry">
          ${mark(itemMark(item))}
          <div class="tx">
            <b>${escapeHtml(item.name)}</b>
            <small>${escapeHtml(appSubtitle)}</small>
          </div>
          <span class="stat"><i class="dot ${active ? 'ok' : picked ? 'warn' : 'empty'}"></i><b>${stateLabel}</b></span>
        </div>
        ${detailTools.length ? `<div class="tool-list">
          ${detailTools.map(([toolName, toolDesc]) => `<div class="tool-row">
            <div class="tx"><code>${escapeHtml(toolName)}</code><small>${escapeHtml(toolDesc)}</small></div>
            <span class="pill">工具</span>
          </div>`).join('')}
        </div>` : ''}
      </section>`
  }

  const noteBanner = candidate && (item.note || methodInfo?.[2])
    ? `<div class="banner">
        <span>${icon('info')}</span>
        <div class="tx"><b>接入前核对</b><small>${escapeHtml(item.note || methodInfo[2])}</small></div>
        ${item.source ? `<a class="btn" href="${item.source}" target="_blank" rel="noopener">${icon('external-link')}文档</a>` : ''}
      </div>`
    : ''

  const accessText = methodInfo
    ? `${methodInfo[0]} · ${methodInfo[1]}`
    : (cfg?.transport || item.transport || (kind === 'skill' ? 'SKILL.md 技能规范' : kind === 'tool' ? 'Fox 内置核心工具' : '网页搜索插件'))
  const statusText = active ? '已启用' : (cfg || kind === 'tool' || kind === 'skill' ? '已停用' : picked ? '已加入候选' : '未配置')

  const factRows = [
    ['功能', escapeHtml(capabilities)],
    ['开发者', escapeHtml(publisher)],
    ['分类', escapeHtml(item.group)],
    ['接入方式', escapeHtml(accessText)],
    ['状态', `<span class="stat"><i class="dot ${active ? 'ok' : picked ? 'warn' : 'empty'}"></i><b>${statusText}</b></span>`],
  ]
  if (cfg?.target) factRows.push(['端点 / 命令', `<code>${escapeHtml(cfg.target)}</code>`])
  if (item.source) factRows.push(['参考来源', `<a class="doc-link" href="${item.source}" target="_blank" rel="noopener">${escapeHtml(item.source.replace(/^https?:\/\//, ''))}${icon('external-link')}</a>`])

  const kindLabel = directory[kind]?.name || '插件'
  return `<div class="sp sp-detail">
    <nav class="breadcrumb" aria-label="面包屑导航">
      ${st.detailFrom === 'settings'
        ? `<button type="button" class="crumb-btn" data-action="go-settings-home">设置 · 插件</button>`
        : `<button type="button" class="crumb-btn" data-action="go-directory">插件</button>`}
      <span class="crumb-sep" aria-hidden="true">${icon('chevron-right')}</span>
      <button type="button" class="crumb-btn" data-action="go-directory-kind" data-value="${escapeHtml(kind)}">${escapeHtml(kindLabel)}</button>
      <span class="crumb-sep" aria-hidden="true">${icon('chevron-right')}</span>
      <span class="crumb-current" aria-current="page">${escapeHtml(item.name)}</span>
    </nav>

    <header class="detail-hero">
      <div class="detail-id">
        ${mark(itemMark(item), 'xl')}
        <div class="detail-title-wrap">
          <div class="detail-title-row">
            <h1>${escapeHtml(item.name)}</h1>
            ${statusPill(item, kind)}
          </div>
          <p class="detail-sub">由 ${escapeHtml(publisher)} 发布 · ${escapeHtml(item.description.replace(/[。.]$/, ''))}</p>
        </div>
      </div>
      <div class="detail-acts">
        <button type="button" class="btn ghost icon lg" data-action="toggle-detail-menu" aria-expanded="${st.menuOpen === 'detail'}" aria-label="更多操作">${icon('ellipsis')}</button>
        ${st.menuOpen === 'detail' ? `<div class="menu" role="menu">${menuItems.join('')}</div>` : ''}
        ${secondaryBtn}
        ${primaryBtn}
      </div>
    </header>

    <section class="detail-showcase" aria-label="示例指令">
      ${prompts.map((text, idx) => `<button type="button" class="prompt-card" data-action="run-prompt" data-text="${escapeHtml(text)}">
        <span class="prompt-text"><span class="prompt-mention">${mark(itemMark(item), 'sm')}${escapeHtml(item.name)}</span>${escapeHtml(text)}</span>
        <span class="prompt-go" aria-hidden="true">${icon('arrow-right')}</span>
      </button>`).join('')}
    </section>

    <p class="detail-overview">${escapeHtml(overview)}</p>
    ${noteBanner}
    ${capabilitySection}

    <section class="detail-sec">
      <div class="detail-sec-head"><h2>信息</h2></div>
      <dl class="facts">${factRows.map(([k, v]) => `<div><dt>${k}</dt><dd>${v}</dd></div>`).join('')}</dl>
    </section>
  </div>`
}

/* ---------- View 4: Experts Center (主页面 -> 专家, 3 dropdown filters, circular avatar + bottom capsule tags) ---------- */
function renderExpertsView(frameId, st) {
  const list = allExperts()
  const terms = (st.expQuery || '').trim().toLowerCase().split(/\s+/).filter(Boolean)

  const filtered = list.filter((exp) => {
    const available = exp.available !== false
    if (st.expCat !== 'all' && exp.category !== st.expCat) return false
    if (st.expEnv === 'local' && exp.env !== 'local') return false
    if (st.expEnv === 'knowledge' && exp.env !== 'knowledge') return false
    if (st.expAvail === 'available' && !available) return false
    if (st.expAvail === 'unavailable' && available) return false
    if (!terms.length) return true
    const haystack = [exp.name, exp.description, EXPERT_CATEGORY_MAP[exp.category], exp.source, ...(exp.skills || []), ...(exp.connectors || [])].join(' ').toLowerCase()
    return terms.every((term) => haystack.includes(term))
  })

  const isOverview = st.expCat === 'all' && st.expEnv === 'all' && st.expAvail === 'all' && !terms.length
  const featured = isOverview ? FEATURED_EXPERT_IDS.map((id) => list.find((e) => e.id === id)).filter(Boolean) : []
  const remaining = isOverview ? filtered.filter((e) => !FEATURED_EXPERT_IDS.includes(e.id)) : filtered

  const renderExpertCard = (exp) => {
    const tags = []
    if (exp.tools?.length) tags.push(`<span class="exp-tag">工具 ${exp.tools.length}</span>`)
    if (exp.knowledge?.length) tags.push(`<span class="exp-tag">资料库 ${exp.knowledge.length}</span>`)
    if (exp.connectors?.length) tags.push(`<span class="exp-tag">连接器 ${exp.connectors.length}</span>`)
    if (exp.skills?.length) tags.push(`<span class="exp-tag">技能 ${exp.skills.length}</span>`)
    if (!tags.length) tags.push('<span class="exp-tag">通用专家</span>')
    const subTag = exp.env === 'knowledge'
      ? '<span class="pill info">资料库</span>'
      : exp.custom
        ? '<span class="pill ok">自定义</span>'
        : ''
    return `<li class="exp-card">
      <button type="button" class="exp-card-main" data-action="open-expert-detail" data-value="${escapeHtml(exp.id)}">
        <div class="exp-card-head">
          <span class="exp-avatar"><img src="${expertAvatarSrc(exp)}" alt=""></span>
          <span class="exp-head-copy">
            <span class="exp-head"><strong>${escapeHtml(exp.name)}</strong>${subTag}</span>
            <span class="exp-sub">${escapeHtml(exp.env === 'knowledge' ? '资料库服务' : '本机')} · ${escapeHtml(EXPERT_CATEGORY_MAP[exp.category] || '通用')}</span>
          </span>
        </div>
        <p class="exp-desc">${escapeHtml(exp.description)}</p>
        <div class="exp-tags">${tags.join('')}</div>
      </button>
      <span class="exp-card-chev" aria-hidden="true">${icon('chevron-right')}</span>
      <button type="button" class="btn primary exp-summon" data-action="summon-expert" data-value="${escapeHtml(exp.id)}" aria-label="召唤 ${escapeHtml(exp.name)} 开始对话">
        ${icon('sparkles')}召唤对话
      </button>
    </li>`
  }

  const renderSection = (title, items) => `<section class="grp">
    <div class="grp-head">
      <div class="grp-head-left"><h2>${escapeHtml(title)}</h2><span class="grp-count">${items.length} 位</span></div>
    </div>
    <ul class="exp-grid">${items.map(renderExpertCard).join('')}</ul>
  </section>`

  const sections = []
  if (featured.length) sections.push(renderSection('常用精选', featured))
  const orderedCats = EXPERT_CATEGORIES.filter((c) => c.key !== 'all')
  orderedCats.forEach((cat) => {
    const matches = remaining.filter((e) => e.category === cat.key)
    if (matches.length) sections.push(renderSection(cat.label, matches))
  })

  return `<div class="sp sp-wide">
    <header class="sp-head">
      <div>
        <h1>专家</h1>
        <p>选择各领域专业助手；点击卡片查看详情，悬停可一键召唤至新对话。</p>
      </div>
      <div class="acts">
        <button type="button" class="btn lg" data-action="open-import-expert">${icon('upload')}导入能力包</button>
        <button type="button" class="btn primary lg" data-action="open-create-expert">${icon('plus')}新建专家</button>
      </div>
    </header>

    <div class="toolbar">
      <div class="toolbar-row">
        <div class="toolbar-filters">
          <select class="sel" data-select="exp-avail" aria-label="按状态筛选">
            <option value="all" ${st.expAvail === 'all' ? 'selected' : ''}>全部状态</option>
            <option value="available" ${st.expAvail === 'available' ? 'selected' : ''}>可使用</option>
            <option value="unavailable" ${st.expAvail === 'unavailable' ? 'selected' : ''}>不可用</option>
          </select>
          <select class="sel" data-select="exp-env" aria-label="按运行环境筛选">
            <option value="all" ${st.expEnv === 'all' ? 'selected' : ''}>全部环境</option>
            <option value="local" ${st.expEnv === 'local' ? 'selected' : ''}>本机</option>
            <option value="knowledge" ${st.expEnv === 'knowledge' ? 'selected' : ''}>资料库</option>
          </select>
          <select class="sel" data-select="exp-cat" aria-label="按分类筛选">
            ${EXPERT_CATEGORIES.map((cat) => `<option value="${cat.key}" ${cat.key === st.expCat ? 'selected' : ''}>${cat.key === 'all' ? '全部分类' : cat.label}</option>`).join('')}
          </select>
          <span class="toolbar-count">${filtered.length} 位</span>
        </div>
        <div class="toolbar-right">
          <label class="inp search-box dir-search">
            ${icon('search')}
            <input type="search" data-input="exp-query" value="${escapeHtml(st.expQuery || '')}" placeholder="搜索专家" aria-label="搜索专家">
            <kbd aria-hidden="true">Ctrl K</kbd>
          </label>
        </div>
      </div>
    </div>

    ${sections.join('') || `<div class="empty">${icon('users')}<h2>没有匹配的专家</h2><p>可以调整筛选条件，或新建一位专属专家。</p>
      <button type="button" class="btn" data-action="exp-reset">清除筛选</button></div>`}

    <footer class="directory-footer">
      <span>共 ${filtered.length} 位可用专家 · 点击卡片看详情，悬停「召唤对话」直达新对话</span>
      <span>本机 ${list.filter((e) => e.env === 'local').length} · 资料库 ${list.filter((e) => e.env === 'knowledge').length}</span>
    </footer>
  </div>`
}

/* ---------- View 5: Expert Detail Page (Equipped capabilities ONLY + Manage drawer) ---------- */
function expertEquippedItems(exp) {
  const skillMap = Object.fromEntries(allSkills().map((s) => [s.name, s]))
  const connMap = Object.fromEntries(installedConnectors().map((c) => [c.name, c]))
  const kbMap = Object.fromEntries(KNOWLEDGE_BASES.map((k) => [k.id, k]))
  const toolMap = Object.fromEntries(tools.map((t) => [t.name, t]))

  const items = []
  ;(exp.skills || []).forEach((name) => {
    const src = skillMap[name]
    items.push({ kind: 'skills', kindLabel: '技能', name, desc: src?.description || '已启用的技能规范', mark: src ? itemMark(src) : 'puzzle' })
  })
  ;(exp.connectors || []).forEach((name) => {
    const src = connMap[name]
    items.push({ kind: 'connectors', kindLabel: '连接器', name, desc: src?.description || '已绑定的外部连接器', mark: src ? itemMark(src) : 'cable' })
  })
  ;(exp.knowledge || []).forEach((name) => {
    const src = kbMap[name]
    items.push({ kind: 'knowledge', kindLabel: '资料库', name, desc: src?.desc || '已关联的知识资料库', mark: 'library-big' })
  })
  ;(exp.tools || []).forEach((name) => {
    const src = toolMap[name]
    items.push({ kind: 'tools', kindLabel: '内置工具', name, desc: src?.description || 'Fox 内置核心工具', mark: src ? itemMark(src) : 'wrench' })
  })
  return items
}

function renderExpertDetailView(frameId, st) {
  const exp = findExpertById(st.expertId)
  const catLabel = EXPERT_CATEGORY_MAP[exp.category] || '通用'
  const readOnly = exp.env === 'knowledge'

  const equipped = expertEquippedItems(exp)
  const kindDefs = [
    ['skills', '技能'],
    ['connectors', '连接器'],
    ['knowledge', '资料库'],
    ['tools', '内置工具'],
  ]
  // Only display categories that actually have enabled items!
  const nonEmptyTabs = kindDefs
    .map(([key, label]) => [key, label, equipped.filter((item) => item.kind === key).length])
    .filter(([, , count]) => count > 0)
  const tabs = [['all', '全部', equipped.length], ...nonEmptyTabs]
  const activeTab = tabs.some(([k]) => k === st.expDetailTab) ? st.expDetailTab : 'all'
  const visibleEquipped = activeTab === 'all' ? equipped : equipped.filter((item) => item.kind === activeTab)

  const menuItems = [
    `<button type="button" class="mi" data-action="copy-expert" data-value="${escapeHtml(exp.id)}">${icon('copy')}复制为新专家</button>`,
    `<button type="button" class="mi" data-action="export-expert" data-value="${escapeHtml(exp.name)}">${icon('download')}导出能力包 (.foxexpert)</button>`,
    `<button type="button" class="mi" data-action="create-companion-from-expert" data-value="${escapeHtml(exp.id)}">${icon('cat-paw')}创建为常驻伙伴</button>`,
  ]
  if (exp.custom) {
    menuItems.push('<div class="ms"></div>')
    menuItems.push(`<button type="button" class="mi danger" data-action="delete-expert" data-value="${escapeHtml(exp.id)}">${icon('trash-2')}删除专家</button>`)
  }

  const factRows = [
    ['所属领域', escapeHtml(catLabel)],
    ['运行环境', exp.env === 'knowledge' ? '资料库服务（远程托管）' : '本机 Fox Runtime'],
    ['默认模型', escapeHtml(exp.model || '跟随默认模型')],
    ['来源与版本', `${escapeHtml(exp.source || 'Fox 内置')} · v${escapeHtml(exp.version || '1.0.0')}`],
    ['最近协作', `${exp.recentRuns || 0} 次对话 · 常用项目：${escapeHtml(exp.recentProject || 'Fox')}`],
  ]

  return `<div class="sp sp-detail">
    <nav class="breadcrumb" aria-label="面包屑导航">
      <button type="button" class="crumb-btn" data-action="go-experts">专家</button>
      <span class="crumb-sep" aria-hidden="true">${icon('chevron-right')}</span>
      <button type="button" class="crumb-btn" data-action="go-experts-cat" data-value="${escapeHtml(exp.category)}">${escapeHtml(catLabel)}</button>
      <span class="crumb-sep" aria-hidden="true">${icon('chevron-right')}</span>
      <span class="crumb-current" aria-current="page">${escapeHtml(exp.name)}</span>
    </nav>

    <header class="detail-hero">
      <div class="detail-id">
        <span class="exp-avatar xl"><img src="${expertAvatarSrc(exp)}" alt=""></span>
        <div class="detail-title-wrap">
          <div class="detail-title-row">
            <h1>${escapeHtml(exp.name)}</h1>
            <span class="pill ${exp.env === 'knowledge' ? 'info' : 'ok'}">${exp.env === 'knowledge' ? '资料库服务' : '本机可用'}</span>
            ${exp.custom ? '<span class="pill">自定义</span>' : ''}
          </div>
          <p class="detail-sub">${escapeHtml(exp.description)}</p>
          <div class="exp-stats">
            <span>${icon('history')}协作 ${exp.recentRuns || 0} 次</span>
            <span>${icon('folder-open')}常用项目 ${escapeHtml(exp.recentProject || 'Fox')}</span>
            <span>${icon('boxes')}${escapeHtml(exp.model || '跟随默认模型')}</span>
          </div>
        </div>
      </div>
      <div class="detail-acts">
        <button type="button" class="btn ghost icon lg" data-action="toggle-detail-menu" aria-expanded="${st.menuOpen === 'detail'}" aria-label="更多专家操作">${icon('ellipsis')}</button>
        ${st.menuOpen === 'detail' ? `<div class="menu" role="menu">${menuItems.join('')}</div>` : ''}
        ${!readOnly ? `<button type="button" class="btn lg" data-action="open-edit-expert" data-value="${escapeHtml(exp.id)}">${icon('pencil')}编辑</button>` : ''}
        <button type="button" class="btn primary lg" data-action="summon-expert" data-value="${escapeHtml(exp.id)}">${icon('sparkles')}召唤对话</button>
      </div>
    </header>

    <section class="detail-showcase" aria-label="推荐开场提问">
      ${(exp.openings || []).map((text) => `<button type="button" class="prompt-card" data-action="summon-expert-prompt" data-value="${escapeHtml(exp.id)}" data-text="${escapeHtml(text)}">
        <span class="prompt-text"><span class="prompt-mention"><span class="exp-avatar sm"><img src="${expertAvatarSrc(exp)}" alt=""></span>${escapeHtml(exp.name)}</span>${escapeHtml(text)}</span>
        <span class="prompt-go" aria-hidden="true">${icon('arrow-right')}</span>
      </button>`).join('')}
    </section>

    ${readOnly ? `<div class="banner"><span>${icon('info')}</span><div class="tx"><b>资料库服务托管专家</b><small>该专家的工作要求与知识范围由远程资料库服务统一维护，点击右上角「召唤对话」即可直接开始调研。</small></div></div>` : ''}

    <section class="detail-sec">
      <div class="detail-sec-head">
        <h2>工作要求</h2>
        <span>系统提示词</span>
        <span class="grow"></span>
        ${!readOnly ? `<button type="button" class="btn ghost" data-action="open-edit-expert" data-value="${escapeHtml(exp.id)}">${icon('pencil')}修改</button>` : ''}
      </div>
      <div class="prompt-box">${escapeHtml(exp.prompt || '暂无工作要求。')}</div>
    </section>

    <section class="detail-sec">
      <div class="detail-sec-head">
        <h2>已装配能力</h2>
        <span>${equipped.length} 项已启用</span>
        <span class="grow"></span>
        ${!readOnly ? `<button type="button" class="btn" data-action="open-expert-caps" data-value="${escapeHtml(exp.id)}">${icon('settings')}管理装配</button>` : ''}
      </div>
      ${equipped.length ? `
        ${tabs.length > 2 ? `<div class="cap-tabs" role="tablist" aria-label="已装配能力分类">
          ${tabs.map(([key, label, count]) => `<button type="button" class="tab-pill" role="tab"
            data-action="exp-detail-tab" data-value="${key}" aria-selected="${key === activeTab}">${label}<span>${count}</span></button>`).join('')}
        </div>` : ''}
        <div class="cap-card-list">
          ${visibleEquipped.map((item) => `<div class="cap-item-row">
            ${mark(item.mark)}
            <div class="tx"><b>${escapeHtml(item.name)}</b><small>${escapeHtml(item.desc)}</small></div>
            <span class="exp-tag">${escapeHtml(item.kindLabel)}</span>
          </div>`).join('')}
        </div>
      ` : `<div class="empty" style="padding:24px 16px"><p style="margin:0">暂未装配额外能力，可在右上角点击「管理装配」按需启用。</p></div>`}
    </section>

    <section class="detail-sec">
      <div class="detail-sec-head"><h2>基础信息</h2></div>
      <dl class="facts">${factRows.map(([k, v]) => `<div><dt>${k}</dt><dd>${v}</dd></div>`).join('')}</dl>
    </section>
  </div>`
}

/* ---------- View 6: Summoned New Chat Preview (reached ONLY via 召唤对话) ---------- */
function renderSummonedChatView(frameId, st) {
  const exp = st.chatExpertId ? findExpertById(st.chatExpertId) : null
  return `<div class="chat-stage">
    <div class="chat-topbar">
      <button type="button" class="btn ghost" data-action="go-experts">${icon('arrow-left')}返回专家中心</button>
      ${exp ? `<button type="button" class="btn ghost" data-action="open-expert-detail" data-value="${escapeHtml(exp.id)}">${icon('info')}查看专家详情</button>` : ''}
    </div>

    <div class="chat-welcome">
      <span class="exp-avatar xl"><img src="${exp ? expertAvatarSrc(exp) : FOX}" alt=""></span>
      <h1>${exp ? `已召唤「${escapeHtml(exp.name)}」` : '开始一段新对话'}</h1>
      <p>${exp ? escapeHtml(exp.description) : '可直接输入问题，或前往专家中心挑选一位专业助手加入对话。'}</p>
      ${exp?.openings?.length ? `<div class="chat-suggestions">
        ${exp.openings.map((text) => `<button type="button" class="prompt-card" data-action="fill-chat-draft" data-text="${escapeHtml(text)}">
          <span class="prompt-text">${escapeHtml(text)}</span>
          <span class="prompt-go" aria-hidden="true">${icon('arrow-right')}</span>
        </button>`).join('')}
      </div>` : ''}
    </div>

    <div class="chat-composer">
      <textarea class="chat-composer-input" data-input="chat-draft" placeholder="${exp ? `向 ${escapeHtml(exp.name)} 描述你的任务…` : '输入消息，按 Enter 发送…'}">${escapeHtml(st.chatDraft || '')}</textarea>
      <div class="chat-composer-foot">
        <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap">
          ${exp ? `<span class="chat-expert-chip"><img src="${expertAvatarSrc(exp)}" alt=""><span>@${escapeHtml(exp.name)}</span><button type="button" data-action="detach-chat-expert" aria-label="移除已召唤专家" title="移除专家">${icon('x')}</button></span>` : `<button type="button" class="btn" data-action="go-experts">${icon('users')}选择专家</button>`}
          <span class="stat">${icon('boxes')}<b>${escapeHtml(exp?.model || '跟随默认模型')}</b></span>
        </div>
        <button type="button" class="btn primary lg" data-action="send-chat">${icon('send')}发送</button>
      </div>
    </div>
  </div>`
}

/* ---------- View 7: Companion Workspace (待我处理 / 全部伙伴 / 伙伴私聊) + Redesigned Create Modal ---------- */
function findCompanion(id) {
  return (demo.companions || []).find((c) => c.id === id) || demo.companions[0]
}

function renderCompanionView(frameId, st) {
  if (st.compSection === 'tasks') {
    const taskList = demo.tasks || []
    const enabledCount = taskList.filter((t) => t.enabled).length
    return `<div class="sp sp-wide">
      <header class="sp-head">
        <div>
          <h1>定时任务</h1>
          <p>汇总所有伙伴的定时计划与自动巡检任务；不必逐个点进资料卡即可统一启停或立即触发。</p>
        </div>
        <div class="acts">
          <button type="button" class="btn primary lg" data-action="open-new-comp-task">${icon('plus')}新建定时任务</button>
        </div>
      </header>

      <div class="set-list">
        ${taskList.length ? taskList.map((t) => {
          const comp = findCompanion(t.companionId)
          return `<div class="set-row ${t.enabled ? '' : 'is-off'}">
            <button type="button" class="set-row-main" data-action="open-companion-chat" data-value="${escapeHtml(comp.id)}">
              <span class="exp-avatar"><img src="${escapeHtml(comp.avatar)}" alt=""></span>
              <span class="set-row-copy">
                <span class="set-row-head">
                  <span class="set-row-name">${escapeHtml(t.name)}</span>
                  <span class="pill ${t.enabled ? 'info' : ''}">${escapeHtml(t.schedule)}</span>
                  <span class="set-row-sub">${escapeHtml(comp.name)} · 下次：${escapeHtml(t.enabled ? t.nextRun : '已暂停')}</span>
                </span>
                <span class="set-row-desc">${escapeHtml(t.objective)} · ${escapeHtml(t.lastResult)}</span>
              </span>
            </button>
            <span class="set-row-ctrl">
              <button type="button" class="btn" data-action="run-comp-task" data-value="${escapeHtml(t.id)}">${icon('play')}立即执行</button>
              <button type="button" class="sw ${t.enabled ? 'on' : ''}" role="switch" aria-checked="${Boolean(t.enabled)}"
                data-action="toggle-comp-task" data-value="${escapeHtml(t.id)}" aria-label="${t.enabled ? '暂停' : '启用'} ${escapeHtml(t.name)}"><i></i></button>
            </span>
          </div>`
        }).join('') : `<div class="empty" style="border:0">${icon('calendar-clock')}<h2>还没有定时任务</h2><p>可以给任意伙伴安排每日简报、周期巡检或提醒。</p></div>`}
      </div>

      <div class="set-foot-hint">
        <span>共 ${taskList.length} 项任务，${enabledCount} 项运行中 · 任务结果会直接发送到对应伙伴的私聊中。</span>
        <button type="button" class="btn ghost" data-action="comp-section" data-value="inbox">查看待我处理${icon('chevron-right')}</button>
      </div>
    </div>`
  }

  if (st.compSection === 'overview') {
    const active = (demo.companions || []).filter((c) => !c.archived)
    return `<div class="sp sp-wide">
      <header class="sp-head">
        <div>
          <h1>全部伙伴</h1>
          <p>管理常驻数字伙伴的职责、工作页快捷入口与执行权限；点击任意伙伴进入私聊。</p>
        </div>
        <div class="acts">
          <button type="button" class="btn lg" data-action="open-create-companion" data-mode="expert">${icon('users')}从专家创建</button>
          <button type="button" class="btn primary lg" data-action="open-create-companion" data-mode="blank">${icon('plus')}新建伙伴</button>
        </div>
      </header>
      <div class="set-list">
        ${active.map((c) => `<div class="set-row">
          <button type="button" class="set-row-main" data-action="open-companion-chat" data-value="${escapeHtml(c.id)}">
            <span class="exp-avatar"><img src="${escapeHtml(c.avatar)}" alt=""></span>
            <span class="set-row-copy">
              <span class="set-row-head">
                <span class="set-row-name">${escapeHtml(c.name)}</span>
                ${c.isDefault ? '<span class="pill ok">默认伙伴</span>' : ''}
                ${c.pinned ? '<span class="pill">已置顶</span>' : ''}
                <span class="set-row-sub">${escapeHtml(c.perm)} · ${escapeHtml(c.model)}</span>
              </span>
              <span class="set-row-desc">${escapeHtml(c.duty)}</span>
            </span>
          </button>
          <span class="set-row-ctrl">
            <span class="set-scope">工作页快捷</span>
            <button type="button" class="sw ${c.inWork ? 'on' : ''}" role="switch" aria-checked="${Boolean(c.inWork)}" ${c.isDefault ? 'disabled' : ''}
              data-action="toggle-comp-work" data-value="${escapeHtml(c.id)}" aria-label="在工作页显示 ${escapeHtml(c.name)}"><i></i></button>
            <button type="button" class="set-chev" data-action="open-companion-chat" data-value="${escapeHtml(c.id)}" aria-label="打开私聊">${icon('chevron-right')}</button>
          </span>
        </div>`).join('')}
      </div>
      <div class="set-foot-hint">
        <span>新伙伴的工作目录、默认模型与提醒方式遵循全局默认值。</span>
        <a class="btn ghost" href="../index.html?frame=companions">${icon('settings')}伙伴默认设置</a>
      </div>
    </div>`
  }

  if (st.compSection === 'chat') {
    const comp = findCompanion(st.compActiveId)
    return `<div class="cchat-wrap">
      <div class="cchat-head">
        <button type="button" class="btn ghost" data-action="comp-section" data-value="inbox">${icon('arrow-left')}待我处理</button>
        <div class="cchat-pill"><img src="${escapeHtml(comp.avatar)}" alt=""><b>${escapeHtml(comp.name)}</b></div>
        <button type="button" class="btn ghost" data-action="noop" data-label="伙伴资料与文件抽屉">${icon('info')}资料</button>
      </div>
      <div class="cchat-stream">
        <div class="cmsg">
          <img src="${escapeHtml(comp.avatar)}" alt="">
          <div class="cbubble">你好 Hury，我是 <b>${escapeHtml(comp.name)}</b>（${escapeHtml(comp.duty)}）。今天有什么需要我长期跟进的事情？</div>
        </div>
        <div class="cmsg me">
          <div class="cbubble">帮我盯着这轮插件中心和专家页的改动，有需要我确认的操作直接发给我。</div>
        </div>
        <div class="cmsg">
          <img src="${escapeHtml(comp.avatar)}" alt="">
          <div class="cbubble">${escapeHtml(comp.preview)}</div>
        </div>
      </div>
      <div class="chat-composer">
        <textarea class="chat-composer-input" data-input="comp-chat-draft" placeholder="给 ${escapeHtml(comp.name)} 发消息…">${escapeHtml(st.compChatDraft || '')}</textarea>
        <div class="chat-composer-foot">
          <span class="stat">${icon('folder-open')}<code>${escapeHtml(comp.dir)}</code></span>
          <button type="button" class="btn primary lg" data-action="send-comp-chat">${icon('send')}发送</button>
        </div>
      </div>
    </div>`
  }

  // Default: 'inbox' (待我处理 / 伙伴动态收件箱)
  const pending = (demo.inbox || []).filter((i) => !i.resolved && i.kind === 'approval')
  const running = (demo.inbox || []).filter((i) => i.kind === 'running')
  const done = (demo.inbox || []).filter((i) => i.resolved || i.kind === 'done')

  const renderInboxItem = (item) => {
    const comp = findCompanion(item.companionId)
    const pillHtml = item.resolved
      ? '<span class="pill ok">已处理</span>'
      : item.kind === 'approval'
        ? '<span class="pill warn">等你确认</span>'
        : '<span class="pill info">后台运行中</span>'
    return `<div class="inbox-card">
      <span class="exp-avatar"><img src="${escapeHtml(comp.avatar)}" alt=""></span>
      <div class="inbox-body">
        <div class="inbox-top">
          <b>${escapeHtml(comp.name)}</b>
          ${pillHtml}
          <span class="set-row-sub">${escapeHtml(item.title)}</span>
          <time>${escapeHtml(item.time)}</time>
        </div>
        <p class="inbox-text">${escapeHtml(item.desc)}</p>
        <div class="inbox-op">${escapeHtml(item.op)}</div>
        <div class="inbox-acts">
          ${item.kind === 'approval' && !item.resolved ? `
            <button type="button" class="btn primary" data-action="resolve-inbox" data-value="${item.id}" data-decision="approve">${icon('check')}批准执行</button>
            <button type="button" class="btn" data-action="resolve-inbox" data-value="${item.id}" data-decision="reject">暂不执行</button>
          ` : ''}
          <button type="button" class="btn ghost" data-action="open-companion-chat" data-value="${escapeHtml(comp.id)}">进入私聊${icon('chevron-right')}</button>
        </div>
      </div>
    </div>`
  }

  return `<div class="sp">
    <header class="sp-head">
      <div>
        <h1>待我处理</h1>
        <p>集中查看所有伙伴等待审批的操作、后台任务进度与最新交付物，无需逐个点开私聊。</p>
      </div>
      <div class="acts">
        <button type="button" class="btn primary lg" data-action="open-create-companion" data-mode="blank">${icon('plus')}新建伙伴</button>
      </div>
    </header>

    <section class="grp">
      <div class="grp-head"><div class="grp-head-left"><h2>等待你确认</h2><span class="grp-count">${pending.length} 项</span></div></div>
      <div class="grp-body" style="padding:0">${pending.length ? pending.map(renderInboxItem).join('') : '<div class="empty" style="border:0;padding:28px 16px"><p style="margin:0">所有伙伴待确认事项都已处理完毕。</p></div>'}</div>
    </section>

    ${running.length ? `<section class="grp">
      <div class="grp-head"><div class="grp-head-left"><h2>正在后台执行</h2><span class="grp-count">${running.length} 项</span></div></div>
      <div class="grp-body" style="padding:0">${running.map(renderInboxItem).join('')}</div>
    </section>` : ''}

    ${done.length ? `<section class="grp">
      <div class="grp-head"><div class="grp-head-left"><h2>近期交付与已处理</h2><span class="grp-count">${done.length} 项</span></div></div>
      <div class="grp-body" style="padding:0">${done.map(renderInboxItem).join('')}</div>
    </section>` : ''}
  </div>`
}

function renderCreateCompanionModal(frameId, st) {
  if (!st.createCompModal) return ''
  const mode = st.createCompModal // 'blank' | 'expert'

  if (mode === 'blank') {
    const chosenAvatarFile = st.createCompAvatar || DEFAULT_AVATARS[0]
    const chosenAvatarUrl = `${ROOT}/avatars/defaults/${chosenAvatarFile}`
    const currentName = (st.createCompName || '').trim()
    const matchedPreset = COMPANION_PRESETS.find((p) => currentName && currentName === p.name)

    return `<div class="modal-scrim" data-action="close-comp-modal-scrim">
      <section class="modal modal-create-blank" role="dialog" aria-modal="true" aria-label="新建伙伴">
        <header class="modal-head modal-head-clean">
          <h2>新建伙伴</h2>
          <button type="button" class="modal-x" data-action="close-comp-modal" aria-label="关闭弹窗">${icon('x')}</button>
        </header>

        <div class="modal-body create-blank-body">
          <div class="create-avatar-hero">
            <img src="${chosenAvatarUrl}" alt="">
          </div>

          <input class="inp create-name-input" data-input="create-comp-name" maxlength="40" value="${escapeHtml(st.createCompName || '')}" placeholder="给你的伙伴起个名字" aria-label="给你的伙伴起个名字">

          <div class="create-avatar-grid" role="radiogroup" aria-label="选择头像">
            ${DEFAULT_AVATARS.map((file) => {
              const on = file === chosenAvatarFile
              return `<button type="button" class="create-avatar-opt ${on ? 'on' : ''}" role="radio" aria-checked="${on}" data-action="pick-comp-avatar" data-value="${file}" aria-label="选择头像 ${file}">
                <img src="${ROOT}/avatars/defaults/${file}" alt="">
              </button>`
            }).join('')}
          </div>

          <div class="create-preset-block">
            <div class="create-preset-chips" aria-label="快捷角色模板">
              ${COMPANION_PRESETS.map((p, idx) => {
                const on = currentName === p.name
                return `<button type="button" class="preset-chip ${on ? 'on' : ''}" data-action="apply-comp-preset" data-idx="${idx}">${p.label}</button>`
              }).join('')}
            </div>
            <p class="create-preset-hint">${matchedPreset ? escapeHtml(matchedPreset.duty) : '也可直接点选上方快捷角色模板快速起名'}</p>
          </div>
        </div>

        <footer class="modal-foot modal-foot-clean">
          <button type="button" class="btn pill-btn" data-action="close-comp-modal">取消</button>
          <button type="button" class="btn primary pill-btn" data-action="confirm-create-companion">创建</button>
        </footer>
      </section>
    </div>`
  }

  // mode === 'expert'
  const localExperts = allExperts().filter((e) => e.env === 'local')
  const q = (st.createCompExpertQuery || '').trim().toLowerCase()
  const filteredExperts = localExperts.filter((e) => {
    if (st.createCompExpertCat !== 'all' && e.category !== st.createCompExpertCat) return false
    if (!q) return true
    return [e.name, e.description, EXPERT_CATEGORY_MAP[e.category]].join(' ').toLowerCase().includes(q)
  })
  const selectedExpert = localExperts.find((e) => e.id === st.createCompExpertId) || localExperts[0]

  return `<div class="modal-scrim" data-action="close-comp-modal-scrim">
    <section class="modal modal-create-expert" role="dialog" aria-modal="true" aria-label="从专家创建">
      <header class="modal-head">
        <h2>从专家创建</h2>
        <button type="button" class="modal-x" data-action="close-comp-modal" aria-label="关闭弹窗">${icon('x')}</button>
      </header>

      <div class="modal-body create-expert-body">
        <div class="exp-pick-bar">
          <label class="inp search-box">
            ${icon('search')}
            <input type="search" data-input="create-comp-exp-query" value="${escapeHtml(st.createCompExpertQuery || '')}" placeholder="搜索专家名称或用处" aria-label="搜索专家">
          </label>
          <select class="sel" data-select="create-comp-exp-cat" aria-label="按领域筛选">
            ${EXPERT_CATEGORIES.map((c) => `<option value="${c.key}" ${c.key === st.createCompExpertCat ? 'selected' : ''}>${c.key === 'all' ? '全部分类' : c.label}</option>`).join('')}
          </select>
        </div>

        ${filteredExperts.length ? `<div class="exp-pick-grid">
          ${filteredExperts.map((exp) => {
            const on = exp.id === selectedExpert?.id
            return `<button type="button" class="exp-pick-item ${on ? 'on' : ''}" data-action="pick-comp-expert" data-value="${escapeHtml(exp.id)}">
              <img src="${expertAvatarSrc(exp)}" alt="">
              <span class="exp-pick-copy">
                <span class="exp-pick-title">
                  <b>${escapeHtml(exp.name)}</b>
                  ${on ? `<span class="exp-pick-check" aria-hidden="true">${icon('check')}</span>` : ''}
                </span>
                <small>${escapeHtml(exp.description)}</small>
              </span>
            </button>`
          }).join('')}
        </div>` : '<div class="empty" style="padding:36px 16px"><p style="margin:0">没有匹配的专家，可换个关键词试试。</p></div>'}
      </div>

      <footer class="modal-foot">
        <div class="modal-foot-info">
          ${selectedExpert ? `<img src="${expertAvatarSrc(selectedExpert)}" alt=""><span>已选 <b>${escapeHtml(selectedExpert.name)}</b></span>` : '<span>请选择一位专家</span>'}
        </div>
        <div class="modal-foot-acts">
          <button type="button" class="btn lg" data-action="close-comp-modal">取消</button>
          <button type="button" class="btn primary lg" data-action="confirm-create-companion">创建</button>
        </div>
      </footer>
    </section>
  </div>`
}

function renderNotificationsView(frameId, st) {
  const all = demo.notifications || []
  const unreadCount = all.filter((n) => n.unread).length
  const actionCount = all.filter((n) => n.kind === 'approval' || n.kind === 'failed').length
  const hasRead = all.some((n) => !n.unread)

  const filtered = all.filter((n) => {
    if (st.notifFilter === 'unread') return n.unread
    if (st.notifFilter === 'action') return n.kind === 'approval' || n.kind === 'failed'
    return true
  })

  // Group by date while preserving order
  const groups = []
  filtered.forEach((item) => {
    const last = groups[groups.length - 1]
    if (last && last.date === item.date) last.items.push(item)
    else groups.push({ date: item.date, label: item.dateLabel || item.date, items: [item] })
  })

  const renderBadge = (item) => {
    if (item.kind === 'approval') return '<span class="pill warn">待确认</span>'
    if (item.kind === 'failed') return '<span class="pill err">异常</span>'
    return ''
  }

  const renderVisual = (item) => {
    const dot = item.unread ? '<span class="notif-unread-dot" aria-label="未读"></span>' : ''
    if (item.avatar) {
      return `<span class="notif-av"><img src="${escapeHtml(item.avatar)}" alt="">${dot}</span>`
    }
    return `<span class="notif-av is-icon ${escapeHtml(item.tone || 'brand')}">${icon(item.icon || 'bell')}${dot}</span>`
  }

  return `<div class="notif-page">
    <header class="notif-topbar">
      <div class="notif-topbar-side"></div>
      <h1 class="notif-page-title">消息中心</h1>
      <div class="notif-topbar-side right">
        <button type="button" class="modal-x" data-action="close-notifications" aria-label="关闭消息中心" title="关闭">${icon('x')}</button>
      </div>
    </header>

    <div class="notif-wrap">
      <div class="notif-toolbar">
        <div class="tab-pills" role="tablist" aria-label="通知筛选">
          <button type="button" class="tab-pill" role="tab" data-action="notif-filter" data-value="all" aria-selected="${st.notifFilter === 'all'}">全部<span>${all.length}</span></button>
          <button type="button" class="tab-pill" role="tab" data-action="notif-filter" data-value="unread" aria-selected="${st.notifFilter === 'unread'}">未读<span>${unreadCount}</span></button>
          <button type="button" class="tab-pill" role="tab" data-action="notif-filter" data-value="action" aria-selected="${st.notifFilter === 'action'}">需处理<span>${actionCount}</span></button>
        </div>
        <div class="notif-toolbar-acts">
          <button type="button" class="btn ghost" data-action="notif-mark-all" ${unreadCount ? '' : 'disabled'}>${icon('check-check')}全部已读</button>
          <button type="button" class="btn ghost" data-action="notif-clear-read" ${hasRead ? '' : 'disabled'}>${icon('trash-2')}清除已读</button>
        </div>
      </div>

      ${groups.length ? `<div class="notif-stream">
        ${groups.map((grp) => `<section class="notif-group">
          <div class="notif-date">${escapeHtml(grp.label)}</div>
          <div class="notif-list">
            ${grp.items.map((item) => {
              const expanded = Boolean(st.notifExpanded?.[item.id])
              return `<article class="notif-row ${item.unread ? 'is-unread' : ''}">
                ${renderVisual(item)}
                <div class="notif-copy">
                  <div class="notif-head">
                    <strong>${escapeHtml(item.title)}</strong>
                    ${renderBadge(item)}
                    <time>${escapeHtml(item.time)}</time>
                  </div>
                  <p class="notif-body ${expanded ? 'expanded' : ''}">
                    <span>${escapeHtml(item.body)}</span>
                    ${item.expandable ? `<button type="button" class="notif-expand" data-action="notif-toggle-expand" data-value="${escapeHtml(item.id)}">${expanded ? '收起' : '展开'}</button>` : ''}
                  </p>
                </div>
                ${item.actionLabel ? `<div class="notif-cta">
                  <button type="button" class="btn notif-btn ${item.kind === 'approval' ? 'primary' : ''}" data-action="notif-action" data-value="${escapeHtml(item.id)}">${escapeHtml(item.actionLabel)}</button>
                </div>` : ''}
              </article>`
            }).join('')}
          </div>
        </section>`).join('')}
      </div>` : `<div class="empty" style="margin-top:20px">${icon('bell')}<h2>暂无${st.notifFilter === 'unread' ? '未读' : st.notifFilter === 'action' ? '需处理的' : ''}消息</h2><p>伙伴审批、任务交付与系统更新都会显示在这里。</p></div>`}
    </div>
  </div>`
}

function renderCreateTaskModal(frameId, st) {
  if (!st.createTaskModal) return ''
  const activeCompanions = (demo.companions || []).filter((c) => !c.archived)
  const chosenCompId = activeCompanions.some((c) => c.id === st.taskCompanionId)
    ? st.taskCompanionId
    : (activeCompanions[0]?.id || 'cmp-default')
  const chosenSchedule = st.taskSchedule || TASK_SCHEDULES[0]

  return `<div class="modal-scrim" data-action="close-task-modal-scrim">
    <section class="modal modal-create-blank modal-create-task" role="dialog" aria-modal="true" aria-label="新建定时任务">
      <header class="modal-head modal-head-clean">
        <h2>新建定时任务</h2>
        <button type="button" class="modal-x" data-action="close-task-modal" aria-label="关闭弹窗">${icon('x')}</button>
      </header>

      <div class="modal-body create-task-body">
        <div class="task-fld">
          <span class="task-fld-label">执行伙伴</span>
          <div class="task-comp-pills" role="radiogroup" aria-label="选择执行伙伴">
            ${activeCompanions.map((c) => {
              const on = c.id === chosenCompId
              return `<button type="button" class="task-comp-pill ${on ? 'on' : ''}" role="radio" aria-checked="${on}" data-action="pick-task-companion" data-value="${escapeHtml(c.id)}">
                <img src="${escapeHtml(c.avatar)}" alt="">
                <span>${escapeHtml(c.name)}</span>
              </button>`
            }).join('')}
          </div>
        </div>

        <label class="task-fld">
          <span class="task-fld-label">任务名称</span>
          <input class="inp task-inp" data-input="task-name" maxlength="50" value="${escapeHtml(st.taskName || '')}" placeholder="例如：每日晨间简报、周五进展周报">
        </label>

        <div class="task-fld">
          <span class="task-fld-label">执行频率</span>
          <div class="task-sched-pills" role="radiogroup" aria-label="选择执行频率">
            ${TASK_SCHEDULES.map((sched) => {
              const on = sched === chosenSchedule
              return `<button type="button" class="preset-chip ${on ? 'on' : ''}" role="radio" aria-checked="${on}" data-action="pick-task-schedule" data-value="${escapeHtml(sched)}">${escapeHtml(sched)}</button>`
            }).join('')}
          </div>
        </div>

        <label class="task-fld">
          <span class="task-fld-label">任务目标</span>
          <textarea class="ta task-ta" data-input="task-objective" rows="3" maxlength="200" placeholder="说明到点后需要伙伴自动检查、整理或交付什么内容">${escapeHtml(st.taskObjective || '')}</textarea>
        </label>
      </div>

      <footer class="modal-foot modal-foot-clean">
        <button type="button" class="btn pill-btn" data-action="close-task-modal">取消</button>
        <button type="button" class="btn primary pill-btn" data-action="confirm-create-task">创建</button>
      </footer>
    </section>
  </div>`
}

function renderWin(frame) {
  const st = frameState.get(frame.id)
  let content = ''
  if (st.view === 'detail') content = renderDetailView(frame.id, st)
  else if (st.view === 'directory') content = renderDirectoryView(frame.id, st)
  else if (st.view === 'experts') content = renderExpertsView(frame.id, st)
  else if (st.view === 'expert-detail') content = renderExpertDetailView(frame.id, st)
  else if (st.view === 'chat') content = renderSummonedChatView(frame.id, st)
  else if (st.view === 'companions') content = renderCompanionView(frame.id, st)
  else if (st.view === 'notifications') content = renderNotificationsView(frame.id, st)
  else content = renderSettingsView(frame.id, st)

  const sidebar = st.view === 'settings'
    ? renderSettingsSidebar()
    : (st.view === 'companions' || (st.view === 'notifications' && st.notifSidebar === 'companions'))
      ? renderCompanionSidebar(st)
      : renderMainSidebar(st)

  return `<div class="win" data-frame="${frame.id}" style="--frame-height:${frame.height}px">
    ${renderTitlebar(st)}
    <div class="app">
      ${sidebar}
      <div class="card">
        <main class="spage" tabindex="-1">${content}</main>
      </div>
    </div>
    ${renderCreateCompanionModal(frame.id, st)}
    ${renderCreateTaskModal(frame.id, st)}
  </div>`
}

function renderFrame(frameId) {
  const winEl = document.querySelector(`.win[data-frame="${frameId}"]`)
  const frame = FRAMES.find((f) => f.id === frameId)
  if (!winEl || !frame) return
  const scrollTop = winEl.querySelector('.spage')?.scrollTop || 0
  const st = frameState.get(frameId)
  const tmp = document.createElement('div')
  tmp.innerHTML = renderWin(frame)
  const nextWin = tmp.firstElementChild
  // Keep open sheet if any? Re-render replaces win cleanly; sheets are closed before re-render.
  winEl.replaceWith(nextWin)
  if (st.preserveScroll) {
    nextWin.querySelector('.spage').scrollTop = scrollTop
    st.preserveScroll = false
  }
}

function renderAllFrames() {
  FRAMES.forEach((f) => {
    const st = frameState.get(f.id)
    st.preserveScroll = true
    renderFrame(f.id)
  })
}

function pushHistory(st, snapshot) {
  st.history.splice(st.historyIdx + 1)
  st.history.push(snapshot)
  st.historyIdx = st.history.length - 1
}

/* ---------- Right Sheet Drawer (per-card, matching 设置页美化/index.html) ---------- */
function openSheet(winEl, { markName, title, subtitle = '', pills = '', body, footer }) {
  const card = winEl.querySelector('.card')
  closeSheet(card)
  card.insertAdjacentHTML('beforeend', `
    <div class="scrim" data-action="close-sheet"></div>
    <aside class="sheet" role="dialog" aria-modal="true">
      <header class="sh-head">
        ${mark(markName, 'lg')}
        <div class="tt">
          <div class="tn"><span>${escapeHtml(title)}</span>${pills}</div>
          ${subtitle ? `<small>${escapeHtml(subtitle)}</small>` : ''}
        </div>
        <button type="button" class="x" data-action="close-sheet" aria-label="关闭抽屉">${icon('x')}</button>
      </header>
      <div class="sh-body">${body}</div>
      <footer class="sh-foot">${footer}</footer>
    </aside>`)
  card.querySelector('.spage').inert = true
  card.querySelector('.sheet .x').focus({ preventScroll: true })
}

function closeSheet(card) {
  if (!card) return
  card.querySelectorAll(':scope > .sheet, :scope > .scrim').forEach((el) => el.remove())
  const spage = card.querySelector('.spage')
  if (spage) spage.inert = false
}

function showConnectorSheet(winEl, prefill = null) {
  const isLocal = prefill?.method === 'local' || prefill?.transport === 'stdio MCP'
  const transport = prefill?.transport || (isLocal ? 'stdio MCP' : 'HTTP MCP')
  const defaultTarget = prefill?.target || (prefill?.id ? `https://mcp.example.com/${prefill.id}` : '')
  openSheet(winEl, {
    markName: prefill ? itemMark(prefill) : 'cable',
    title: prefill ? `配置 ${prefill.name}` : '添加连接器',
    subtitle: prefill ? `连接方式 · ${transport}` : '填写连接地址或启动命令，保存后自动加入已启用列表。',
    body: `<form id="sheet-form" data-kind="connector" data-candidate="${escapeHtml(prefill?.fromCandidate || (prefill?.method ? prefill.id : ''))}" data-editing="${escapeHtml(!prefill?.method && prefill?.id ? prefill.id : '')}">
      <label class="fld"><span>名称</span><input class="inp" name="name" required maxlength="60" value="${escapeHtml(prefill?.name || '')}" placeholder="例如 团队资料库"></label>
      <label class="fld"><span>连接方式</span><select class="sel" name="transport">
        <option ${transport === 'HTTP MCP' ? 'selected' : ''}>HTTP MCP</option>
        <option ${transport === 'stdio MCP' ? 'selected' : ''}>stdio MCP</option>
        <option ${transport === 'OpenAPI' ? 'selected' : ''}>OpenAPI</option>
      </select></label>
      <label class="fld"><span data-target-label>${transport === 'stdio MCP' ? '启动命令' : '连接地址'}</span>
        <input class="inp ${transport === 'stdio MCP' ? 'mono' : ''}" name="target" type="${transport === 'stdio MCP' ? 'text' : 'url'}" value="${escapeHtml(defaultTarget)}" placeholder="${transport === 'stdio MCP' ? 'npx -y @example/mcp-server' : 'https://example.com/mcp'}" required>
      </label>
      <label class="fld" data-args-field ${transport === 'stdio MCP' ? '' : 'hidden'}><span>参数（每行一项）</span>
        <textarea class="ta mono" name="args" placeholder="--read-only">${escapeHtml(prefill?.args || '')}</textarea>
      </label>
    </form>`,
    footer: `<button type="button" class="btn" data-action="close-sheet">取消</button>
      <button type="submit" form="sheet-form" class="btn primary">保存连接器</button>`,
  })
}

function showSkillSheet(winEl) {
  openSheet(winEl, {
    markName: 'puzzle',
    title: '导入技能',
    subtitle: '选择 SKILL.md 文件导入为个人技能。',
    body: `<form id="sheet-form" data-kind="skill">
      <label class="fld"><span>选择 SKILL.md</span><input class="inp" type="file" name="file" accept=".md"></label>
      <label class="fld"><span>显示名称</span><input class="inp" name="name" required maxlength="60" placeholder="例如 周报汇总规范"></label>
      <label class="fld"><span>简介</span><textarea class="ta" name="description" maxlength="180" placeholder="说明该技能适用的对话场景"></textarea></label>
    </form>`,
    footer: `<button type="button" class="btn" data-action="close-sheet">取消</button>
      <button type="submit" form="sheet-form" class="btn primary">导入技能</button>`,
  })
}

function showPluginConfigSheet(winEl, item) {
  const isSearch = item.group === '网页搜索'
  const isFree = item.id === 'p-free'
  openSheet(winEl, {
    markName: itemMark(item),
    title: isSearch ? `配置 ${item.name}` : `${item.name} · 运行边界`,
    subtitle: isSearch ? '网页搜索插件配置' : 'Fox 内置核心工具说明',
    body: isSearch
      ? (isFree
        ? `<p class="lead">免 Key 搜索默认启用，无需配置密钥；当 DuckDuckGo 被拦截或无结果时自动回退至 Yahoo。</p>
          <div class="sh-sec"><h3>适用范围</h3>
            <dl class="facts">
              <div><dt>生效对象</dt><dd>Fox 通用助手与专家（外部 Agent 使用各自搜索能力）</dd></div>
              <div><dt>优先级</dt><dd>未配置 Tavily / Brave / Exa / SearXNG 时作为默认搜索插件</dd></div>
            </dl>
          </div>`
        : `<form id="sheet-form" data-kind="search-plugin" data-id="${escapeHtml(item.id)}">
            <p class="lead">网页搜索已作为插件统一管理。保存配置后将自动启用该搜索服务。</p>
            <label class="fld"><span>${item.id === 'p-searxng' ? '实例地址' : 'API Key'}</span>
              <input class="inp mono" name="secret" type="${item.id === 'p-searxng' ? 'url' : 'password'}" placeholder="${item.id === 'p-searxng' ? 'https://search.example.com' : '输入示例 Key（留空保留已配置）'}">
            </label>
          </form>`)
      : `<p class="lead">${escapeHtml(item.description)}</p>
        <div class="sh-sec"><h3>运行边界</h3>
          <dl class="facts">
            <div><dt>分类</dt><dd>内置工具（独立于外部插件与连接器）</dd></div>
            <div><dt>来源</dt><dd>Fox 内置</dd></div>
            <div><dt>权限策略</dt><dd>遵循当前项目执行方式与全局生命周期 Hook</dd></div>
          </dl>
        </div>`,
    footer: isSearch && !isFree
      ? `<button type="button" class="btn" data-action="close-sheet">取消</button><button type="submit" form="sheet-form" class="btn primary">保存配置</button>`
      : `<button type="button" class="btn primary" data-action="close-sheet">完成</button>`,
  })
}

function showPolicySheet(winEl) {
  openSheet(winEl, {
    markName: 'shield-check',
    title: '插件策略与 Hook',
    subtitle: '跨 Agent 的工具调用拦截与审批规则',
    body: `<p class="lead">插件、网页搜索与连接器的写入操作均受全局安全规则保护。</p>
      <div class="sh-sec"><h3>已启用规则（示例）</h3>
        <div class="scope-card">
          <div class="srow"><div class="st"><b>高风险命令与外部写入审批</b><small>工具调用前 · write_*, send_* · 要求确认</small></div><span class="pill warn">要求审批</span></div>
          <div class="srow"><div class="st"><b>敏感路径保护</b><small>工具调用前 · delete_path · 自动阻止</small></div><span class="pill err">阻止</span></div>
        </div>
      </div>`,
    footer: `<a class="btn" href="../index.html?frame=security-hooks">在权限与安全中管理</a>
      <button type="button" class="btn primary" data-action="close-sheet">关闭</button>`,
  })
}

const EXPERT_AVATAR_CHOICES = [
  'Fox_FrontendDeveloper.png', 'Fox_BackendDeveloper.png', 'Fox_UIDesigner.png',
  'Fox_ProductManager.png', 'Fox_CodeReviewer.png', 'Fox_DataAnalyst.png',
  'Fox_ResearchWriter.png', 'Fox_MeetingNotes.png', 'Fox_SystemsEngineer.png',
  'Fox_SecurityReviewer.png', 'Fox_VisualStoryteller.png', 'Fox_Researcher.png',
]

function showExpertEditorSheet(winEl, editing = null) {
  const chosenAvatar = editing?.avatar || EXPERT_AVATAR_CHOICES[0]
  openSheet(winEl, {
    markName: 'users',
    title: editing ? `编辑 ${editing.name}` : '新建专家',
    subtitle: editing ? '修改专家资料、工作要求与推荐开场提问。' : '创建一位本机可编辑专家，保存后可在详情页绑定技能与资料库。',
    body: `<form id="sheet-form" data-kind="expert" data-editing="${escapeHtml(editing?.id || '')}">
      <input type="hidden" name="avatar" value="${escapeHtml(chosenAvatar)}">
      <div class="fld"><span>头像</span>
        <div class="avatar-picker">
          ${EXPERT_AVATAR_CHOICES.map((file) => `<button type="button" class="avatar-opt ${file === chosenAvatar ? 'on' : ''}" data-action="pick-expert-avatar" data-value="${file}" aria-label="选择头像 ${file}">
            <img src="${EXPERT_AVATAR_BASE}/${file}" alt="">
          </button>`).join('')}
        </div>
      </div>
      <label class="fld"><span>名称</span><input class="inp" name="name" required maxlength="40" value="${escapeHtml(editing?.name || '')}" placeholder="例如 竞品洞察专家"></label>
      <label class="fld"><span>所属领域</span><select class="sel" name="category">
        ${EXPERT_CATEGORIES.filter((c) => c.key !== 'all').map((c) => `<option value="${c.key}" ${(editing?.category || 'planning') === c.key ? 'selected' : ''}>${c.label}</option>`).join('')}
      </select></label>
      <label class="fld"><span>简介</span><textarea class="ta" name="description" required maxlength="200" placeholder="一句话说明该专家最擅长的任务">${escapeHtml(editing?.description || '')}</textarea></label>
      <label class="fld"><span>工作要求（系统提示词）</span><textarea class="ta" name="prompt" placeholder="定义该专家的角色边界、交付标准与输出风格">${escapeHtml(editing?.prompt || '')}</textarea></label>
      <label class="fld"><span>推荐开场提问（每行一条，最多 3 条）</span><textarea class="ta" name="openings" placeholder="例如 帮我梳理本周产品的 3 个核心风险">${escapeHtml((editing?.openings || []).join('\n'))}</textarea></label>
    </form>`,
    footer: `<button type="button" class="btn" data-action="close-sheet">取消</button>
      <button type="submit" form="sheet-form" class="btn primary">${editing ? '保存修改' : '创建专家'}</button>`,
  })
}

function showExpertCapabilitiesSheet(winEl, exp) {
  const sections = [
    ['skills', '技能', allSkills().map((s) => ({ id: s.name, desc: s.description }))],
    ['connectors', '连接器', installedConnectors().map((c) => ({ id: c.name, desc: c.description }))],
    ['knowledge', '资料库', KNOWLEDGE_BASES.map((k) => ({ id: k.id, desc: k.desc }))],
    ['tools', '内置工具', tools.map((t) => ({ id: t.name, desc: t.description }))],
  ]
  openSheet(winEl, {
    markName: 'settings',
    title: `管理 ${exp.name} 的装配能力`,
    subtitle: '启用后将展示在专家详情页的「已装配能力」列表中。',
    body: sections.map(([key, label, items]) => {
      const selected = new Set(exp[key] || [])
      return `<div class="sh-sec"><h3>${label} (${selected.size})</h3>
        <div class="scope-card">
          ${items.map((res) => {
            const on = selected.has(res.id)
            return `<div class="srow">
              <div class="st"><b>${escapeHtml(res.id)}</b><small>${escapeHtml(res.desc)}</small></div>
              <button type="button" class="sw ${on ? 'on' : ''}" role="switch" aria-checked="${on}"
                data-action="toggle-expert-resource-sheet" data-expert="${escapeHtml(exp.id)}" data-res-key="${key}" data-res-id="${escapeHtml(res.id)}"><i></i></button>
            </div>`
          }).join('')}
        </div>
      </div>`
    }).join(''),
    footer: `<button type="button" class="btn primary" data-action="close-sheet">完成</button>`,
  })
}

function showImportExpertSheet(winEl) {
  openSheet(winEl, {
    markName: 'upload',
    title: '导入专家能力包',
    subtitle: '支持 .foxexpert 或 .json 声明式专家包；安装前预览资源与权限。',
    body: `<form id="sheet-form" data-kind="import-expert">
      <label class="fld"><span>选择能力包文件</span><input class="inp" type="file" name="file" accept=".foxexpert,.json"></label>
      <div class="sh-sec"><h3>安全校验说明</h3>
        <dl class="facts">
          <div><dt>校验方式</dt><dd>SHA-256 摘要校验与 Schema 版本检查</dd></div>
          <div><dt>运行边界</dt><dd>仅登记声明式提示词与资源引用，不执行包内脚本</dd></div>
        </dl>
      </div>
    </form>`,
    footer: `<button type="button" class="btn" data-action="close-sheet">取消</button>
      <button type="submit" form="sheet-form" class="btn primary">演示导入示例专家</button>`,
  })
}

/* ---------- Toast ---------- */
let toastTimer
function notify(message) {
  const el = document.querySelector('#toast')
  el.textContent = message
  el.hidden = false
  clearTimeout(toastTimer)
  toastTimer = setTimeout(() => { el.hidden = true }, 2800)
}

/* ---------- Initial Page Render (Doc shell matching 设置页美化/index.html) ---------- */
function renderDoc() {
  const params = new URLSearchParams(location.search)
  const only = params.get('frame')
  const doc = document.querySelector('#doc')
  if (only) {
    document.body.classList.add('single')
    const target = FRAMES.find((f) => f.id === only) || FRAMES[0]
    doc.innerHTML = `
      <div class="prototype-bar"><a href="?">全部原型页面</a><span>交互原型 · 示例数据 · 支持专家/插件/设置全链路切换</span></div>
      ${renderWin(target)}`
    return
  }
  doc.innerHTML = `
    <header class="doc-hero">
      <h1>Fox 伙伴、专家与插件中心 · 交互原型</h1>
      <p>覆盖优化后的「伙伴页面」（含左侧「新建伙伴」弹出菜单）、参考 Grok Bot 居中布局的「新建伙伴」弹窗、宽松留白的「从专家创建」弹窗，「专家中心」与「专家详情页」，以及「插件中心」「插件详情页」「设置 · 插件」。每个画框内均可直接点击交互。</p>
      <nav class="toc" aria-label="原型导航">
        <a class="btn lg" href="连接器目录.html">连接器目录 · ${connectors.length} 项候选</a>
        <select class="sel" name="jump-frame" aria-label="跳转到画面">
          <option value="">跳转到画面</option>
          ${FRAMES.map((f) => `<option value="${f.id}">${f.title}</option>`).join('')}
        </select>
      </nav>
    </header>
    ${FRAMES.map((f) => `
      <section class="frame" id="${f.id}">
        <div class="frame-head">
          <h2>${f.title}</h2>
          <p>${f.desc}</p>
          <a href="?frame=${f.id}">单独预览</a>
        </div>
        ${renderWin(f)}
      </section>`).join('')}`
}

/* ---------- Global Event Delegation ---------- */
document.addEventListener('click', (event) => {
  // Close open dropdown menus when clicking outside their container
  if (!event.target.closest('.acts, .detail-acts, .comp-create-wrap')) {
    FRAMES.forEach((f) => {
      const st = frameState.get(f.id)
      if (st.menuOpen) {
        st.menuOpen = null
        st.preserveScroll = true
        renderFrame(f.id)
      }
    })
  }

  const target = event.target.closest('[data-action]')
  if (!target) return
  const action = target.dataset.action
  const winEl = target.closest('.win')
  const frameId = winEl?.dataset.frame
  const st = frameId ? frameState.get(frameId) : null

  if (action === 'toggle-theme') {
    document.documentElement.classList.toggle('dark')
    notify(document.documentElement.classList.contains('dark') ? '已切换至暖纸 · 深色模式' : '已切换至暖纸 · 浅色模式')
    return
  }
  if (action === 'close-sheet') {
    closeSheet(winEl?.querySelector('.card'))
    return
  }
  if (!st) return

  switch (action) {
    case 'nav-back':
      if (st.historyIdx > 0) {
        st.historyIdx -= 1
        Object.assign(st, st.history[st.historyIdx], { menuOpen: null })
        renderFrame(frameId)
      }
      break
    case 'nav-forward':
      if (st.historyIdx < st.history.length - 1) {
        st.historyIdx += 1
        Object.assign(st, st.history[st.historyIdx], { menuOpen: null })
        renderFrame(frameId)
      }
      break
    case 'go-settings-home':
      st.view = 'settings'
      st.menuOpen = null
      pushHistory(st, { view: 'settings', detailId: null, detailKind: null, detailFrom: 'settings' })
      renderFrame(frameId)
      break
    case 'go-settings-plugins-search':
      st.view = 'settings'
      st.setTab = 'plugin'
      st.menuOpen = null
      pushHistory(st, { view: 'settings', detailId: null, detailKind: null, detailFrom: 'settings' })
      renderFrame(frameId)
      break
    case 'go-directory':
      st.view = 'directory'
      st.menuOpen = null
      pushHistory(st, { view: 'directory', detailId: null, detailKind: null, detailFrom: 'directory' })
      renderFrame(frameId)
      break
    case 'go-directory-kind':
      st.view = 'directory'
      st.dirKind = target.dataset.value || 'connector'
      st.dirGroup = ''
      st.dirStatus = '全部'
      st.dirQuery = ''
      st.expanded.clear()
      st.menuOpen = null
      pushHistory(st, { view: 'directory', detailId: null, detailKind: null, detailFrom: 'directory' })
      renderFrame(frameId)
      break
    case 'noop':
      notify(`当前原型聚焦专家、插件与设置页，「${target.dataset.label || '该入口'}」保留主页面导航示意`)
      break
    case 'go-experts':
      st.view = 'experts'
      st.menuOpen = null
      pushHistory(st, { view: 'experts' })
      renderFrame(frameId)
      break
    case 'go-experts-cat':
      st.view = 'experts'
      st.expCat = target.dataset.value || 'all'
      st.expQuery = ''
      st.menuOpen = null
      pushHistory(st, { view: 'experts' })
      renderFrame(frameId)
      break
    case 'go-new-chat':
      st.view = 'chat'
      st.chatExpertId = null
      st.chatDraft = ''
      st.menuOpen = null
      pushHistory(st, { view: 'chat', chatExpertId: null, chatDraft: '' })
      renderFrame(frameId)
      break
    case 'exp-cat':
      st.expCat = target.dataset.value || 'all'
      renderFrame(frameId)
      break
    case 'exp-reset':
      st.expCat = 'all'
      st.expEnv = 'all'
      st.expAvail = 'all'
      st.expQuery = ''
      renderFrame(frameId)
      break
    case 'open-expert-detail':
      st.view = 'expert-detail'
      st.expertId = target.dataset.value
      st.expDetailTab = 'all'
      st.menuOpen = null
      pushHistory(st, { view: 'expert-detail', expertId: st.expertId })
      renderFrame(frameId)
      break
    case 'exp-detail-tab':
      st.expDetailTab = target.dataset.value
      st.preserveScroll = true
      renderFrame(frameId)
      break
    case 'summon-expert': {
      const exp = findExpertById(target.dataset.value)
      st.view = 'chat'
      st.chatExpertId = exp.id
      st.chatDraft = ''
      st.menuOpen = null
      pushHistory(st, { view: 'chat', chatExpertId: exp.id, chatDraft: '' })
      renderFrame(frameId)
      notify(`已召唤「${exp.name}」并跳转至新对话`)
      break
    }
    case 'summon-expert-prompt': {
      const exp = findExpertById(target.dataset.value)
      const draft = target.dataset.text || ''
      st.view = 'chat'
      st.chatExpertId = exp.id
      st.chatDraft = draft
      st.menuOpen = null
      pushHistory(st, { view: 'chat', chatExpertId: exp.id, chatDraft: draft })
      renderFrame(frameId)
      notify(`已召唤「${exp.name}」并填入推荐提问`)
      break
    }
    case 'fill-chat-draft':
      st.chatDraft = target.dataset.text || ''
      renderFrame(frameId)
      break
    case 'detach-chat-expert':
      st.chatExpertId = null
      renderFrame(frameId)
      notify('已移除对话中的专家')
      break
    case 'send-chat': {
      const exp = st.chatExpertId ? findExpertById(st.chatExpertId) : null
      const msg = (st.chatDraft || '').trim()
      if (!msg) {
        notify('请先输入消息或点击上方推荐提问')
        break
      }
      st.chatDraft = ''
      renderFrame(frameId)
      notify(exp ? `已发送给「${exp.name}」：「${msg.slice(0, 22)}${msg.length > 22 ? '…' : ''}」` : '已发送消息')
      break
    }
    case 'toggle-expert-resource': {
      const { expert: expertId, resKey, resId } = target.dataset
      const exp = findExpertById(expertId)
      if (!exp || exp.env === 'knowledge') break
      const current = new Set(exp[resKey] || [])
      if (current.has(resId)) current.delete(resId)
      else current.add(resId)
      demo.expertOverrides = demo.expertOverrides || {}
      demo.expertOverrides[expertId] = { ...(demo.expertOverrides[expertId] || {}), [resKey]: [...current] }
      saveDemo()
      renderAllFrames()
      notify(`已更新「${exp.name}」的${resId}绑定状态`)
      break
    }
    case 'open-expert-caps': {
      const exp = findExpertById(target.dataset.value)
      showExpertCapabilitiesSheet(winEl, exp)
      break
    }
    case 'toggle-expert-resource-sheet': {
      const { expert: expertId, resKey, resId } = target.dataset
      const exp = findExpertById(expertId)
      if (!exp || exp.env === 'knowledge') break
      const current = new Set(exp[resKey] || [])
      if (current.has(resId)) current.delete(resId)
      else current.add(resId)
      demo.expertOverrides = demo.expertOverrides || {}
      demo.expertOverrides[expertId] = { ...(demo.expertOverrides[expertId] || {}), [resKey]: [...current] }
      saveDemo()
      renderAllFrames()
      showExpertCapabilitiesSheet(document.querySelector(`.win[data-frame="${frameId}"]`), findExpertById(expertId))
      notify(`已更新「${exp.name}」的装配能力`)
      break
    }
    case 'create-companion-from-expert': {
      const exp = findExpertById(target.dataset.value)
      st.view = 'companions'
      st.createCompModal = 'expert'
      st.createCompExpertId = exp.id
      st.menuOpen = null
      renderFrame(frameId)
      break
    }
    case 'go-companions':
      st.view = 'companions'
      st.menuOpen = null
      pushHistory(st, { view: 'companions', compSection: st.compSection || 'inbox' })
      renderFrame(frameId)
      break
    case 'comp-section':
      st.view = 'companions'
      st.compSection = target.dataset.value || 'inbox'
      st.compShowArchived = false
      renderFrame(frameId)
      break
    case 'open-companion-chat':
      st.view = 'companions'
      st.compSection = 'chat'
      st.compActiveId = target.dataset.value
      st.compShowArchived = false
      renderFrame(frameId)
      break
    case 'toggle-comp-archived':
      st.compShowArchived = !st.compShowArchived
      renderFrame(frameId)
      break
    case 'restore-companion': {
      const comp = (demo.companions || []).find((c) => c.id === target.dataset.value)
      if (comp) {
        comp.archived = false
        comp.preview = '已恢复伙伴，随时可继续对话'
        saveDemo()
        st.compShowArchived = false
        st.compSection = 'chat'
        st.compActiveId = comp.id
        renderAllFrames()
        notify(`已恢复伙伴「${comp.name}」`)
      }
      break
    }
    case 'toggle-comp-work': {
      const comp = (demo.companions || []).find((c) => c.id === target.dataset.value)
      if (comp && !comp.isDefault) {
        comp.inWork = !comp.inWork
        saveDemo()
        renderAllFrames()
        notify(comp.inWork ? `已将「${comp.name}」固定到工作页常用伙伴` : `已从工作页快捷栏移除「${comp.name}」`)
      }
      break
    }
    case 'resolve-inbox': {
      const item = (demo.inbox || []).find((i) => i.id === target.dataset.value)
      if (item) {
        item.resolved = true
        const comp = findCompanion(item.companionId)
        if (comp && comp.status === 'action') {
          comp.status = 'idle'
          comp.preview = target.dataset.decision === 'approve' ? '已按你的批准继续执行中…' : '已取消该操作'
        }
        saveDemo()
        renderAllFrames()
        notify(target.dataset.decision === 'approve' ? '已批准该伙伴执行操作' : '已拒绝该操作')
      }
      break
    }
    case 'send-comp-chat': {
      const text = (st.compChatDraft || '').trim()
      if (!text) { notify('请输入要发送的消息'); break }
      const comp = findCompanion(st.compActiveId)
      comp.preview = `我：${text}`
      comp.time = '刚刚'
      st.compChatDraft = ''
      saveDemo()
      renderAllFrames()
      notify(`已发送给「${comp.name}」`)
      break
    }
    case 'toggle-comp-task': {
      const task = (demo.tasks || []).find((t) => t.id === target.dataset.value)
      if (task) {
        task.enabled = !task.enabled
        task.lastResult = task.enabled ? '已恢复计划' : '已暂停'
        saveDemo()
        renderAllFrames()
        notify(task.enabled ? `已启用定时任务「${task.name}」` : `已暂停定时任务「${task.name}」`)
      }
      break
    }
    case 'run-comp-task': {
      const task = (demo.tasks || []).find((t) => t.id === target.dataset.value)
      if (task) {
        const comp = findCompanion(task.companionId)
        task.lastResult = '已触发执行 · 刚刚'
        saveDemo()
        renderAllFrames()
        notify(`已通知「${comp.name}」立即执行「${task.name}」`)
      }
      break
    }
    case 'open-notifications': {
      if (st.view !== 'notifications') {
        st.notifReturn = { view: st.view, compSection: st.compSection, compActiveId: st.compActiveId, detailId: st.detailId, detailKind: st.detailKind, expertId: st.expertId }
        st.notifSidebar = target.dataset.sidebar || (st.view === 'companions' ? 'companions' : 'main')
      }
      st.view = 'notifications'
      st.menuOpen = null
      pushHistory(st, { view: 'notifications', notifSidebar: st.notifSidebar })
      renderFrame(frameId)
      break
    }
    case 'close-notifications': {
      const back = st.notifReturn || { view: st.notifSidebar === 'companions' ? 'companions' : 'experts', compSection: 'inbox' }
      Object.assign(st, back, { menuOpen: null })
      renderFrame(frameId)
      break
    }
    case 'notif-filter':
      st.notifFilter = target.dataset.value || 'all'
      st.preserveScroll = true
      renderFrame(frameId)
      break
    case 'notif-toggle-expand': {
      const id = target.dataset.value
      st.notifExpanded = { ...(st.notifExpanded || {}), [id]: !st.notifExpanded?.[id] }
      const item = (demo.notifications || []).find((n) => n.id === id)
      if (item && item.unread) {
        item.unread = false
        saveDemo()
      }
      st.preserveScroll = true
      renderAllFrames()
      break
    }
    case 'notif-mark-all':
      (demo.notifications || []).forEach((n) => { n.unread = false })
      saveDemo()
      renderAllFrames()
      notify('已将全部通知标记为已读')
      break
    case 'notif-clear-read':
      demo.notifications = (demo.notifications || []).filter((n) => n.unread)
      saveDemo()
      renderAllFrames()
      notify('已清除全部已读通知')
      break
    case 'notif-action': {
      const item = (demo.notifications || []).find((n) => n.id === target.dataset.value)
      if (!item) break
      item.unread = false
      saveDemo()
      if (item.actionType === 'inbox') {
        st.view = 'companions'
        st.compSection = 'inbox'
      } else if (item.actionType === 'companion' && item.targetId) {
        st.view = 'companions'
        st.compSection = 'chat'
        st.compActiveId = item.targetId
      } else if (item.actionType === 'settings-plugin') {
        st.view = 'settings'
        st.setTab = 'plugin'
      } else if (item.actionType === 'experts') {
        st.view = 'experts'
      }
      renderAllFrames()
      notify(`已打开「${item.title}」关联页面`)
      break
    }
    case 'open-new-comp-task':
      st.menuOpen = null
      st.createTaskModal = true
      st.taskCompanionId = (demo.companions || []).find((c) => !c.archived)?.id || 'cmp-default'
      st.taskName = ''
      st.taskSchedule = TASK_SCHEDULES[0]
      st.taskObjective = ''
      renderFrame(frameId)
      break
    case 'pick-task-companion':
      st.taskCompanionId = target.dataset.value
      renderFrame(frameId)
      break
    case 'pick-task-schedule':
      st.taskSchedule = target.dataset.value
      renderFrame(frameId)
      break
    case 'close-task-modal':
      st.createTaskModal = false
      renderFrame(frameId)
      break
    case 'close-task-modal-scrim':
      if (event.target === target) {
        st.createTaskModal = false
        renderFrame(frameId)
      }
      break
    case 'confirm-create-task': {
      const name = (st.taskName || '').trim() || '每日工作巡检'
      const objective = (st.taskObjective || '').trim() || '按设定周期自动整理关键进展并发送到私聊。'
      const schedule = st.taskSchedule || TASK_SCHEDULES[0]
      const companionId = st.taskCompanionId || 'cmp-default'
      demo.tasks = [{
        id: `tsk-${Date.now()}`,
        companionId,
        name,
        objective,
        schedule,
        nextRun: schedule,
        lastResult: '尚未运行 · 刚创建',
        status: 'ok',
        enabled: true,
      }, ...(demo.tasks || [])]
      st.createTaskModal = false
      st.view = 'companions'
      st.compSection = 'tasks'
      saveDemo()
      renderAllFrames()
      notify(`已创建定时任务「${name}」`)
      break
    }
    case 'toggle-comp-create-menu':
      st.menuOpen = st.menuOpen === 'comp-create' ? null : 'comp-create'
      st.preserveScroll = true
      renderFrame(frameId)
      break
    case 'open-create-companion':
      st.menuOpen = null
      st.createCompModal = target.dataset.mode || 'blank'
      st.createCompName = ''
      st.createCompDuty = ''
      st.createCompAvatar = DEFAULT_AVATARS[0]
      st.createCompExpertCat = 'all'
      st.createCompExpertQuery = ''
      renderFrame(frameId)
      break
    case 'close-comp-modal':
      st.createCompModal = null
      renderFrame(frameId)
      break
    case 'close-comp-modal-scrim':
      if (event.target === target) {
        st.createCompModal = null
        renderFrame(frameId)
      }
      break
    case 'apply-comp-preset': {
      const preset = COMPANION_PRESETS[Number(target.dataset.idx)]
      if (preset) {
        st.createCompName = preset.name
        st.createCompDuty = preset.duty
        renderFrame(frameId)
      }
      break
    }
    case 'pick-comp-avatar':
      st.createCompAvatar = target.dataset.value
      renderFrame(frameId)
      break
    case 'pick-comp-expert':
      st.createCompExpertId = target.dataset.value
      renderFrame(frameId)
      break
    case 'confirm-create-companion': {
      const id = `cmp-${Date.now()}`
      if (st.createCompModal === 'blank') {
        const name = (st.createCompName || '').trim() || `小狐伙伴 ${(demo.companions?.length || 0) + 1}`
        const duty = (st.createCompDuty || '').trim() || '协助日常事务跟进、文档整理与信息汇总。'
        demo.companions.unshift({
          id, name, avatar: `${ROOT}/avatars/defaults/${st.createCompAvatar || DEFAULT_AVATARS[0]}`,
          duty, pinned: false, inWork: false, status: 'idle', time: '刚刚',
          preview: '你好！我已经准备好协助你开展工作。', model: '跟随默认模型', perm: '操作前询问', dir: `D:\\Fox\\companions\\${id}`,
        })
        st.createCompModal = null
        st.compSection = 'chat'
        st.compActiveId = id
        saveDemo()
        renderAllFrames()
        notify(`已创建伙伴「${name}」并打开私聊`)
      } else {
        const exp = findExpertById(st.createCompExpertId)
        demo.companions.unshift({
          id, name: exp.name, avatar: expertAvatarSrc(exp),
          duty: exp.description, pinned: false, inWork: false, status: 'idle', time: '刚刚',
          preview: `已基于「${exp.name}」创建独立伙伴，随时开始协作。`, model: exp.model || '跟随默认模型', perm: '操作前询问', dir: `D:\\Fox\\companions\\${exp.id}`,
        })
        st.createCompModal = null
        st.compSection = 'chat'
        st.compActiveId = id
        saveDemo()
        renderAllFrames()
        notify(`已从专家创建伙伴「${exp.name}」`)
      }
      break
    }
    case 'open-create-expert':
      showExpertEditorSheet(winEl, null)
      break
    case 'open-edit-expert': {
      const exp = findExpertById(target.dataset.value)
      st.menuOpen = null
      st.preserveScroll = true
      renderFrame(frameId)
      showExpertEditorSheet(document.querySelector(`.win[data-frame="${frameId}"]`), exp)
      break
    }
    case 'open-import-expert':
      showImportExpertSheet(winEl)
      break
    case 'pick-expert-avatar': {
      const form = target.closest('form')
      if (form) {
        form.elements.avatar.value = target.dataset.value
        form.querySelectorAll('.avatar-opt').forEach((btn) => btn.classList.toggle('on', btn === target))
      }
      break
    }
    case 'copy-expert': {
      const orig = findExpertById(target.dataset.value)
      const id = `custom-exp-${Date.now()}`
      const copy = {
        ...orig,
        id,
        name: `${orig.name} 副本`,
        env: 'local',
        source: '自定义创建',
        custom: true,
        recentRuns: 0,
      }
      demo.customExperts = [copy, ...(demo.customExperts || [])]
      saveDemo()
      st.expertId = id
      st.menuOpen = null
      renderAllFrames()
      notify(`已复制为「${copy.name}」`)
      break
    }
    case 'export-expert':
      st.menuOpen = null
      st.preserveScroll = true
      renderFrame(frameId)
      notify(`已导出「${target.dataset.value}」能力包 (.foxexpert)`)
      break
    case 'delete-expert': {
      const id = target.dataset.value
      demo.customExperts = (demo.customExperts || []).filter((e) => e.id !== id)
      delete demo.expertOverrides?.[id]
      saveDemo()
      st.view = 'experts'
      st.menuOpen = null
      renderAllFrames()
      notify('已删除自定义专家')
      break
    }
    case 'set-tab':
      st.setTab = target.dataset.value
      st.setQuery = ''
      st.menuOpen = null
      renderFrame(frameId)
      break
    case 'dir-kind':
      st.dirKind = target.dataset.value
      st.dirGroup = ''
      st.dirStatus = '全部'
      st.dirQuery = ''
      st.expanded.clear()
      renderFrame(frameId)
      break
    case 'dir-reset':
      st.dirGroup = ''
      st.dirStatus = '全部'
      st.dirQuery = ''
      renderFrame(frameId)
      break
    case 'expand':
      if (st.expanded.has(target.dataset.value)) st.expanded.delete(target.dataset.value)
      else st.expanded.add(target.dataset.value)
      st.preserveScroll = true
      renderFrame(frameId)
      break
    case 'open-detail': {
      const from = st.view === 'directory' ? 'directory' : 'settings'
      st.view = 'detail'
      st.detailId = target.dataset.value
      st.detailKind = target.dataset.kind || st.dirKind
      st.detailFrom = from
      st.menuOpen = null
      pushHistory(st, { view: 'detail', detailId: st.detailId, detailKind: st.detailKind, detailFrom: from })
      renderFrame(frameId)
      break
    }
    case 'toggle-enabled': {
      const id = target.dataset.value
      const next = !demo.enabledMap[id]
      demo.enabledMap[id] = next
      saveDemo()
      renderAllFrames()
      notify(next ? '已启用该扩展' : '已停用该扩展')
      break
    }
    case 'quick-shortlist': {
      const id = target.dataset.value
      const picked = demo.shortlist.includes(id)
      demo.shortlist = picked ? demo.shortlist.filter((x) => x !== id) : [...demo.shortlist, id]
      saveDemo()
      renderAllFrames()
      notify(picked ? '已移出候选清单' : '已加入候选清单')
      break
    }
    case 'toggle-skill-scope': {
      const { item: itemId, agent } = target.dataset
      const current = demo.scopes[itemId] || []
      demo.scopes[itemId] = current.includes(agent) ? current.filter((a) => a !== agent) : [...current, agent]
      demo.enabledMap[itemId] = demo.scopes[itemId].length > 0
      saveDemo()
      renderAllFrames()
      notify(`已更新 ${agent} 的技能范围`)
      break
    }
    case 'toggle-add-menu':
      st.menuOpen = st.menuOpen === 'add' ? null : 'add'
      st.preserveScroll = true
      renderFrame(frameId)
      break
    case 'toggle-detail-menu':
      st.menuOpen = st.menuOpen === 'detail' ? null : 'detail'
      st.preserveScroll = true
      renderFrame(frameId)
      break
    case 'open-add-connector':
      st.menuOpen = null
      st.preserveScroll = true
      renderFrame(frameId)
      showConnectorSheet(document.querySelector(`.win[data-frame="${frameId}"]`))
      break
    case 'open-add-skill':
      st.menuOpen = null
      st.preserveScroll = true
      renderFrame(frameId)
      showSkillSheet(document.querySelector(`.win[data-frame="${frameId}"]`))
      break
    case 'open-policy':
      st.menuOpen = null
      st.preserveScroll = true
      renderFrame(frameId)
      showPolicySheet(document.querySelector(`.win[data-frame="${frameId}"]`))
      break
    case 'open-connect': {
      const found = findItemById(target.dataset.value, 'connector')
      if (found) showConnectorSheet(winEl, found.item)
      break
    }
    case 'open-manage': {
      const kind = target.dataset.kind
      if (kind === 'connector') {
        const cfg = installedConnectors().find((c) => c.id === target.dataset.value)
        if (cfg === office) {
          showPluginConfigSheet(winEl, { ...office, group: '内置工具' })
        } else if (cfg) {
          showConnectorSheet(winEl, cfg)
        }
      } else {
        const found = findItemById(target.dataset.value, kind)
        if (found) showPluginConfigSheet(winEl, found.item)
      }
      break
    }
    case 'remove-connector': {
      const id = target.dataset.value
      demo.connectors = demo.connectors.filter((c) => c.id !== id)
      delete demo.enabledMap[id]
      st.menuOpen = null
      saveDemo()
      renderAllFrames()
      notify('已移除连接器配置')
      break
    }
    case 'try-item':
      notify(`已在新对话中挂载「${target.dataset.value}」，可直接开始提问`)
      break
    case 'run-prompt': {
      const text = target.dataset.text || ''
      notify(`已填入示例指令：「${text.slice(0, 26)}${text.length > 26 ? '…' : ''}」`)
      break
    }
    case 'copy-info':
      st.menuOpen = null
      st.preserveScroll = true
      renderFrame(frameId)
      notify(`已复制「${target.dataset.value}」的接入信息`)
      break
    case 'test-connection':
      st.menuOpen = null
      st.preserveScroll = true
      renderFrame(frameId)
      notify('连接测试通过 · 延迟 38 ms')
      break
  }
})

document.addEventListener('input', (event) => {
  const input = event.target
  const winEl = input.closest('.win')
  if (!winEl) return
  const frameId = winEl.dataset.frame
  const st = frameState.get(frameId)
  if (!st) return
  if (input.dataset.input === 'set-query') {
    st.setQuery = input.value
    const pos = input.selectionStart
    st.preserveScroll = true
    renderFrame(frameId)
    const next = document.querySelector(`.win[data-frame="${frameId}"] [data-input="set-query"]`)
    if (next) { next.focus(); next.setSelectionRange(pos, pos) }
  } else if (input.dataset.input === 'dir-query') {
    st.dirQuery = input.value
    const pos = input.selectionStart
    st.preserveScroll = true
    renderFrame(frameId)
    const next = document.querySelector(`.win[data-frame="${frameId}"] [data-input="dir-query"]`)
    if (next) { next.focus(); next.setSelectionRange(pos, pos) }
  } else if (input.dataset.input === 'exp-query') {
    st.expQuery = input.value
    const pos = input.selectionStart
    st.preserveScroll = true
    renderFrame(frameId)
    const next = document.querySelector(`.win[data-frame="${frameId}"] [data-input="exp-query"]`)
    if (next) { next.focus(); next.setSelectionRange(pos, pos) }
  } else if (input.dataset.input === 'chat-draft') {
    st.chatDraft = input.value
  } else if (input.dataset.input === 'comp-chat-draft') {
    st.compChatDraft = input.value
  } else if (input.dataset.input === 'create-comp-name') {
    st.createCompName = input.value
    const trimmed = input.value.trim()
    const matched = COMPANION_PRESETS.find((p) => trimmed && trimmed === p.name)
    winEl.querySelectorAll('.create-preset-chips .preset-chip').forEach((chip, idx) => {
      chip.classList.toggle('on', Boolean(matched && COMPANION_PRESETS[idx]?.name === matched.name))
    })
    const hint = winEl.querySelector('.create-preset-hint')
    if (hint) hint.textContent = matched ? matched.duty : '也可直接点选上方快捷角色模板快速起名'
  } else if (input.dataset.input === 'create-comp-exp-query') {
    st.createCompExpertQuery = input.value
    const pos = input.selectionStart
    renderFrame(frameId)
    const next = document.querySelector(`.win[data-frame="${frameId}"] [data-input="create-comp-exp-query"]`)
    if (next) { next.focus(); next.setSelectionRange(pos, pos) }
  } else if (input.dataset.input === 'task-name') {
    st.taskName = input.value
  } else if (input.dataset.input === 'task-objective') {
    st.taskObjective = input.value
  }
})

document.addEventListener('change', (event) => {
  const el = event.target
  if (el.name === 'jump-frame' && el.value) {
    location.hash = el.value
    return
  }
  if (el.name === 'settings-page' && el.value !== 'plugins') {
    location.href = `../index.html?frame=${el.value}`
    return
  }
  if (el.name === 'main-page') {
    const winEl = el.closest('.win')
    const frameId = winEl?.dataset.frame
    const st = frameId ? frameState.get(frameId) : null
    if (st) {
      st.view = el.value === 'settings' ? 'settings' : el.value === 'experts' ? 'experts' : el.value === 'companions' ? 'companions' : el.value === 'chat' ? 'chat' : 'directory'
      renderFrame(frameId)
    }
    return
  }
  if (el.name === 'comp-page') {
    const winEl = el.closest('.win')
    const frameId = winEl?.dataset.frame
    const st = frameId ? frameState.get(frameId) : null
    if (st) {
      st.view = 'companions'
      st.compSection = el.value || 'inbox'
      renderFrame(frameId)
    }
    return
  }
  const winEl = el.closest('.win')
  if (!winEl) return
  const frameId = winEl.dataset.frame
  const st = frameState.get(frameId)
  if (el.dataset.select === 'dir-group') {
    st.dirGroup = el.value
    renderFrame(frameId)
  } else if (el.dataset.select === 'dir-status') {
    st.dirStatus = el.value
    renderFrame(frameId)
  } else if (el.dataset.select === 'exp-env') {
    st.expEnv = el.value
    renderFrame(frameId)
  } else if (el.dataset.select === 'exp-avail') {
    st.expAvail = el.value
    renderFrame(frameId)
  } else if (el.dataset.select === 'exp-cat') {
    st.expCat = el.value
    renderFrame(frameId)
  } else if (el.dataset.select === 'create-comp-exp-cat') {
    st.createCompExpertCat = el.value
    renderFrame(frameId)
  } else if (el.name === 'transport') {
    const form = el.form
    const local = el.value === 'stdio MCP'
    form.querySelector('[data-target-label]').textContent = local ? '启动命令' : '连接地址'
    form.elements.target.type = local ? 'text' : 'url'
    form.elements.target.placeholder = local ? 'npx -y @example/mcp-server' : 'https://example.com/mcp'
    form.elements.target.classList.toggle('mono', local)
    form.querySelector('[data-args-field]').hidden = !local
  }
})

document.addEventListener('submit', (event) => {
  const form = event.target
  if (form.id !== 'sheet-form') return
  event.preventDefault()
  const winEl = form.closest('.win')
  const card = winEl?.querySelector('.card')
  const data = new FormData(form)

  if (form.dataset.kind === 'search-plugin') {
    demo.enabledMap[form.dataset.id] = true
    saveDemo()
    closeSheet(card)
    renderAllFrames()
    notify('已保存网页搜索配置并启用')
    return
  }
  if (form.dataset.kind === 'connector') {
    const name = data.get('name').trim()
    const target = data.get('target').trim()
    const transport = data.get('transport')
    if (!name || !target) return
    const candidateId = form.dataset.candidate || ''
    const editingId = form.dataset.editing || ''
    const idx = demo.connectors.findIndex((c) => (editingId && c.id === editingId) || (candidateId && c.fromCandidate === candidateId))
    const base = candidateId ? connectors.find((c) => c.id === candidateId) : null
    const id = idx >= 0 ? demo.connectors[idx].id : `custom-${Date.now()}`
    const record = {
      id,
      fromCandidate: candidateId || undefined,
      name,
      sub: transport,
      group: '已配置',
      mark: base?.mark || 'cable',
      transport,
      target,
      args: transport === 'stdio MCP' ? (data.get('args') || '').trim() : '',
      description: base?.description || `自定义接入 · ${transport}`,
    }
    if (idx >= 0) demo.connectors[idx] = record
    else demo.connectors.push(record)
    demo.enabledMap[id] = true
    saveDemo()
    closeSheet(card)
    renderAllFrames()
    notify(`已保存「${name}」并启用`)
    return
  }
  if (form.dataset.kind === 'skill') {
    const name = data.get('name').trim()
    if (!name) return
    const id = `skill-${Date.now()}`
    demo.skills.push({
      id,
      name,
      group: '本地技能',
      mark: 'puzzle',
      publisher: '个人',
      scopeLabel: '个人',
      file: data.get('file')?.name || 'SKILL.md',
      description: data.get('description').trim() || '本地导入技能',
    })
    demo.enabledMap[id] = true
    demo.scopes[id] = ['通用助手']
    saveDemo()
    closeSheet(card)
    renderAllFrames()
    notify(`已导入技能「${name}」`)
    return
  }
  if (form.dataset.kind === 'expert') {
    const name = data.get('name').trim()
    const description = data.get('description').trim()
    if (!name || !description) return
    const category = data.get('category') || 'planning'
    const avatar = data.get('avatar') || EXPERT_AVATAR_CHOICES[0]
    const prompt = (data.get('prompt') || '').trim() || `你是${name}，负责${description}`
    const openings = (data.get('openings') || '').split(/\r?\n/).map((s) => s.trim()).filter(Boolean).slice(0, 3)
    const editingId = form.dataset.editing || ''
    if (editingId) {
      demo.expertOverrides = demo.expertOverrides || {}
      demo.expertOverrides[editingId] = {
        ...(demo.expertOverrides[editingId] || {}),
        name, category, avatar, description, prompt,
        openings: openings.length ? openings : [`请作为${name}帮我分析当前任务的核心要点`],
      }
      saveDemo()
      closeSheet(card)
      renderAllFrames()
      notify(`已保存专家「${name}」`)
    } else {
      const id = `custom-exp-${Date.now()}`
      demo.customExperts = [{
        id, name, category, env: 'local', avatar,
        model: '跟随默认模型', version: '1.0.0', source: '自定义创建', custom: true,
        recentRuns: 0, recentProject: 'Fox',
        description, prompt,
        openings: openings.length ? openings : [`请作为${name}帮我梳理这个需求的关键步骤`],
        skills: [], connectors: [], knowledge: [], tools: ['文件工具'],
      }, ...(demo.customExperts || [])]
      saveDemo()
      closeSheet(card)
      const frameId = winEl?.dataset.frame
      if (frameId) {
        const st = frameState.get(frameId)
        st.view = 'expert-detail'
        st.expertId = id
        pushHistory(st, { view: 'expert-detail', expertId: id })
      }
      renderAllFrames()
      notify(`已创建专家「${name}」`)
    }
    return
  }
  if (form.dataset.kind === 'comp-task') {
    const name = data.get('name').trim()
    const objective = data.get('objective').trim()
    const companionId = data.get('companionId')
    const schedule = data.get('schedule') || '每天 09:00'
    if (!name || !objective) return
    demo.tasks = [{
      id: `tsk-${Date.now()}`,
      companionId,
      name,
      objective,
      schedule,
      nextRun: schedule,
      lastResult: '尚未运行 · 刚创建',
      status: 'ok',
      enabled: true,
    }, ...(demo.tasks || [])]
    saveDemo()
    closeSheet(card)
    renderAllFrames()
    notify(`已为伙伴创建定时任务「${name}」`)
    return
  }
  if (form.dataset.kind === 'import-expert') {
    const id = `imported-exp-${Date.now()}`
    demo.customExperts = [{
      id,
      name: '增长运营专家',
      category: 'planning',
      env: 'local',
      avatar: 'Fox_BusinessManager.png',
      model: '跟随默认模型',
      version: '1.0.0',
      source: '导入能力包',
      custom: true,
      recentRuns: 0,
      recentProject: 'Fox',
      description: '从能力包导入的增长运营专家，擅长留存漏斗拆解、转化实验设计与复盘。',
      prompt: '你是增长运营专家，基于漏斗数据定位流失环节并提出可快速验证的 A/B 实验方案。',
      openings: [
        '分析新用户首次召唤专家的转化漏斗，提出 3 个低成本优化实验',
        '帮我设计一份针对核心活跃用户的版本更新公告和引导文案',
      ],
      skills: ['Excel 表格'],
      connectors: ['Google Drive'],
      knowledge: ['用户访谈与反馈库'],
      tools: ['文件工具'],
    }, ...(demo.customExperts || [])]
    saveDemo()
    closeSheet(card)
    renderAllFrames()
    notify('已导入能力包：增长运营专家')
  }
})

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    const openSheetCard = document.querySelector('.card:has(> .sheet)')
    if (openSheetCard) { closeSheet(openSheetCard); return }
    FRAMES.forEach((f) => {
      const st = frameState.get(f.id)
      if (st.createCompModal || st.createTaskModal || st.menuOpen) {
        st.createCompModal = null
        st.createTaskModal = false
        st.menuOpen = null
        renderFrame(f.id)
      }
    })
  }
})

renderDoc()
