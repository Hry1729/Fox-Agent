<template>
  <div class="tools-card-list">
    <ExtensionToolbar
      v-model:search="searchQuery"
      search-placeholder="搜索工具..."
      :loading="loading"
      @refresh="fetchTools"
    >
      <template #filters>
        <ElSelect v-model="selectedCategory" clearable placeholder="全部分类" style="width: 140px">
          <ElOption label="全部分类" value="" />
          <ElOption
            v-for="cat in categories"
            :key="cat"
            :label="categoryLabels[cat] || cat"
            :value="cat"
          />
        </ElSelect>
      </template>
    </ExtensionToolbar>

    <ExtensionEmptyState
      v-if="!loading && !filteredTools.length"
      :description="searchQuery ? '无匹配工具' : '暂无工具'"
    />

    <ExtensionCardGrid v-else>
      <ExtensionInfoCard
        v-for="tool in filteredTools"
        :key="getToolSlug(tool)"
        :title="formatExtensionCardTitle(tool.name)"
        :subtitle="getToolSlug(tool)"
        :description="tool.description || '无描述'"
        icon="ri:tools-line"
        accent="blue"
        :tags="toolTags(tool)"
        @click="openDetail(tool)"
      />
    </ExtensionCardGrid>

    <ToolDetailDialog v-model="detailVisible" :tool="currentTool" />
  </div>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { toolsApi, type SystemTool } from '@/api/tools'
  import ExtensionCardGrid from '@/components/extensions/common/ExtensionCardGrid.vue'
  import ExtensionEmptyState from '@/components/extensions/common/ExtensionEmptyState.vue'
  import ExtensionInfoCard from '@/components/extensions/common/ExtensionInfoCard.vue'
  import ExtensionToolbar from '@/components/extensions/common/ExtensionToolbar.vue'
  import ToolDetailDialog from '@/components/extensions/tools/ToolDetailDialog.vue'
  import { formatExtensionCardTitle } from '@/utils/extensionDisplayName'
  import { unwrapList } from '@/utils/apiData'

  const loading = ref(false)
  const searchQuery = ref('')
  const selectedCategory = ref('')
  const tools = ref<SystemTool[]>([])
  const currentTool = ref<SystemTool | null>(null)
  const detailVisible = ref(false)

  const categories = ['buildin', 'knowledge', 'mysql', 'debug']
  const categoryLabels: Record<string, string> = {
    buildin: '内置工具',
    knowledge: '知识库',
    mysql: 'MySQL',
    debug: '调试'
  }
  // 与 Yuxi ToolsCardList categoryColors 对齐
  const categoryColors: Record<string, 'blue' | 'purple' | 'green' | 'orange'> = {
    buildin: 'blue',
    knowledge: 'purple',
    mysql: 'green',
    debug: 'orange'
  }

  const getToolSlug = (tool: SystemTool) => tool.slug || tool.id || ''
  const toolTags = (tool: SystemTool) => {
    const tags: Array<string | { name: string; color?: 'blue' | 'purple' | 'green' | 'orange' }> =
      []
    if (tool.category) {
      tags.push({
        name: categoryLabels[tool.category] || tool.category,
        color: categoryColors[tool.category] || 'blue'
      })
    }
    ;(tool.tags || []).slice(0, 2).forEach((tag) => tags.push(tag))
    return tags
  }

  const filteredTools = computed(() => {
    let result = tools.value
    if (selectedCategory.value) {
      result = result.filter((item) => item.category === selectedCategory.value)
    }
    if (searchQuery.value) {
      const q = searchQuery.value.toLowerCase()
      result = result.filter(
        (item) =>
          item.name.toLowerCase().includes(q) ||
          getToolSlug(item).toLowerCase().includes(q) ||
          item.description?.toLowerCase().includes(q) ||
          item.config_guide?.toLowerCase().includes(q)
      )
    }
    return result
  })

  const openDetail = (tool: SystemTool) => {
    currentTool.value = tool
    detailVisible.value = true
  }

  const fetchTools = async () => {
    loading.value = true
    try {
      const result = await toolsApi.getTools()
      tools.value = unwrapList<SystemTool>(result)
    } catch {
      ElMessage.error('加载工具失败')
      tools.value = []
    } finally {
      loading.value = false
    }
  }

  onMounted(fetchTools)
  defineExpose({ fetchTools, loading })
</script>
