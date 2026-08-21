<template>
  <div class="dashboard-mini-stats">
    <div
      v-for="item in items"
      :key="item.key || item.label"
      class="art-card mini-stat-card"
    >
      <div class="mini-value" :class="item.valueClass" :style="item.valueStyle">
        <span>{{ item.value }}</span>
        <span v-if="item.suffix" class="mini-suffix">{{ item.suffix }}</span>
      </div>
      <div class="mini-label">{{ item.label }}</div>
    </div>
  </div>
</template>

<script setup lang="ts">
  export interface MiniStatItem {
    key?: string
    label: string
    value: string | number
    suffix?: string
    valueClass?: string
    valueStyle?: Record<string, string>
  }

  defineOptions({ name: 'DashboardMiniStats' })

  defineProps<{
    items: MiniStatItem[]
  }>()
</script>

<style scoped>
  .dashboard-mini-stats {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 10px;
    margin-bottom: 16px;
  }

  .mini-stat-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 4.5rem;
    padding: 12px 10px;
    text-align: center;
  }

  .mini-value {
    font-size: 1.35rem;
    font-weight: 600;
    line-height: 1.2;
    color: var(--art-gray-900);
    word-break: break-all;
  }

  .mini-suffix {
    margin-left: 3px;
    font-size: 0.7rem;
    font-weight: 500;
    color: var(--art-gray-600);
  }

  .mini-label {
    margin-top: 4px;
    font-size: 0.75rem;
    color: var(--art-gray-600);
  }
</style>
