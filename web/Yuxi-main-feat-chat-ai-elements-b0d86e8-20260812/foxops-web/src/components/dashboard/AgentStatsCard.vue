<template>
  <div class="art-card agent-stats-card" v-loading="loading">
    <div class="card-title">AI智能体分析</div>

    <DashboardMiniStats :items="miniStatItems" />

    <div class="section-block">
      <div class="section-title">对话/工具调用分布 (TOP 3)</div>
      <ArtBarChart
        v-if="topAgentIds.length"
        :data="barChartData"
        :x-axis-data="barXAxisData"
        :show-legend="true"
        legend-position="top"
        height="240px"
        bar-width="28%"
      />
      <div v-else class="empty-tip">暂无智能体统计数据</div>
    </div>

    <div v-if="topPerformers.length" class="section-block">
      <div class="section-title">表现最佳智能体 TOP 5</div>
      <ElTable :data="topPerformers" size="small">
        <ElTableColumn label="排名" width="70" align="center">
          <template #default="{ $index }">
            <span class="rank-badge" :class="{ featured: $index < 3 }">{{ $index + 1 }}</span>
          </template>
        </ElTableColumn>
        <ElTableColumn label="智能体" min-width="120">
          <template #default="{ row }">
            <span class="agent-name" :title="resolveAgentName(row.agent_id)">
              {{ resolveAgentName(row.agent_id) }}
            </span>
          </template>
        </ElTableColumn>
        <ElTableColumn label="满意度" width="100" align="center">
          <template #default="{ row }">
            <span :class="satisfactionClass(row.satisfaction_rate)">
              {{ row.satisfaction_rate }}%
            </span>
          </template>
        </ElTableColumn>
        <ElTableColumn
          prop="conversation_count"
          label="对话数"
          width="90"
          align="center"
        />
      </ElTable>
    </div>
  </div>
</template>

<script setup lang="ts">
  import DashboardMiniStats from '@/components/dashboard/DashboardMiniStats.vue'

  defineOptions({ name: 'AgentStatsCard' })

  interface AgentCountItem {
    agent_id: string
    conversation_count: number
  }

  interface AgentToolItem {
    agent_id: string
    tool_usage_count: number
  }

  interface TopPerformer {
    agent_id: string
    satisfaction_rate: number
    conversation_count: number
  }

  interface AgentStats {
    total_agents?: number
    agent_conversation_counts?: AgentCountItem[]
    agent_tool_usage?: AgentToolItem[]
    top_performing_agents?: TopPerformer[]
    agent_names?: Record<string, string>
  }

  const props = withDefaults(
    defineProps<{
      agentStats?: AgentStats | null
      loading?: boolean
    }>(),
    {
      agentStats: null,
      loading: false
    }
  )

  const agentNames = computed(() => props.agentStats?.agent_names || {})

  const resolveAgentName = (agentId: string) => agentNames.value[agentId] || agentId

  const totalConversations = computed(() => {
    const items = props.agentStats?.agent_conversation_counts || []
    return items.reduce((sum, item) => sum + (item.conversation_count || 0), 0)
  })

  const totalToolUsage = computed(() => {
    const items = props.agentStats?.agent_tool_usage || []
    return items.reduce((sum, item) => sum + (item.tool_usage_count || 0), 0)
  })

  const miniStatItems = computed(() => [
    {
      key: 'agents',
      label: '智能体总数',
      value: props.agentStats?.total_agents || 0,
      suffix: '个'
    },
    { key: 'conversations', label: '总对话数', value: totalConversations.value, suffix: '次' },
    { key: 'tools', label: '工具调用总数', value: totalToolUsage.value, suffix: '次' }
  ])

  const topPerformers = computed(() => props.agentStats?.top_performing_agents || [])

  /** 合并对话数与工具调用数，取总量最高的 TOP 智能体 */
  const topAgentIds = computed(() => {
    const conversationData = props.agentStats?.agent_conversation_counts || []
    const toolData = props.agentStats?.agent_tool_usage || []
    const merged: Record<string, { conversation: number; tool: number; total: number }> = {}

    conversationData.forEach((item) => {
      if (!merged[item.agent_id]) {
        merged[item.agent_id] = { conversation: 0, tool: 0, total: 0 }
      }
      merged[item.agent_id].conversation = item.conversation_count || 0
      merged[item.agent_id].total += item.conversation_count || 0
    })

    toolData.forEach((item) => {
      if (!merged[item.agent_id]) {
        merged[item.agent_id] = { conversation: 0, tool: 0, total: 0 }
      }
      merged[item.agent_id].tool = item.tool_usage_count || 0
      merged[item.agent_id].total += item.tool_usage_count || 0
    })

    return Object.entries(merged)
      .sort(([, a], [, b]) => b.total - a.total)
      .slice(0, 3)
      .map(([agentId]) => agentId)
  })

  const barXAxisData = computed(() => topAgentIds.value.map(resolveAgentName))

  const barChartData = computed(() => {
    const conversationData = props.agentStats?.agent_conversation_counts || []
    const toolData = props.agentStats?.agent_tool_usage || []

    return [
      {
        name: '对话数',
        data: topAgentIds.value.map((agentId) => {
          const item = conversationData.find((d) => d.agent_id === agentId)
          return item?.conversation_count || 0
        })
      },
      {
        name: '工具调用数',
        data: topAgentIds.value.map((agentId) => {
          const item = toolData.find((d) => d.agent_id === agentId)
          return item?.tool_usage_count || 0
        })
      }
    ]
  })

  const satisfactionClass = (rate: number) => {
    if (rate >= 80) return 'text-success'
    if (rate >= 60) return 'text-warning'
    return 'text-error'
  }
</script>

<style scoped>
  .agent-stats-card {
    height: 100%;
    padding: 20px;
  }

  .card-title {
    margin-bottom: 16px;
    font-size: 16px;
    font-weight: 600;
    color: var(--art-gray-900);
  }

  .section-block {
    margin-top: 16px;
  }

  .section-title {
    margin-bottom: 12px;
    font-size: 14px;
    font-weight: 600;
    color: var(--art-gray-800);
  }

  .empty-tip {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 200px;
    font-size: 14px;
    color: var(--art-gray-500);
  }

  .rank-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    font-size: 12px;
    font-weight: 600;
    color: var(--art-gray-600);
    background: var(--art-gray-100);
    border: 1px solid var(--art-gray-200);
    border-radius: 50%;
  }

  .rank-badge.featured {
    color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
    border-color: var(--el-color-primary-light-7);
  }

  .agent-name {
    display: inline-block;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .text-success {
    color: var(--el-color-success);
    font-weight: 600;
  }

  .text-warning {
    color: var(--el-color-warning);
    font-weight: 600;
  }

  .text-error {
    color: var(--el-color-danger);
    font-weight: 600;
  }
</style>
