<template>
  <div class="art-card call-stats-card">
    <div class="card-header">
      <span class="card-title">调用统计</span>
      <div class="card-controls">
        <div class="toggle-group">
          <button
            v-for="opt in timeRangeOptions"
            :key="opt.value"
            type="button"
            class="toggle-btn"
            :class="{ active: callTimeRange === opt.value }"
            @click="switchTimeRange(opt.value)"
          >
            {{ opt.label }}
          </button>
        </div>
        <span class="divider" />
        <div class="toggle-group">
          <button
            v-for="opt in dataTypeOptions"
            :key="opt.value"
            type="button"
            class="toggle-btn"
            :class="{ active: callDataType === opt.value }"
            @click="switchDataType(opt.value)"
          >
            {{ opt.label }}
          </button>
        </div>
      </div>
    </div>

    <div v-loading="loading" class="chart-wrap">
      <ArtBarChart
        v-if="hasChartData"
        :key="chartInstanceKey"
        :data="stackBarData"
        :x-axis-data="xAxisData"
        :stack="true"
        :show-legend="true"
        legend-position="bottom"
        height="100%"
        bar-width="40%"
      />
      <div v-else-if="!loading" class="empty-tip">暂无调用统计数据</div>
    </div>
  </div>
</template>

<script setup lang="ts">
  import { dashboardApi } from '@/api/dashboard'

  defineOptions({ name: 'CallStatsCard' })

  type TimeRange = '14hours' | '14days' | '14weeks'
  type DataType = 'models' | 'agents' | 'tokens' | 'tools'

  interface TimeseriesPoint {
    date: string
    data: Record<string, number>
  }

  interface CallTimeseriesResponse {
    categories?: string[]
    data?: TimeseriesPoint[]
    agent_names?: Record<string, string>
  }

  const timeRangeOptions: { value: TimeRange; label: string }[] = [
    { value: '14hours', label: '近14小时' },
    { value: '14days', label: '近14天' },
    { value: '14weeks', label: '近14周' }
  ]

  const dataTypeOptions: { value: DataType; label: string }[] = [
    { value: 'models', label: '模型调用' },
    { value: 'agents', label: '智能体调用' },
    { value: 'tokens', label: 'Token消耗' },
    { value: 'tools', label: '工具调用' }
  ]

  const loading = ref(false)
  const callTimeRange = ref<TimeRange>('14days')
  const callDataType = ref<DataType>('agents')
  const callStatsData = ref<CallTimeseriesResponse | null>(null)

  /** 请求序号，防止并发切换导致旧响应覆盖新数据 */
  let loadSeq = 0

  const resolveCategoryLabel = (category: string, agentNames: Record<string, string>) => {
    if (category === 'None') return '未知模型'
    return agentNames[category] || category
  }

  const formatXAxisLabel = (date: string) => {
    if (callTimeRange.value === '14hours') {
      return date.split(' ')[1] || date
    }
    if (callTimeRange.value === '14weeks') {
      return `第${date.split('-')[1] || date}周`
    }
    return date.split('-').slice(1).join('-')
  }

  const xAxisData = computed(() => {
    const rows = callStatsData.value?.data || []
    return rows.map((item) => formatXAxisLabel(item.date))
  })

  const stackBarData = computed(() => {
    const payload = callStatsData.value
    if (!payload?.categories?.length || !payload.data?.length) return []

    const agentNames = payload.agent_names || {}
    return payload.categories.map((category) => ({
      name: resolveCategoryLabel(category, agentNames),
      data: payload.data!.map((item) => item.data?.[category] || 0)
    }))
  })

  const hasChartData = computed(() => stackBarData.value.length > 0 && xAxisData.value.length > 0)

  /** 切换筛选时强制重建图表实例，避免 ECharts 合并旧系列 */
  const chartInstanceKey = computed(
    () => `${callDataType.value}-${callTimeRange.value}-${stackBarData.value.length}`
  )

  const loadCallStats = async () => {
    const seq = ++loadSeq
    loading.value = true
    // 先清空，卸载旧图，防止切换时图例/系列叠乱
    callStatsData.value = null
    try {
      const response = (await dashboardApi.getCallTimeseries(
        callDataType.value,
        callTimeRange.value
      )) as CallTimeseriesResponse
      if (seq !== loadSeq) return
      callStatsData.value = response
    } catch (error) {
      if (seq !== loadSeq) return
      console.error('加载调用统计数据失败:', error)
      callStatsData.value = null
    } finally {
      if (seq === loadSeq) {
        loading.value = false
      }
    }
  }

  const switchTimeRange = (value: TimeRange) => {
    if (callTimeRange.value === value) return
    callTimeRange.value = value
    loadCallStats()
  }

  const switchDataType = (value: DataType) => {
    if (callDataType.value === value) return
    callDataType.value = value
    loadCallStats()
  }

  const reload = () => loadCallStats()

  defineExpose({ reload })

  onMounted(() => {
    loadCallStats()
  })
</script>

<style scoped>
  .call-stats-card {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 420px;
    padding: 16px 20px 12px;
  }

  .card-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-shrink: 0;
    margin-bottom: 8px;
  }

  .card-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--art-gray-900);
  }

  .card-controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }

  .toggle-group {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .toggle-btn {
    padding: 4px 10px;
    font-size: 12px;
    color: var(--art-gray-600);
    background: transparent;
    border: 1px solid var(--art-gray-200);
    border-radius: 6px;
    cursor: pointer;
    transition: all 0.2s ease;
  }

  .toggle-btn:hover {
    color: var(--art-gray-800);
    border-color: var(--art-gray-300);
  }

  .toggle-btn.active {
    color: #fff;
    background: var(--el-color-primary);
    border-color: var(--el-color-primary);
  }

  .divider {
    width: 1px;
    height: 16px;
    background: var(--art-gray-200);
  }

  .chart-wrap {
    position: relative;
    flex: 1 1 auto;
    min-height: 340px;
  }

  .chart-wrap :deep(> div) {
    position: absolute;
    inset: 0;
    height: 100% !important;
    width: 100%;
  }

  .empty-tip {
    display: flex;
    align-items: center;
    justify-content: center;
    position: absolute;
    inset: 0;
    font-size: 14px;
    color: var(--art-gray-500);
  }
</style>
