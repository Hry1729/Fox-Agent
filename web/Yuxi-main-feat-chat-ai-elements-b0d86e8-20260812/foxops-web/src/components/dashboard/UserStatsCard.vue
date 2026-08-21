<template>
  <div class="art-card user-stats-card" v-loading="loading">
    <div class="card-title">用户活跃度分析</div>

    <DashboardMiniStats :items="miniStatItems" />

    <div class="chart-section">
      <div class="chart-header">
        <span class="chart-title">活跃度趋势</span>
        <span class="chart-subtitle">最近7天</span>
      </div>
      <ArtLineChart
        v-if="hasSeries"
        :data="lineData"
        :x-axis-data="xAxisData"
        :show-area-color="true"
        symbol="none"
        height="220px"
      />
      <div v-else class="empty-tip">暂无活跃度数据</div>
    </div>
  </div>
</template>

<script setup lang="ts">
  import DashboardMiniStats from '@/components/dashboard/DashboardMiniStats.vue'

  defineOptions({ name: 'UserStatsCard' })

  interface DailyActiveUser {
    date: string
    active_users: number
  }

  interface UserStats {
    total_users?: number
    active_users_24h?: number
    active_users_30d?: number
    daily_active_users?: DailyActiveUser[]
  }

  const props = withDefaults(
    defineProps<{
      userStats?: UserStats | null
      loading?: boolean
    }>(),
    {
      userStats: null,
      loading: false
    }
  )

  const miniStatItems = computed(() => [
    { key: 'total', label: '总用户', value: props.userStats?.total_users || 0 },
    { key: '24h', label: '24h活跃', value: props.userStats?.active_users_24h || 0 },
    { key: '30d', label: '30天活跃', value: props.userStats?.active_users_30d || 0 }
  ])

  const dailySeries = computed(() => props.userStats?.daily_active_users || [])

  const hasSeries = computed(() => dailySeries.value.length > 0)

  const xAxisData = computed(() => dailySeries.value.map((item) => item.date))

  const lineData = computed(() => dailySeries.value.map((item) => item.active_users))
</script>

<style scoped>
  .user-stats-card {
    height: 100%;
    padding: 20px;
  }

  .card-title {
    margin-bottom: 16px;
    font-size: 16px;
    font-weight: 600;
    color: var(--art-gray-900);
  }

  .chart-section {
    padding: 12px;
    border: 1px solid var(--art-gray-150);
    border-radius: 8px;
  }

  .chart-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .chart-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--art-gray-900);
  }

  .chart-subtitle {
    padding: 2px 8px;
    font-size: 11px;
    color: var(--art-gray-600);
    background: var(--art-gray-100);
    border-radius: 4px;
  }

  .empty-tip {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 220px;
    font-size: 14px;
    color: var(--art-gray-500);
  }
</style>
