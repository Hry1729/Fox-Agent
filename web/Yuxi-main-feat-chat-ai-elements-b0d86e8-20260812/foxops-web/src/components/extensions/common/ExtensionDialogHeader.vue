<template>
  <div class="extension-dialog-header-inner">
    <span class="extension-dialog-title">{{ title }}</span>
    <div class="extension-dialog-header-actions">
      <slot name="actions" />
      <button
        type="button"
        class="extension-dialog-close"
        aria-label="关闭"
        @click="$emit('close')"
      >
        <ArtSvgIcon icon="ri:close-line" />
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
  defineProps<{ title: string }>()
  defineEmits<{ close: [] }>()
</script>
