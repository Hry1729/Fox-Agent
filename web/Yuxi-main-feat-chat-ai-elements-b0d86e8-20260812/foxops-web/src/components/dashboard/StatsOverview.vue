<template>
  <ElRow :gutter="16" class="stats-overview">
    <ElCol
      v-for="card in statCards"
      :key="card.key"
      :xs="24"
      :sm="12"
      :md="8"
      :lg="4"
      class="mb-4"
    >
      <div
        class="stat-card-wrap"
        :class="{ 'cursor-pointer': card.clickable }"
        @click="card.clickable ? emit('open-feedback') : undefined"
      >
        <ArtStatsCard
          :icon="card.icon"
          :icon-style="card.iconStyle"
          box-style="overview-stat-card"
          :count="card.count"
          :decimals="card.decimals"
          :description="card.description"
          :show-arrow="false"
        />
      </div>
    </ElCol>
  </ElRow>
</template>

<script setup lang="ts">
  defineOptions({ name: 'StatsOverview' })

  interface BasicStats {
    total_conversations?: number
    active_conversations?: number
    total_messages?: number
    total_users?: number
    feedback_stats?: {
      total_feedbacks?: number
      satisfaction_rate?: number
    }
  }

  const props = withDefaults(
    defineProps<{
      basicStats?: BasicStats | null
    }>(),
    {
      basicStats: () => ({})
    }
  )

  const emit = defineEmits<{
    'open-feedback': []
  }>()

  /** 满意度卡片图标背景色 */
  const satisfactionIconStyle = computed(() => {
    const rate = props.basicStats?.feedback_stats?.satisfaction_rate || 0
    if (rate >= 80) return 'bg-success'
    if (rate >= 60) return 'bg-warning'
    return 'bg-error'
  })

  const statCards = computed(() => {
    const stats = props.basicStats || {}
    const feedback = stats.feedback_stats || {}
    const satisfactionRate = feedback.satisfaction_rate || 0

    return [
      {
        key: 'conversations',
        icon: 'ri:chat-3-line',
        iconStyle: 'bg-primary',
        count: stats.total_conversations || 0,
        decimals: 0,
        description: '累计会话',
        clickable: false
      },
      {
        key: 'active',
        icon: 'ri:pulse-line',
        iconStyle: 'bg-success',
        count: stats.active_conversations || 0,
        decimals: 0,
        description: '活跃对话',
        clickable: false
      },
      {
        key: 'messages',
        icon: 'ri:mail-line',
        iconStyle: 'bg-info',
        count: stats.total_messages || 0,
        decimals: 0,
        description: '总消息数',
        clickable: false
      },
      {
        key: 'users',
        icon: 'ri:group-line',
        iconStyle: 'bg-warning',
        count: stats.total_users || 0,
        decimals: 0,
        description: '用户数',
        clickable: false
      },
      {
        key: 'feedbacks',
        icon: 'ri:bar-chart-2-line',
        iconStyle: 'bg-secondary',
        count: feedback.total_feedbacks || 0,
        decimals: 0,
        description: '总反馈数',
        clickable: true
      },
      {
        key: 'satisfaction',
        icon: 'ri:heart-line',
        iconStyle: satisfactionIconStyle.value,
        count: satisfactionRate,
        decimals: 1,
        description: '满意度 (%)',
        clickable: false
      }
    ]
  })
</script>

<style scoped>
  /* 压缩高度，内容垂直居中并略偏右 */
  .stat-card-wrap :deep(.overview-stat-card) {
    height: 6.5rem;
    min-height: 6.5rem;
    padding: 0 1.25rem 0 2.25rem;
    align-items: center;
    justify-content: flex-start;
  }

  .stat-card-wrap :deep(.overview-stat-card > div:first-child) {
    width: 2.5rem;
    height: 2.5rem;
    margin-right: 0.875rem;
    font-size: 1.2rem;
    flex-shrink: 0;
  }

  .stat-card-wrap :deep(.overview-stat-card .text-2xl) {
    font-size: 1.5rem;
    line-height: 1.2;
  }

  .stat-card-wrap :deep(.overview-stat-card .text-sm) {
    margin-top: 0.2rem;
    font-size: 0.8125rem;
  }
</style>
