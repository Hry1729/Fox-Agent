import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { agentApi } from '@/api/agent'
import { handleChatError } from '@/utils/errorHandler'
import { isDefaultAllAgentResourceKind } from '@/utils/agentConfigUtils'

/**
 * 智能体 store（搬自 Yuxi stores/agent.js，MVP 简化版）
 *
 * 简化点（后置）：
 * - 去掉 @提及资源（availableKnowledgeBases/Mcps/Skills + fetchMentionResources）
 * - 去掉智能体配置管理（agentConfig/originalAgentConfig/configurableItems/saveAgentConfig 等）
 * 只保留对话必需：智能体列表、选智能体、初始化。
 */
function extractContext(agent) {
  const configJson = agent?.config_json || {}
  if (configJson && typeof configJson === 'object' && configJson.context) {
    return { ...configJson.context }
  }
  if (agent?.config_context && typeof agent.config_context === 'object') {
    return { ...agent.config_context }
  }
  if (configJson && typeof configJson === 'object' && !Array.isArray(configJson)) {
    return { ...configJson }
  }
  return {}
}

function applyConfigDefaults(loadedConfig, configItems) {
  const config = { ...(loadedConfig || {}) }
  const entries = Array.isArray(configItems)
    ? configItems.map((item) => [item.key, item])
    : Object.entries(configItems || {})

  for (const [key, rawItem] of entries) {
    if (!key || !rawItem || typeof rawItem !== 'object') continue
    const item = rawItem.x_oap_ui_config
      ? { ...rawItem, ...rawItem.x_oap_ui_config }
      : rawItem
    const isDefaultAllList = isDefaultAllAgentResourceKind(item?.kind)
    if (config[key] === undefined || (config[key] === null && !isDefaultAllList)) {
      if (item.default !== undefined) config[key] = item.default
    }
    if (
      config[key] !== undefined &&
      config[key] !== null &&
      config[key] !== '' &&
      (item?.type === 'number' || item?.type === 'int' || item?.type === 'float')
    ) {
      const numericValue = Number(config[key])
      if (!Number.isNaN(numericValue)) {
        config[key] = item.type === 'int' ? Math.trunc(numericValue) : numericValue
      }
    }
  }
  return config
}

function normalizeAgent(agent) {
  const agentId = agent?.agent_id || agent?.slug || agent?.id
  return agentId
    ? { ...agent, id: agentId, agent_id: agentId, slug: agent?.slug || agentId }
    : agent
}

export const BUILTIN_AGENT_ID = 'default-chatbot'

export function isBuiltinAgent(agent) {
  return agent?.is_builtin || agent?.id === BUILTIN_AGENT_ID || agent?.slug === BUILTIN_AGENT_ID
}

function sortAgents(agents) {
  return [...agents].sort((a, b) => {
    if (isBuiltinAgent(a) !== isBuiltinAgent(b)) return isBuiltinAgent(a) ? -1 : 1
    return String(a.name || a.id).localeCompare(String(b.name || b.id), 'zh-CN')
  })
}

function getPreferredAgentId(agents, persistedId) {
  const chatAgents = agents.filter((agent) => !agent.is_subagent)
  if (persistedId && chatAgents.some((agent) => agent.id === persistedId)) return persistedId
  return chatAgents.find(isBuiltinAgent)?.id || chatAgents[0]?.id || null
}

export const useAgentStore = defineStore('agent', () => {
  const agents = ref([])
  const selectedAgentId = ref(null)
  const agentDetails = ref({})

  // 运行时配置（模型/工具/其他 tab 的配置项）
  const agentConfig = ref({})
  const originalAgentConfig = ref({})

  const isLoadingAgents = ref(false)
  const isLoadingAgentDetail = ref(false)
  const error = ref(null)
  const isInitialized = ref(false)
  const isInitializing = ref(false)

  const selectedAgent = computed(() => {
    const agentId = selectedAgentId.value
    return agentId
      ? agentDetails.value[agentId] || agents.value.find((a) => a.id === agentId) || null
      : null
  })

  const agentsList = computed(() => agents.value)

  // 可配置项（后端驱动，来自 agent detail 的 configurable_items）
  // configurable_items 可能是数组或对象（keyed by key），统一转为数组
  const configurableItems = computed(() => {
    const items = selectedAgent.value?.configurable_items
    if (Array.isArray(items)) return items
    if (items && typeof items === 'object') {
      return Object.entries(items).map(([key, item]) => ({
        ...(typeof item === 'object' && item !== null ? item : { value: item }),
        key: item?.key || key
      }))
    }
    return []
  })

  // 配置是否有未保存修改
  const hasConfigChanges = computed(() => {
    return JSON.stringify(agentConfig.value) !== JSON.stringify(originalAgentConfig.value)
  })

  function loadAgentRuntimeConfig(agent) {
    const loadedConfig = applyConfigDefaults(
      extractContext(agent),
      agent?.configurable_items || {}
    )
    agentConfig.value = JSON.parse(JSON.stringify(loadedConfig))
    originalAgentConfig.value = JSON.parse(JSON.stringify(loadedConfig))
    return loadedConfig
  }

  async function initialize() {
    if (isInitialized.value || isInitializing.value) return
    isInitializing.value = true
    try {
      await fetchAgents()
      const targetAgentId = getPreferredAgentId(agents.value, selectedAgentId.value)
      if (targetAgentId) {
        await selectAgent(targetAgentId)
      }
      isInitialized.value = true
    } catch (err) {
      console.error('Failed to initialize agent store:', err)
      handleChatError(err, 'initialize')
      error.value = err.message
    } finally {
      isInitializing.value = false
    }
  }

  async function fetchAgents({ includeSubagents = false } = {}) {
    isLoadingAgents.value = true
    error.value = null
    try {
      const response = await agentApi.getAgents({ includeSubagents })
      agents.value = sortAgents((response.agents || []).map(normalizeAgent))
    } catch (err) {
      console.error('Failed to fetch agents:', err)
      handleChatError(err, 'fetch')
      error.value = err.message
      throw err
    } finally {
      isLoadingAgents.value = false
    }
  }

  async function fetchAgentDetail(agentId, forceRefresh = false) {
    if (!agentId) return null
    if (!forceRefresh && agentDetails.value[agentId]) return agentDetails.value[agentId]

    isLoadingAgentDetail.value = true
    error.value = null
    try {
      const response = await agentApi.getAgentDetail(agentId)
      const agent = normalizeAgent(response.agent || response)
      agentDetails.value[agent.id] = agent
      loadAgentRuntimeConfig(agent)
      return agent
    } catch (err) {
      console.error(`Failed to fetch agent detail for ${agentId}:`, err)
      handleChatError(err, 'fetch')
      error.value = err.message
      throw err
    } finally {
      isLoadingAgentDetail.value = false
    }
  }

  async function selectAgent(agentId, { allowSubagent = false } = {}) {
    if (!agentId) return
    let knownAgent = agentDetails.value[agentId] || agents.value.find((a) => a.id === agentId)
    // 列表接口不含 configurable_items/config_context，需调详情接口补全
    if (!knownAgent || !knownAgent.configurable_items) {
      knownAgent = await fetchAgentDetail(agentId)
    }
    if (knownAgent?.is_subagent && !allowSubagent) return
    selectedAgentId.value = agentId
    if (knownAgent?.configurable_items) {
      loadAgentRuntimeConfig(knownAgent)
    }
  }

  const agentBackends = ref([])
  const agentBackendsLoaded = ref(false)

  async function fetchAgentBackends(forceRefresh = false) {
    if (agentBackendsLoaded.value && !forceRefresh) return agentBackends.value
    try {
      const response = await agentApi.getAgentBackends()
      agentBackends.value = (response.backends || []).map((backend) => ({
        label: backend.name || backend.backend_id,
        value: backend.backend_id
      }))
      agentBackendsLoaded.value = true
      return agentBackends.value
    } catch (err) {
      console.error('Failed to fetch agent backends:', err)
      throw err
    }
  }

  async function createAgent(payload) {
    const response = await agentApi.createAgent(payload)
    const agent = normalizeAgent(response.agent || response)
    await fetchAgents()
    return agent
  }

  async function updateAgentProfile(agentId, payload) {
    const response = await agentApi.updateAgent(agentId, payload)
    const agent = normalizeAgent(response.agent || response)
    agentDetails.value[agent.id] = agent
    const idx = agents.value.findIndex((a) => a.id === agent.id)
    if (idx !== -1) agents.value[idx] = agent
    return agent
  }

  function reset() {
    agents.value = []
    selectedAgentId.value = null
    agentDetails.value = {}
    agentConfig.value = {}
    originalAgentConfig.value = {}
    isLoadingAgents.value = false
    isLoadingAgentDetail.value = false
    error.value = null
    isInitialized.value = false
    isInitializing.value = false
  }

  // 更新运行时配置（局部更新）
  function updateAgentConfig(updates) {
    Object.assign(agentConfig.value, updates)
  }

  // 重置配置到原始值
  function resetAgentConfig() {
    agentConfig.value = JSON.parse(JSON.stringify(originalAgentConfig.value || {}))
  }

  // 保存配置（带 config_json）
  async function saveAgentConfig(agentId) {
    const cachedAgent = agentDetails.value[agentId]
    const response = await agentApi.updateAgent(agentId, {
      config_json: { context: agentConfig.value }
    })
    const agent = normalizeAgent(response.agent || response)
    const cachedItems = cachedAgent?.configurable_items
    if (cachedItems) {
      if (!agent.configurable_items) {
        agent.configurable_items = cachedItems
      } else if (
        typeof cachedItems === 'object' &&
        !Array.isArray(cachedItems) &&
        typeof agent.configurable_items === 'object' &&
        !Array.isArray(agent.configurable_items)
      ) {
        agent.configurable_items = { ...cachedItems, ...agent.configurable_items }
      }
    }
    agentDetails.value[agentId] = agent
    loadAgentRuntimeConfig(agent)
    return agent
  }

  return {
    agents,
    selectedAgentId,
    agentDetails,
    agentConfig,
    originalAgentConfig,
    configurableItems,
    hasConfigChanges,
    isLoadingAgents,
    isLoadingAgentDetail,
    error,
    isInitialized,
    selectedAgent,
    agentsList,
    agentBackends,
    agentBackendsLoaded,
    initialize,
    fetchAgents,
    fetchAgentDetail,
    fetchAgentBackends,
    createAgent,
    updateAgentProfile,
    saveAgentConfig,
    updateAgentConfig,
    resetAgentConfig,
    selectAgent,
    reset
  }
})
