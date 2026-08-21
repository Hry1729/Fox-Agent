<template>
  <span class="file-type-icon" :style="{ fontSize: `${size}px` }">📄</span>
</template>

<script setup>
const props = defineProps({
  name: { type: String, default: '' },
  isDir: { type: Boolean, default: false },
  folderVariant: { type: String, default: 'default' },
  size: { type: Number, default: 16 }
})
</script>

<style scoped>
.file-type-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 1;
}
</style>
