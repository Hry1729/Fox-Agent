import type {
  PluginActivation,
  PluginActivationUpdate,
  PluginAgentScopeDTO,
  PluginCardView,
  PluginCatalogDTO,
  PluginCatalogQuery,
  PluginGateway,
  PluginInstallationsDTO,
  PluginOperationDTO,
  PluginOperationStage,
} from './model'

const defaultAgentScope: PluginAgentScopeDTO[] = [
  { agentId: 'fox-assistant', agentName: 'Fox 助手', enabled: false },
  { agentId: 'research-expert', agentName: '研究专家', enabled: true },
  { agentId: 'coding-expert', agentName: '代码专家', enabled: true },
]

function card(
  value: Omit<PluginCardView, 'installedAt' | 'updatedAt'>,
): PluginCardView {
  return { ...value, permissions: [...value.permissions] }
}

export function createMockPluginCatalog(): PluginCardView[] {
  return [
    card({
      id: 'mcp-filesystem',
      kind: 'mcp',
      origin: 'builtin',
      category: '文件与办公',
      name: '文件系统',
      description: '安全访问当前工作区中的文件和目录。',
      icon: 'folder',
      version: '1.2.0',
      installStatus: 'installed',
      activation: { mode: 'global', enabled: true },
      runtimeStatus: 'healthy',
      permissions: ['workspace.read', 'workspace.write'],
      compatible: true,
    }),
    card({
      id: 'mcp-browser',
      kind: 'mcp',
      origin: 'official',
      category: '信息获取',
      name: '浏览器访问',
      description: '让助手访问公开网页并提取页面内容。',
      icon: 'globe',
      version: '0.9.1',
      installStatus: 'not_installed',
      activation: { mode: 'global', enabled: false },
      runtimeStatus: 'unknown',
      permissions: ['network.public'],
      compatible: true,
    }),
    card({
      id: 'mcp-knowledge-remote',
      kind: 'mcp',
      origin: 'local',
      category: '知识库',
      name: '远程知识库连接',
      description: '连接已配置的知识库服务，使用远程检索能力。',
      icon: 'database',
      version: '1.0.0',
      installStatus: 'installed',
      activation: { mode: 'global', enabled: false },
      runtimeStatus: 'degraded',
      permissions: ['knowledge.remote'],
      compatible: true,
    }),
    card({
      id: 'skill-browser-automation',
      kind: 'skill',
      origin: 'official',
      category: '效率工具',
      name: 'BrowserSkill',
      description: '把网页浏览、表单填写和页面信息整理成可复用工作流。',
      icon: 'sparkles',
      version: '2.1.0',
      installStatus: 'installed',
      activation: { mode: 'per_agent', enabledAgentCount: 2 },
      permissions: ['network.public'],
      compatible: true,
    }),
    card({
      id: 'skill-pdf-extractor',
      kind: 'skill',
      origin: 'builtin',
      category: '文档处理',
      name: 'PDF 文档提取',
      description: '从 PDF 中提取正文、表格和页面结构，便于后续分析。',
      icon: 'file-text',
      version: '1.0.0',
      installStatus: 'installed',
      activation: { mode: 'per_agent', enabledAgentCount: 1 },
      permissions: ['workspace.read'],
      compatible: true,
    }),
    card({
      id: 'skill-meeting-notes',
      kind: 'skill',
      origin: 'community',
      category: '效率工具',
      name: '会议纪要整理',
      description: '将长文本整理为议题、结论、待办和风险清单。',
      icon: 'notebook',
      version: '0.6.0',
      installStatus: 'not_installed',
      activation: { mode: 'per_agent', enabledAgentCount: 0 },
      permissions: [],
      compatible: true,
    }),
    card({
      id: 'tool-file-read',
      kind: 'tool',
      origin: 'builtin',
      category: '文件与办公',
      name: '读取文件',
      description: '读取工作区中的文本文件、目录结构和附件内容，帮助助手理解现有资料。',
      icon: 'file',
      installStatus: 'installed',
      activation: { mode: 'global', enabled: true },
      permissions: ['workspace.read'],
      compatible: true,
    }),
    card({
      id: 'tool-terminal',
      kind: 'tool',
      origin: 'builtin',
      category: '开发工具',
      name: '终端命令',
      description: '在已授权的项目目录中运行非交互命令，用于构建、测试和自动化操作。',
      icon: 'terminal',
      installStatus: 'installed',
      activation: { mode: 'global', enabled: false },
      permissions: ['process.execute'],
      compatible: true,
    }),
    card({
      id: 'tool-local-knowledge',
      kind: 'tool',
      origin: 'builtin',
      category: '知识库',
      name: '本地知识检索',
      description: '在本地知识库中检索相关片段并读取原文，为模型回答提供依据。',
      icon: 'database',
      installStatus: 'installed',
      activation: { mode: 'not_applicable' },
      permissions: ['knowledge.local'],
      compatible: true,
    }),
  ]
}

function cloneActivation(activation: PluginActivation): PluginActivation {
  return activation.mode === 'global'
    ? { mode: 'global', enabled: activation.enabled }
    : activation.mode === 'per_agent'
      ? { mode: 'per_agent', enabledAgentCount: activation.enabledAgentCount }
      : { mode: 'not_applicable' }
}

function cloneCard(value: PluginCardView): PluginCardView {
  return {
    ...value,
    activation: cloneActivation(value.activation),
    permissions: [...value.permissions],
  }
}

function cloneOperation(value: PluginOperationDTO): PluginOperationDTO {
  return { ...value }
}

function operationStage(operation: PluginOperationDTO['operation']): PluginOperationStage {
  if (operation === 'install') return 'downloading'
  if (operation === 'update') return 'verifying'
  if (operation === 'uninstall') return 'registering'
  return 'resolving'
}

export class MockPluginGateway implements PluginGateway {
  private readonly catalog: PluginCardView[]
  private readonly operations = new Map<string, PluginOperationDTO>()
  private readonly scopes = new Map<string, PluginAgentScopeDTO[]>()
  private sequence = 0
  private readonly now: () => number

  constructor(options: { catalog?: PluginCardView[]; now?: () => number } = {}) {
    this.catalog = (options.catalog ?? createMockPluginCatalog()).map(cloneCard)
    this.now = options.now ?? (() => Date.now())
    this.scopes.set('skill-browser-automation', defaultAgentScope.map((item) => ({ ...item })))
    this.scopes.set('skill-pdf-extractor', defaultAgentScope.map((item, index) => ({ ...item, enabled: index === 1 })))
    this.scopes.set('skill-meeting-notes', defaultAgentScope.map((item) => ({ ...item, enabled: false })))
  }

  async catalogList(query: PluginCatalogQuery): Promise<PluginCatalogDTO> {
    const search = query.search?.trim().toLocaleLowerCase()
    const matching = this.catalog.filter((item) => {
      if (item.kind !== query.kind) return false
      if (query.category && item.category !== query.category) return false
      if (!search) return true
      return [item.name, item.description, item.id, item.category]
        .some((value) => value.toLocaleLowerCase().includes(search))
    })
    const categories = [...new Set(this.catalog.filter((item) => item.kind === query.kind).map((item) => item.category))]
    const start = query.cursor ? Number.parseInt(query.cursor, 10) || 0 : 0
    const pageSize = Math.max(1, query.pageSize ?? (matching.length || 1))
    const items = matching.slice(start, start + pageSize).map(cloneCard)
    return {
      items,
      total: matching.length,
      categories,
      nextCursor: start + pageSize < matching.length ? String(start + pageSize) : undefined,
    }
  }

  async installationsList(): Promise<PluginInstallationsDTO> {
    const items = this.catalog
      .filter((item) => item.installStatus !== 'not_installed')
      .map(cloneCard)
    return { items, total: items.length }
  }

  async operationsList(): Promise<PluginOperationDTO[]> {
    return [...this.operations.values()]
      .sort((left, right) => right.updatedAt - left.updatedAt)
      .map(cloneOperation)
  }

  async install(pluginId: string): Promise<PluginOperationDTO> {
    return this.startOperation(pluginId, 'install')
  }

  async uninstall(pluginId: string): Promise<PluginOperationDTO> {
    return this.startOperation(pluginId, 'uninstall')
  }

  async update(pluginId: string): Promise<PluginOperationDTO> {
    return this.startOperation(pluginId, 'update')
  }

  async setActivation(pluginId: string, update: PluginActivationUpdate): Promise<PluginCardView> {
    const item = this.requirePlugin(pluginId)
    if (update.mode === 'global') {
      if (item.activation.mode !== 'global') throw new Error('该插件需要按 Agent 配置启用范围')
      item.activation = { mode: 'global', enabled: update.enabled }
    } else {
      if (item.activation.mode !== 'per_agent') throw new Error('该插件不支持按 Agent 配置')
      const scope = this.scopes.get(pluginId) ?? []
      const agent = scope.find((entry) => entry.agentId === update.agentId)
      if (!agent) throw new Error('未找到要配置的 Agent')
      agent.enabled = update.enabled
      item.activation = { mode: 'per_agent', enabledAgentCount: scope.filter((entry) => entry.enabled).length }
    }
    item.updatedAt = this.now()
    return cloneCard(item)
  }

  async agentScopeList(pluginId: string): Promise<PluginAgentScopeDTO[]> {
    return (this.scopes.get(pluginId) ?? []).map((item) => ({ ...item }))
  }

  async completeOperation(operationId: string, outcome: 'success' | 'partial' = 'success'): Promise<PluginOperationDTO> {
    const operation = this.operations.get(operationId)
    if (!operation) throw new Error('Operation 不存在')
    const item = this.requirePlugin(operation.pluginId)
    operation.status = outcome === 'partial' ? 'completed' : 'completed'
    operation.outcome = outcome
    operation.stage = 'registering'
    operation.progress = 100
    operation.message = outcome === 'partial' ? '部分文件完成，部分文件需要重试。' : '操作完成。'
    operation.updatedAt = this.now()
    if (outcome === 'success') {
      if (operation.operation === 'uninstall') {
        item.installStatus = 'not_installed'
        item.activation = item.activation.mode === 'global'
          ? { mode: 'global', enabled: false }
          : item.activation.mode === 'per_agent'
            ? { mode: 'per_agent', enabledAgentCount: 0 }
            : { mode: 'not_applicable' }
      } else {
        item.installStatus = 'installed'
      }
      item.updatedAt = this.now()
    }
    return cloneOperation(operation)
  }

  private startOperation(pluginId: string, operationKind: PluginOperationDTO['operation']): PluginOperationDTO {
    const item = this.requirePlugin(pluginId)
    const now = this.now()
    item.installStatus = operationKind === 'uninstall' ? 'uninstalling' : 'installing'
    item.updatedAt = now
    const operation: PluginOperationDTO = {
      id: `plugin-operation-${++this.sequence}`,
      pluginId,
      operation: operationKind,
      status: 'running',
      stage: operationStage(operationKind),
      progress: operationKind === 'uninstall' ? 48 : 24,
      createdAt: now,
      updatedAt: now,
      message: operationKind === 'uninstall' ? '正在停用并移除插件。' : '正在准备插件包。',
    }
    this.operations.set(operation.id, operation)
    return cloneOperation(operation)
  }

  private requirePlugin(pluginId: string): PluginCardView {
    const item = this.catalog.find((candidate) => candidate.id === pluginId)
    if (!item) throw new Error(`未找到插件：${pluginId}`)
    return item
  }
}

export function createMockPluginGateway(options?: ConstructorParameters<typeof MockPluginGateway>[0]): MockPluginGateway {
  return new MockPluginGateway(options)
}
