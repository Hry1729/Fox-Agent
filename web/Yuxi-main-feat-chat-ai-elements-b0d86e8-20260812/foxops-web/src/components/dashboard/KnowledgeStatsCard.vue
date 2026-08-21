<template>
  <div class="art-card knowledge-stats-card" v-loading="loading">
    <div class="card-title">知识库使用情况</div>

    <DashboardMiniStats :items="miniStatItems" />

    <div class="section-block">
      <div class="section-title">文件类型分布</div>
      <ArtRingChart
        v-if="ringChartData.length"
        :data="ringChartData"
        :radius="['50%', '72%']"
        :center-text="String(knowledgeStats?.total_files || 0)"
        :show-legend="true"
        legend-position="bottom"
        height="280px"
      />
      <div v-else class="empty-tip">暂无文件类型数据</div>
    </div>
  </div>
</template>

<script setup lang="ts">
  import DashboardMiniStats from '@/components/dashboard/DashboardMiniStats.vue'

  defineOptions({ name: 'KnowledgeStatsCard' })

  interface KnowledgeStats {
    total_databases?: number
    total_files?: number
    total_storage_size?: number
    file_type_distribution?: Record<string, number>
  }

  const props = withDefaults(
    defineProps<{
      knowledgeStats?: KnowledgeStats | null
      loading?: boolean
    }>(),
    {
      knowledgeStats: null,
      loading: false
    }
  )

  /** 将字节拆成数值与单位，单位走小字号 suffix */
  const formattedStorage = computed(() => {
    const size = props.knowledgeStats?.total_storage_size || 0
    if (size < 1024) return { value: String(size), suffix: 'B' }
    if (size < 1024 * 1024) return { value: (size / 1024).toFixed(2), suffix: 'KB' }
    if (size < 1024 * 1024 * 1024) {
      return { value: (size / (1024 * 1024)).toFixed(2), suffix: 'MB' }
    }
    return { value: (size / (1024 * 1024 * 1024)).toFixed(2), suffix: 'GB' }
  })

  const miniStatItems = computed(() => [
    {
      key: 'databases',
      label: '知识库总数',
      value: props.knowledgeStats?.total_databases || 0,
      suffix: '个'
    },
    {
      key: 'files',
      label: '文件总数',
      value: props.knowledgeStats?.total_files || 0,
      suffix: '个'
    },
    {
      key: 'storage',
      label: '存储容量',
      value: formattedStorage.value.value,
      suffix: formattedStorage.value.suffix
    }
  ])

  const ringChartData = computed(() => {
    const distribution = props.knowledgeStats?.file_type_distribution || {}
    return Object.entries(distribution)
      .map(([type, count]) => ({
        name: type || '未知',
        value: count
      }))
      .sort((a, b) => b.value - a.value)
  })
</script>

<style scoped>
  .knowledge-stats-card {
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
    height: 240px;
    font-size: 14px;
    color: var(--art-gray-500);
  }
</style>
