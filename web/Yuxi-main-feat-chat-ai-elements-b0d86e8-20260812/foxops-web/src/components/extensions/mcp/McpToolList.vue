<template>
  <div class="mcp-tool-list">
    <ExtensionToolbar
      v-model:search="searchQuery"
      search-placeholder="搜索工具..."
      :loading="loading"
      @refresh="refresh"
    />
    <ExtensionEmptyState
      v-if="!loading && !filteredTools.length"
      :description="error || (searchQuery ? '无匹配工具' : '暂无工具')"
    />
    <ElTable v-else v-loading="loading" :data="filteredTools" size="small">
      <ElTableColumn prop="name" label="名称" min-width="160" />
      <ElTableColumn prop="description" label="描述" min-width="220" show-overflow-tooltip />
      <ElTableColumn label="启用" width="90">
        <template #default="{ row }">
          <ElSwitch
            :model-value="row.enabled !== false"
            @change="(value) => toggle(row.name, Boolean(value))"
          />
        </template>
      </ElTableColumn>
      <ElTableColumn label="操作" width="100">
        <template #default="{ row }">
          <ElButton text @click="copyName(row.name)">复制</ElButton>
        </template>
      </ElTableColumn>
    </ElTable>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { mcpApi, type McpToolItem } from '@/api/mcp'
  import ExtensionEmptyState from '@/components/extensions/common/ExtensionEmptyState.vue'
  import ExtensionToolbar from '@/components/extensions/common/ExtensionToolbar.vue'
  import { unwrapList } from '@/utils/apiData'

  const props = defineProps<{ slug: string }>()
  const loading = ref(false)
  const error = ref('')
  const tools = ref<McpToolItem[]>([])
  const searchQuery = ref('')

  const filteredTools = computed(() => {
    if (!searchQuery.value) return tools.value
    const q = searchQuery.value.toLowerCase()
    return tools.value.filter(
      (item) => item.name?.toLowerCase().includes(q) || item.description?.toLowerCase().includes(q)
    )
  })

  const load = async () => {
    loading.value = true
    error.value = ''
    try {
      const result = await mcpApi.getServerTools(props.slug)
      tools.value = unwrapList<McpToolItem>(result)
    } catch (err) {
      error.value = (err as Error)?.message || '加载工具失败'
      tools.value = []
    } finally {
      loading.value = false
    }
  }

  const refresh = async () => {
    loading.value = true
    try {
      await mcpApi.refreshServerTools(props.slug)
      await load()
      ElMessage.success('工具列表已刷新')
    } catch (err) {
      ElMessage.error((err as Error)?.message || '刷新失败')
      loading.value = false
    }
  }

  const toggle = async (toolName: string, enabled: boolean) => {
    try {
      await mcpApi.toggleServerTool(props.slug, toolName)
      await load()
      ElMessage.success(enabled ? '已启用' : '已禁用')
    } catch (err) {
      ElMessage.error((err as Error)?.message || '切换失败')
      await load()
    }
  }

  const copyName = async (name: string) => {
    await navigator.clipboard.writeText(name)
    ElMessage.success('已复制')
  }

  watch(() => props.slug, load, { immediate: true })
</script>
