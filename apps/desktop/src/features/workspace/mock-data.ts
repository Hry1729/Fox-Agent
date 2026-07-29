export const agentCards = [
  { id: 'general', name: '通用助手', description: '适合日常问答、分析与知识检索', model: 'DeepSeek V3.2', tools: 6, knowledge: 2, skills: 4, recent: '刚刚', image: '/mascot/fox_magic.png', active: true },
  { id: 'research', name: '研究助手', description: '擅长资料检索、总结与来源核对', model: 'Qwen3 Max', tools: 8, knowledge: 4, skills: 5, recent: '12 分钟前', image: '/mascot/fox_search.png' },
  { id: 'document', name: '文档助手', description: '阅读项目文件并整理结构化文档', model: 'DeepSeek V3.2', tools: 5, knowledge: 3, skills: 6, recent: '昨天', image: '/mascot/fox_laptop.png' },
  { id: 'ops', name: '运维助手', description: '辅助排查服务状态与部署问题', model: 'Qwen3 Max', tools: 9, knowledge: 1, skills: 3, recent: '7 月 12 日', image: '/mascot/fox_wrench.png' }
]

export const knowledgeCards = [
  { id: 'fox', name: 'Fox 产品知识库', description: '产品架构、功能说明与设计决策', types: 'PDF 12 · MD 16', files: 28, processed: '28 / 28', progress: 100, agents: 4, recent: '今天 15:42', tone: 'blue' },
  { id: 'knowledge-service', name: '知识库使用手册', description: '部署、配置与 API 参考', types: 'MD 34 · PDF 8', files: 42, processed: '42 / 42', progress: 100, agents: 2, recent: '昨天 19:08', tone: 'violet' },
  { id: 'dev', name: '研发规范', description: '编码规范、评审清单与发布流程', types: 'MD 16', files: 16, processed: '15 / 16', progress: 94, agents: 1, recent: '7 月 10 日', tone: 'green' },
  { id: 'project', name: '项目资料', description: '阶段文档与会议纪要', types: 'DOCX 9 · PDF 10', files: 19, processed: '14 / 19', progress: 74, agents: 2, recent: '7 月 8 日', tone: 'amber' },
  { id: 'archive', name: '历史归档', description: '只读历史资料', types: 'PDF 7', files: 7, processed: '7 / 7', progress: 100, agents: 1, recent: '6 月 21 日', tone: 'gray' }
]

export const knowledgeFiles = [
  { name: 'Fox Agent 总体架构.md', type: 'Markdown', size: '18 KB', updated: '今天 15:42', active: true },
  { name: '首版能力清单.md', type: 'Markdown', size: '12 KB', updated: '今天 14:20' },
  { name: '知识库接入说明.pdf', type: 'PDF', size: '2.4 MB', updated: '昨天' },
  { name: '界面设计决策.md', type: 'Markdown', size: '26 KB', updated: '7 月 12 日' },
  { name: '产品路线图.pdf', type: 'PDF', size: '4.1 MB', updated: '7 月 10 日' }
]
