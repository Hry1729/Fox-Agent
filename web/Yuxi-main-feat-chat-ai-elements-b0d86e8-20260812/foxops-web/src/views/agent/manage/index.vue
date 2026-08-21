<template>
  <div class="page-content agent-manage-page !mb-5">
    <div class="manage-header-card">
      <div class="title-main">
        <h1>智能体管理</h1>
        <nav class="detail-nav">
          <button
            v-for="tab in visibleTabs"
            :key="tab.key"
            type="button"
            class="nav-item"
            :class="{ active: activeTab === tab.key }"
            @click="switchTab(tab.key)"
          >
            {{ tab.label }}
          </button>
        </nav>
      </div>
      <div class="summary-strip">
        <template v-if="activeTab === 'agents'">
          <span>{{ activeStats.total || 0 }} 个智能体</span>
          <span>{{ activeStats.global || 0 }} 个全局</span>
          <span v-if="activeStats.builtin">{{ activeStats.builtin }} 个内置</span>
          <span>{{ activeStats.manageable || 0 }} 个可管理</span>
        </template>
        <template v-else>
          <span>{{ activeStats.total || 0 }} 个供应商</span>
          <span>{{ activeStats.enabled || 0 }} 个启用</span>
          <span v-if="(activeStats.warning || 0) > 0" class="warning-count">
            {{ activeStats.warning }} 个凭证缺失
          </span>
          <span>{{ activeStats.models || 0 }} 个模型</span>
        </template>
      </div>
    </div>

    <div v-loading="activeLoading" class="manage-content">
      <div v-show="activeTab === 'agents'" class="tab-panel">
        <AgentManagePanel ref="agentPanelRef" />
      </div>
      <div v-if="userStore.isAdmin && activeTab === 'providers'" class="tab-panel">
        <ModelProviderManagePanel ref="providerPanelRef" />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
  import { useRoute, useRouter } from 'vue-router'
  import AgentManagePanel from '@/components/agent-management/AgentManagePanel.vue'
  import ModelProviderManagePanel from '@/components/agent-management/ModelProviderManagePanel.vue'
  import { useUserStore } from '@/store/modules/user'

  defineOptions({ name: 'AgentManagePage' })

  const route = useRoute()
  const router = useRouter()
  const userStore = useUserStore()

  const activeTab = ref('agents')
  const agentPanelRef = ref<InstanceType<typeof AgentManagePanel> | null>(null)
  const providerPanelRef = ref<InstanceType<typeof ModelProviderManagePanel> | null>(null)

  type ManageStats = Partial<{
    total: number
    global: number
    builtin: number
    manageable: number
    enabled: number
    warning: number
    models: number
  }>

  const visibleTabs = computed(() => {
    const tabs = [{ key: 'agents', label: '智能体' }]
    if (userStore.isAdmin) tabs.push({ key: 'providers', label: '模型供应商' })
    return tabs
  })

  const activePanel = computed(() =>
    activeTab.value === 'providers' ? providerPanelRef.value : agentPanelRef.value
  )

  const activeLoading = computed(() => activePanel.value?.loading || false)
  const activeStats = computed<ManageStats>(() => activePanel.value?.stats || {})

  const normalizeTab = (tab: unknown) => {
    if (tab === 'providers' && userStore.isAdmin) return 'providers'
    return 'agents'
  }

  const switchTab = (tab: string) => {
    activeTab.value = normalizeTab(tab)
  }

  watch(
    () => [route.query.tab, userStore.isAdmin],
    ([tab]) => {
      const nextTab = normalizeTab(tab)
      if (activeTab.value !== nextTab) activeTab.value = nextTab
    },
    { immediate: true }
  )

  watch(activeTab, (tab) => {
    const nextTab = normalizeTab(tab)
    if (nextTab !== tab) {
      activeTab.value = nextTab
      return
    }
    if (route.query.tab === nextTab) return
    router.replace({ query: { ...route.query, tab: nextTab } })
  })
</script>

<style scoped>
  .agent-manage-page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-height: 0;
  }

  .manage-header-card {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
  }

  .title-main {
    display: flex;
    align-items: center;
    gap: 16px;
    min-width: 0;
    flex-wrap: wrap;
  }

  .title-main h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    line-height: 1.3;
    white-space: nowrap;
  }

  .detail-nav {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
  }

  .nav-item {
    height: var(--el-component-size, 32px);
    padding: 0 14px;
    border: none;
    border-radius: var(--el-border-radius-base, 4px);
    background: transparent;
    color: var(--el-text-color-regular);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
    white-space: nowrap;
  }

  .nav-item:hover {
    background: var(--el-fill-color-light);
    color: var(--el-color-primary);
  }

  .nav-item.active {
    background: var(--el-color-primary-light-9);
    color: var(--el-color-primary);
    font-weight: 600;
  }

  .summary-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .summary-strip span {
    padding: 6px 10px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 7px;
    background: var(--el-fill-color-lighter);
    color: var(--el-text-color-secondary);
    font-size: 12px;
    line-height: 18px;
    white-space: nowrap;
  }

  .summary-strip .warning-count {
    background: var(--el-color-warning-light-9);
    border-color: var(--el-color-warning-light-7);
    color: var(--el-color-warning);
  }

  .manage-content {
    flex: 1;
    min-height: 0;
  }

  .tab-panel {
    min-height: 0;
  }
</style>
