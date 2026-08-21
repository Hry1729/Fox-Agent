<template>
  <div class="overview-page">
    <StatsOverview :basic-stats="basicStats" @open-feedback="handleOpenFeedback" />

    <ElRow :gutter="16" class="dashboard-grid">
      <ElCol :xs="24" :lg="16" class="mb-4">
        <CallStatsCard />
      </ElCol>
      <ElCol :xs="24" :lg="8" class="mb-4">
        <UserStatsCard :user-stats="allStatsData.users" :loading="loading" />
      </ElCol>
      <ElCol :xs="24" :lg="8" class="mb-4">
        <AgentStatsCard :agent-stats="allStatsData.agents" :loading="loading" />
      </ElCol>
      <ElCol :xs="24" :lg="8" class="mb-4">
        <ToolStatsCard :tool-stats="allStatsData.tools" :loading="loading" />
      </ElCol>
      <ElCol :xs="24" :lg="8" class="mb-4">
        <KnowledgeStatsCard :knowledge-stats="allStatsData.knowledge" :loading="loading" />
      </ElCol>
    </ElRow>

    <FeedbackDialog ref="feedbackDialogRef" />
  </div>
</template>

<script setup lang="ts">
  import { dashboardApi } from '@/api/dashboard'
  import StatsOverview from '@/components/dashboard/StatsOverview.vue'
  import CallStatsCard from '@/components/dashboard/CallStatsCard.vue'
  import UserStatsCard from '@/components/dashboard/UserStatsCard.vue'
  import AgentStatsCard from '@/components/dashboard/AgentStatsCard.vue'
  import ToolStatsCard from '@/components/dashboard/ToolStatsCard.vue'
  import KnowledgeStatsCard from '@/components/dashboard/KnowledgeStatsCard.vue'
  import FeedbackDialog from '@/components/dashboard/FeedbackDialog.vue'

  defineOptions({ name: 'DataOverviewPage' })

  const loading = ref(false)
  const basicStats = ref<Record<string, unknown> | null>(null)
  const allStatsData = ref({
    users: null as Record<string, unknown> | null,
    tools: null as Record<string, unknown> | null,
    knowledge: null as Record<string, unknown> | null,
    agents: null as Record<string, unknown> | null
  })

  const feedbackDialogRef = ref<InstanceType<typeof FeedbackDialog> | null>(null)

  const loadAllStats = async () => {
    loading.value = true
    try {
      const response = await dashboardApi.getAllStats()
      basicStats.value = (response.basic as Record<string, unknown>) || null
      allStatsData.value = {
        users: (response.users as Record<string, unknown>) || null,
        tools: (response.tools as Record<string, unknown>) || null,
        knowledge: (response.knowledge as Record<string, unknown>) || null,
        agents: (response.agents as Record<string, unknown>) || null
      }
    } catch (error) {
      console.error('加载统计数据失败:', error)
      ElMessage.error('加载统计数据失败')
    } finally {
      loading.value = false
    }
  }

  const handleOpenFeedback = () => {
    feedbackDialogRef.value?.open?.()
  }

  onMounted(() => {
    loadAllStats()
  })
</script>

<style scoped>
  .overview-page {
    padding-bottom: 8px;
  }

  .dashboard-grid {
    margin-top: 0;
  }
</style>
