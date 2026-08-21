<template>
  <div class="extension-card-grid" :style="{ '--card-min-width': `${minWidth}px` }">
    <slot />
  </div>
</template>

<script setup lang="ts">
  withDefaults(defineProps<{ minWidth?: number }>(), { minWidth: 280 })
</script>

<style scoped>
  .extension-card-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--card-min-width), 1fr));
    gap: 12px;
  }
</style>
