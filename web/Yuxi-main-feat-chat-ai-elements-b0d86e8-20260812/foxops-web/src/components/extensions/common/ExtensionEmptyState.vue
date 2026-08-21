<template>
  <div class="extension-empty-state">
    <ElEmpty :description="description">
      <slot />
    </ElEmpty>
  </div>
</template>

<script setup lang="ts">
  withDefaults(defineProps<{ description?: string }>(), { description: '暂无数据' })
</script>

<style scoped>
  .extension-empty-state {
    padding: 48px 16px;
  }
</style>
