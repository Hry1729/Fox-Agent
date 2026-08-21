<template>
  <div class="page-content knowledge-list-page !mb-5">
    <KnowledgeCardList />
  </div>
</template>

<script setup lang="ts">
  import KnowledgeCardList from '@/components/extensions/knowledge/KnowledgeCardList.vue'

  defineOptions({ name: 'ExtensionKnowledge' })
</script>

<style scoped>
  .knowledge-list-page {
    border-left: none !important;
  }
</style>
