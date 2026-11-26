export const searchProviders = [
  { id: 'auto', name: '免 Key 搜索', description: 'DuckDuckGo 优先，必要时尝试 Yahoo', mark: 'globe' },
  { id: 'tavily', name: 'Tavily', description: '为 AI 优化的搜索 · 需 API Key', mark: 'T' },
  { id: 'brave', name: 'Brave Search', description: '使用独立索引 · 需 API Key', mark: 'B' },
  { id: 'exa', name: 'Exa', description: '按语义检索网页 · 需 API Key', mark: 'E' },
  { id: 'searxng', name: 'SearXNG', description: '使用自建搜索实例 · 需实例地址', mark: 'S' },
] as const
