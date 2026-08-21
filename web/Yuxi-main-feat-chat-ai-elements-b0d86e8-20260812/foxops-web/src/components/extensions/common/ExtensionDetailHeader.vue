<template>
  <header class="extension-detail-header">
    <div class="header-main">
      <ElButton text @click="$emit('back')">
        <ArtSvgIcon icon="ri:arrow-left-line" />
        返回
      </ElButton>
      <div class="header-titles">
        <h1>{{ title }}</h1>
        <p v-if="subtitle">{{ subtitle }}</p>
      </div>
    </div>
    <div class="header-actions">
      <slot name="actions" />
    </div>
  </header>
</template>

<script setup lang="ts">
  withDefaults(defineProps<{ title: string; subtitle?: string }>(), { subtitle: '' })
  defineEmits<{ back: [] }>()
</script>

<style scoped>
  .extension-detail-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--el-border-color-lighter);
  }
  .header-main {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .header-titles h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  .header-titles p {
    margin: 2px 0 0;
    color: var(--el-text-color-secondary);
    font-size: 12px;
  }
  .header-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
