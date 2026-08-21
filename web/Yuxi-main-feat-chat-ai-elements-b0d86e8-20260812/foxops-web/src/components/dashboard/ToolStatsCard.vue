<template>
  <div class="art-card tool-stats-card" v-loading="loading">
    <div class="card-title">工具调用监控</div>

    <DashboardMiniStats :items="miniStatItems" />

    <div class="section-block">
      <div class="section-title">最常用工具 TOP 10</div>
      <ArtHBarChart
        v-if="topTools.length"
        :data="topToolCounts"
        :x-axis-data="topToolNames"
        height="260px"
      />
      <div v-else class="empty-tip">暂无工具调用数据</div>
    </div>

    <div v-if="hasErrorData" class="section-block">
      <div class="section-title">工具错误分析</div>
      <ElRow :gutter="12">
        <ElCol :xs="24" :md="12">
          <ElTable :data="errorData" size="small" max-height="220">
            <ElTableColumn label="工具名称" min-width="140">
              <template #default="{ row }">
                <ElTag type="primary" size="small">{{ row.tool_name }}</ElTag>
              </template>
            </ElTableColumn>
            <ElTableColumn label="错误次数" width="100" align="center">
              <template #default="{ row }">
                <ElTag :type="row.error_count > 5 ? 'danger' : 'warning'" size="small">
                  {{ row.error_count }}
                </ElTag>
              </template>
            </ElTableColumn>
          </ElTable>
        </ElCol>
        <ElCol :xs="24" :md="12">
          <div class="error-chart-title">错误分布图 TOP 5</div>
          <ArtHBarChart
            :data="errorChartCounts"
            :x-axis-data="errorChartNames"
            height="220px"
            bar-width="40%"
          />
        </ElCol>
      </ElRow>
    </div>
  </div>
</template>

<script setup lang="ts">
  import DashboardMiniStats from '@/components/dashboard/DashboardMiniStats.vue'

  defineOptions({ name: 'ToolStatsCard' })

  interface ToolUsageItem {
    tool_name: string
    count: number
  }

  interface ToolStats {
    total_calls?: number
    failed_calls?: number
    success_rate?: number
    most_used_tools?: ToolUsageItem[]
    tool_error_distribution?: Record<string, number>
  }

  const props = withDefaults(
    defineProps<{
      toolStats?: ToolStats | null
      loading?: boolean
    }>(),
    {
      toolStats: null,
      loading: false
    }
  )

  const successRateStyle = computed(() => {
    const rate = props.toolStats?.success_rate || 0
    if (rate >= 90) return { color: 'var(--el-color-success)' }
    if (rate >= 70) return { color: 'var(--el-color-warning)' }
    return { color: 'var(--el-color-danger)' }
  })

  const miniStatItems = computed(() => [
    { key: 'total', label: '总调用次数', value: props.toolStats?.total_calls || 0 },
    {
      key: 'failed',
      label: '失败调用',
      value: props.toolStats?.failed_calls || 0,
      suffix: '次'
    },
    {
      key: 'success',
      label: '成功率',
      value: props.toolStats?.success_rate || 0,
      suffix: '%',
      valueStyle: successRateStyle.value
    }
  ])

  const topTools = computed(() => {
    const items = [...(props.toolStats?.most_used_tools || [])]
    return items.sort((a, b) => b.count - a.count).slice(0, 10)
  })

  const topToolNames = computed(() =>
    [...topTools.value].sort((a, b) => a.count - b.count).map((item) => item.tool_name)
  )

  const topToolCounts = computed(() =>
    [...topTools.value].sort((a, b) => a.count - b.count).map((item) => item.count)
  )

  const hasErrorData = computed(() => {
    const distribution = props.toolStats?.tool_error_distribution
    return Boolean(distribution && Object.keys(distribution).length > 0)
  })

  const errorData = computed(() => {
    if (!hasErrorData.value) return []
    return Object.entries(props.toolStats!.tool_error_distribution!)
      .map(([tool_name, error_count]) => ({ tool_name, error_count }))
      .sort((a, b) => b.error_count - a.error_count)
  })

  /** 错误分布图按次数升序，便于水平柱状图自下而上阅读 */
  const errorChartItems = computed(() => [...errorData.value].slice(0, 5).reverse())

  const errorChartNames = computed(() => errorChartItems.value.map((item) => item.tool_name))

  const errorChartCounts = computed(() => errorChartItems.value.map((item) => item.error_count))
</script>

<style scoped>
  .tool-stats-card {
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

  .error-chart-title {
    margin-bottom: 8px;
    font-size: 13px;
    font-weight: 500;
    color: var(--art-gray-700);
  }
</style>
