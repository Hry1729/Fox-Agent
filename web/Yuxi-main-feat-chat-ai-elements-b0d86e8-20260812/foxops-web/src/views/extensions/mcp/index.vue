<template>
  <div class="page-content !mb-5">
    <McpCardList />
  </div>
</template>

<script setup lang="ts">
  import McpCardList from '@/components/extensions/mcp/McpCardList.vue'

  defineOptions({ name: 'ExtensionMcp' })
</script>
