import type { PluginCardView } from './model'
import type { PluginSection } from './plugin-browser-data'

type ExamplePrompts = [string, string, string]

const pluginExamples: Record<string, ExamplePrompts> = {
  'mcp:fox-catalog-gmail': [
      '总结最近 5 封关于「项目评审」的邮件，提炼已确认的结论、遗留问题和下一步跟进事项',
      '帮我起草一封礼貌而明确的邮件回复给客户，列出我们本周将交付的 3 项清单',
      '把最近的线上反馈邮件串整理成行动追踪表，标明负责人、截止日期和对应邮件摘要',
  ],
  'mcp:fox-catalog-github': [
      '汇总本周主仓库合并的 PR，按新功能、修复和优化整理一份发版说明',
      '查看最近失败的 GitHub Actions 构建日志，定位报错原因并给出修复建议',
      '检索所有带 bug 标签且超过 7 天未回复的 Issue，按优先级排个处理顺序',
  ],
  'mcp:fox-catalog-feishu': [
      '搜索飞书知识库里最新的《Q4 产品路线图》，提炼各模块的关键里程碑',
      '读取需求池多维表格中状态为「进行中」的条目，按负责人汇总进度风险',
      '查看我明天的飞书日程，为下午的评审会生成一份 10 分钟议程提纲',
  ],
  'mcp:fox-catalog-linear': [
      '统计当前 Cycle 中未完成的高优先级 Issue，标出可能延期的阻塞项',
      '把刚才的架构讨论结论拆分成 4 个 Linear 任务，补全验收标准',
      '查找我名下处于 In Review 的工单，整理一份今日站会同步内容',
  ],
  'mcp:fox-catalog-drive': [
      '在 Google Drive 中查找最近更新的年度预算文件，提炼核心开销变化',
      '对比共享文件夹里两个版本的合作方案，列出条款差异清单',
      '整理「客户案例」目录下的资料，生成一份面向销售团队的速查摘要',
  ],
  'mcp:fox-catalog-figma': [
      '解析当前 Figma 页面的间距、圆角与配色变量，对照 Fox 主题 Token 列出差异',
      '读取设置页设计稿中的表单组件状态，生成一份前端验收清单',
      '检查这个 Figma 组件库的命名规范，列出与现有代码不一致的组件',
  ],
  'mcp:fox-office': [
      '读取当前目录下的季度汇报.docx，检查格式统一性并输出修订建议',
      '汇总 3 个部门的预算.xlsx，生成一张按科目归类的对比表',
      '根据项目方案大纲生成一份 10 页结构的演示文稿框架',
  ],
  'search:tavily': [
      '联网查询本周发布的最新开源模型基准评测结果，整理对比表',
      '检索 MCP 协议近期关于 OAuth 授权规范的更新要点并附上原文链接',
      '查找主流桌面 AI 工具的插件管理交互方案，总结共性设计',
  ],
}

const categoryExamples: Record<string, (name: string) => ExamplePrompts> = {
  '即时通讯': name => [
      `汇总 ${name} 中今天与「发版」相关的关键消息，整理出待办事项和负责人`,
      `搜索 ${name} 里最近一周关于客户需求的讨论，提炼高频反馈`,
      `起草一条清晰简短的项目进度同步消息，准备发送到 ${name} 频道`,
  ],
  '文档与网盘': name => [
      `在 ${name} 中查找最新的产品需求文档，提炼核心目标与变更点`,
      `整理 ${name} 指定目录下的资料清单，按主题归类并生成目录摘要`,
      `读取 ${name} 中的会议记录，提取所有需要本周跟进的行动项`,
  ],
  '项目与研发': name => [
      `查看 ${name} 中当前迭代未关闭的高优问题，梳理阻塞原因`,
      `总结 ${name} 最近的变更与部署状态，生成一份团队同步简报`,
      `围绕本次需求检索 ${name} 里的历史上下文，避免遗漏依赖项`,
  ],
  '邮件与日程': name => [
      `查看 ${name} 中本周的关键会议与往来邮件，整理每日重点清单`,
      `汇总最近与合作方在 ${name} 中的沟通要点，起草跟进回复`,
      `核对我明天的空闲时段，推荐两个适合安排 45 分钟评审的时间窗口`,
  ],
  '设计与经营': name => [
      `从 ${name} 读取最新的项目资产与状态，整理成跨团队同步摘要`,
      `按优先级汇总 ${name} 中的近期客户记录，标出需要重点跟进的条目`,
      `核对 ${name} 里的核心指标与变更记录，生成一份周度简报`,
  ],
  '数据与检索': name => [
      `通过 ${name} 查询最新相关信息，整理成带来源引用的要点摘要`,
      `检查 ${name} 中的核心数据表或检索结果，输出关键趋势对比`,
      `围绕「行业动态」用 ${name} 执行定向检索，过滤重复内容后汇总`,
  ],
  '网页搜索': name => [
      `使用 ${name} 检索近期关于 MCP 协议更新的官方公告并总结要点`,
      `通过 ${name} 对比主流桌面端 AI 应用的插件管理体验，附来源链接`,
      `查询最新版本的 API 变更说明，核对当前配置是否需要调整`,
  ],
  '办公技能': name => [
      `使用 ${name} 技能把这份方案大纲整理为正式可交付文档`,
      `按团队排版规范检查现有文件并输出修改建议`,
      `根据数据摘要生成结构清晰的汇报内容`,
  ],
  '本地技能': name => [
      `启用 ${name} 技能处理本次任务并按约定格式输出结论`,
      `用 ${name} 技能复核当前草稿，指出遗漏项与改进点`,
      `按 ${name} 的步骤清单拆解并执行接下来的工作`,
  ],
}

export function pluginExamplePrompts(card: PluginCardView, section: PluginSection): ExamplePrompts {
  const category = section === 'plugin' ? '网页搜索'
    : section === 'skill' ? (card.id.startsWith('skill:fox-office-') ? '办公技能' : '本地技能')
      : card.category
  return pluginExamples[card.id] ?? (categoryExamples[category] ?? categoryExamples['数据与检索'])(card.name)
}
