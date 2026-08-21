<template>
  <div class="knowledge-card-list">
    <ExtensionToolbar
      v-model:search="searchQuery"
      search-placeholder="搜索知识库..."
      :loading="store.listLoading"
      @refresh="reload"
    >
      <template #filters>
        <ElSelect v-model="typeFilter" clearable placeholder="全部类型" style="width: 140px">
          <ElOption label="全部类型" value="" />
          <ElOption
            v-for="(info, key) in store.types"
            :key="key"
            :label="(info as any).label || key"
            :value="String(key)"
          />
        </ElSelect>
      </template>
      <template #actions>
        <ElButton
          type="primary"
          :disabled="!Object.keys(store.types).length"
          @click="createVisible = true"
        >
          新建知识库
        </ElButton>
      </template>
    </ExtensionToolbar>

    <ExtensionEmptyState
      v-if="!store.listLoading && !filteredDatabases.length"
      :description="emptyDescription"
    >
      <ElButton v-if="store.listError" type="primary" @click="reload">重新连接</ElButton>
      <ElButton v-else type="primary" @click="createVisible = true">新建知识库</ElButton>
    </ExtensionEmptyState>

    <ExtensionCardGrid v-else :min-width="236" class="knowledge-card-grid">
      <KnowledgeDatabaseCard
        v-for="db in filteredDatabases"
        :key="db.kb_id"
        :database="db"
        :types="store.types"
        @click="goDetail(db.kb_id)"
      />
    </ExtensionCardGrid>

    <KnowledgeCreateDialog v-model="createVisible" @created="onCreated" />
  </div>
</template>

<script setup lang="ts">
  import { useRouter } from 'vue-router'
  import ExtensionCardGrid from '@/components/extensions/common/ExtensionCardGrid.vue'
  import ExtensionEmptyState from '@/components/extensions/common/ExtensionEmptyState.vue'
  import ExtensionToolbar from '@/components/extensions/common/ExtensionToolbar.vue'
  import KnowledgeDatabaseCard from '@/components/extensions/knowledge/KnowledgeDatabaseCard.vue'
  import { useKnowledgeStore } from '@/store/modules/knowledge'
  import KnowledgeCreateDialog from './KnowledgeCreateDialog.vue'

  const router = useRouter()
  const store = useKnowledgeStore()
  const searchQuery = ref('')
  const typeFilter = ref('')
  const createVisible = ref(false)

  const emptyDescription = computed(() => {
    if (store.listError) return `知识库连接失败：${store.listError}`
    return searchQuery.value ? '无匹配知识库' : '暂无知识库'
  })

  defineExpose({ loading: computed(() => store.listLoading) })

  const filteredDatabases = computed(() => {
    return store.databases.filter((db) => {
      if (typeFilter.value && db.kb_type !== typeFilter.value) return false
      if (!searchQuery.value) return true
      const q = searchQuery.value.toLowerCase()
      return (
        db.name?.toLowerCase().includes(q) ||
        db.kb_id?.toLowerCase().includes(q) ||
        db.description?.toLowerCase().includes(q)
      )
    })
  })

  const goDetail = (kbId: string) => router.push(`/extensions/knowledgebase/${kbId}`)

  const onCreated = async () => {
    await reload()
  }

  const reload = async () => {
    await Promise.allSettled([store.loadTypes(), store.loadDatabases()])
  }

  onMounted(async () => {
    await reload()
  })
</script>

<style scoped>
  /* 常规桌面宽度下一行约 4 张卡片 */
  :deep(.knowledge-card-grid) {
    gap: 12px;
  }

  @media (min-width: 1080px) {
    :deep(.extension-card-grid.knowledge-card-grid) {
      grid-template-columns: repeat(4, minmax(0, 1fr));
    }
  }
</style>
