<template>
  <div class="agent-manage-panel">
    <ExtensionToolbar
      v-model:search="searchQuery"
      class="agent-toolbar"
      search-placeholder="搜索智能体..."
      :loading="loading"
      @refresh="loadAgents"
    >
      <template #actions>
        <ElButton type="primary" @click="openCreateAgentModal">
          <ArtSvgIcon icon="ri:add-line" />
          新增智能体
        </ElButton>
      </template>
    </ExtensionToolbar>

    <ExtensionEmptyState
      v-if="!loading && !groupedAgents.length"
      :description="searchQuery ? '没有匹配的智能体' : '暂无智能体'"
    >
      <ElButton type="primary" @click="openCreateAgentModal">新增智能体</ElButton>
    </ExtensionEmptyState>

    <template v-else>
      <section v-for="group in groupedAgents" :key="group.key" class="agent-group-section">
        <div class="agent-group-header">{{ group.title }}</div>
        <ExtensionCardGrid class="extension-card-grid--dense">
          <ExtensionInfoCard
            v-for="agent in group.agents"
            :key="agent.id"
            :title="agent.name || agent.id"
            :subtitle="agent.slug || agent.id"
            :description="agent.description || '暂无描述'"
            accent="blue"
            :tags="getAgentTags(agent)"
            :disabled="!canManageAgent(agent)"
            @click="openEditAgentModal(agent)"
          >
            <template #icon>
              <FallbackAvatar
                class="agent-card-icon"
                :src="agent.icon"
                :default-src="getAgentDefaultIconSrc(agent)"
                :name="agent.name || agent.id"
                :seed="agent.id || agent.name"
                kind="agent"
                :size="38"
                shape="rounded"
                :alt="`${agent.name || '智能体'}图标`"
              />
            </template>
            <template v-if="canManageAgent(agent)" #action>
              <ElDropdown trigger="click" @command="(cmd) => handleAgentCommand(cmd, agent)">
                <ElButton text class="agent-card-menu-trigger" @click.stop>
                  <ArtSvgIcon icon="ri:more-2-fill" />
                </ElButton>
                <template #dropdown>
                  <ElDropdownMenu>
                    <ElDropdownItem command="edit">编辑智能体</ElDropdownItem>
                    <ElDropdownItem command="delete" :disabled="isBuiltinAgent(agent)">
                      删除智能体
                    </ElDropdownItem>
                  </ElDropdownMenu>
                </template>
              </ElDropdown>
            </template>
          </ExtensionInfoCard>
        </ExtensionCardGrid>
      </section>
    </template>

    <AgentEditModal ref="agentEditModalRef" @saved="refreshAgentLists" />
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { agentApi } from '@/api/agent'
  import FallbackAvatar from '@/components/common/FallbackAvatar.vue'
  import AgentEditModal from '@/components/chat/AgentEditModal.vue'
  import ExtensionCardGrid from '@/components/extensions/common/ExtensionCardGrid.vue'
  import ExtensionEmptyState from '@/components/extensions/common/ExtensionEmptyState.vue'
  import ExtensionInfoCard from '@/components/extensions/common/ExtensionInfoCard.vue'
  import ExtensionToolbar from '@/components/extensions/common/ExtensionToolbar.vue'
  import { isBuiltinAgent, useAgentStore } from '@/store/modules/agent'
  import { unwrapList } from '@/utils/apiData'
  import { generatePixelAvatar } from '@/utils/pixelAvatar'

  type AgentItem = {
    id: string
    agent_id?: string
    slug?: string
    name?: string
    description?: string
    icon?: string
    backend_id?: string
    is_subagent?: boolean
    can_manage?: boolean
    share_config?: { access_level?: string }
  }

  const agentStore = useAgentStore()
  const loading = ref(false)
  const searchQuery = ref('')
  const managedAgents = ref<AgentItem[]>([])
  const agentEditModalRef = ref<InstanceType<typeof AgentEditModal> | null>(null)

  const normalizeAgent = (agent: any): AgentItem => {
    const agentId = agent?.agent_id || agent?.slug || agent?.id
    return agentId
      ? { ...agent, id: agentId, agent_id: agentId, slug: agent?.slug || agentId }
      : agent
  }

  const filteredAgents = computed(() => {
    const keyword = searchQuery.value.trim().toLowerCase()
    const list = managedAgents.value || []
    const filtered = keyword
      ? list.filter(
          (agent) =>
            String(agent.name || '')
              .toLowerCase()
              .includes(keyword) ||
            String(agent.id || '')
              .toLowerCase()
              .includes(keyword) ||
            String(agent.backend_id || '')
              .toLowerCase()
              .includes(keyword)
        )
      : list
    return [...filtered].sort((a, b) => {
      if (isBuiltinAgent(a) !== isBuiltinAgent(b)) return isBuiltinAgent(a) ? -1 : 1
      return String(a.name || a.id).localeCompare(String(b.name || b.id), 'zh-CN')
    })
  })

  const groupedAgents = computed(() => {
    const agents = filteredAgents.value.filter((agent) => !agent.is_subagent)
    const subagents = filteredAgents.value.filter((agent) => agent.is_subagent)
    return [
      { key: 'agents', title: '智能体', agents },
      { key: 'subagents', title: '子智能体', agents: subagents }
    ].filter((group) => group.agents.length > 0)
  })

  const agentStats = computed(() => ({
    total: managedAgents.value.length,
    builtin: managedAgents.value.filter(isBuiltinAgent).length,
    manageable: managedAgents.value.filter((agent) => agent.can_manage).length,
    global: managedAgents.value.filter((agent) => agent.share_config?.access_level === 'global')
      .length
  }))

  const canManageAgent = (agent: AgentItem) => !!agent?.can_manage

  const getAgentDefaultIconSrc = (agent: AgentItem) =>
    agent.id ? generatePixelAvatar(agent.id) : ''

  const getAgentTags = (agent: AgentItem) => {
    const tags: Array<{ name: string; color?: 'blue' | 'green' | 'orange' }> = []
    if (!agent?.can_manage) tags.push({ name: '只读', color: 'orange' })
    if (agent?.backend_id) tags.push({ name: agent.backend_id, color: 'blue' })
    return tags
  }

  const loadAgents = async () => {
    loading.value = true
    try {
      const response = await agentApi.getAgents({ includeSubagents: true })
      managedAgents.value = unwrapList(response).map(normalizeAgent)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载智能体失败')
      managedAgents.value = []
    } finally {
      loading.value = false
    }
  }

  const openCreateAgentModal = () => {
    agentEditModalRef.value?.openCreate()
  }

  const openEditAgentModal = (agent: AgentItem) => {
    if (!canManageAgent(agent)) return
    agentEditModalRef.value?.openEdit(agent)
  }

  const refreshAgentLists = async () => {
    await Promise.all([loadAgents(), agentStore.fetchAgents({ includeSubagents: true })])
  }

  const deleteAgent = async (agent: AgentItem) => {
    if (isBuiltinAgent(agent)) {
      ElMessage.warning('内置智能体不能删除')
      return
    }
    try {
      await ElMessageBox.confirm(
        '删除后不可恢复，已绑定该智能体的历史对话仍保留原始绑定信息。',
        `删除 ${agent.name}`,
        { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' }
      )
      await agentApi.deleteAgent(agent.id)
      await refreshAgentLists()
      ElMessage.success('智能体已删除')
    } catch {
      // 取消
    }
  }

  const handleAgentCommand = (command: string, agent: AgentItem) => {
    if (command === 'edit') openEditAgentModal(agent)
    if (command === 'delete') deleteAgent(agent)
  }

  onMounted(loadAgents)

  defineExpose({
    loading,
    stats: agentStats,
    refresh: loadAgents
  })
</script>

<style scoped>
  .agent-toolbar :deep(.toolbar-actions) {
    gap: 4px;
  }

  .agent-toolbar :deep(.toolbar-actions .el-button) {
    margin: 0;
  }

  .agent-group-section + .agent-group-section {
    margin-top: 4px;
  }

  .agent-group-header {
    margin: 4px 0 12px;
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-secondary);
  }

  :deep(.extension-card-grid--dense) {
    gap: 12px;
  }

  @media (min-width: 1080px) {
    :deep(.extension-card-grid--dense) {
      grid-template-columns: repeat(4, minmax(0, 1fr));
    }
  }

  .agent-card-icon {
    width: 100%;
    height: 100%;
  }

  .agent-card-menu-trigger {
    width: 28px;
    height: 28px;
    padding: 0;
    color: var(--el-text-color-secondary);
  }
</style>
