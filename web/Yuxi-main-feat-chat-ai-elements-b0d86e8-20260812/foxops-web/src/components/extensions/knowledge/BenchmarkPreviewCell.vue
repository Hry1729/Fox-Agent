<template>
  <ElTooltip
    v-if="displayText"
    :content="displayText"
    placement="top"
    :show-after="280"
    popper-class="benchmark-preview-tooltip"
  >
    <div class="benchmark-preview-cell">{{ displayText }}</div>
  </ElTooltip>
  <span v-else class="benchmark-preview-cell is-empty">{{ placeholder }}</span>
</template>

<script setup lang="ts">
  const props = withDefaults(
    defineProps<{
      text?: string
      placeholder?: string
    }>(),
    {
      text: '',
      placeholder: '-'
    }
  )

  const displayText = computed(() => String(props.text || '').trim())
</script>

<style scoped>
  .benchmark-preview-cell {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1.5;
    font-size: 13px;
    color: var(--el-text-color-regular);
  }

  .benchmark-preview-cell.is-empty {
    color: var(--el-text-color-placeholder);
  }
</style>
