<template>
  <div class="page-content !mb-5">
    <ToolsCardList />
  </div>
</template>

<script setup lang="ts">
  import ToolsCardList from '@/components/extensions/tools/ToolsCardList.vue'

  defineOptions({ name: 'ExtensionTools' })
</script>
