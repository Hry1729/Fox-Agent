<template>
  <div class="page-content mcp-detail !p-0" :style="{ height: containerMinHeight }">
    <ExtensionDetailHeader
      :title="server?.name || slug"
      :subtitle="server?.transport || ''"
      @back="goBack"
    >
      <template #actions>
        <ElButton :loading="testing" @click="testConnection">测试</ElButton>
        <ElButton @click="openEdit">编辑</ElButton>
        <ElButton :type="server?.enabled === false ? 'primary' : 'danger'" @click="handleDanger">
          {{
            server?.enabled === false ? '添加' : server?.created_by === 'system' ? '移除' : '删除'
          }}
        </ElButton>
      </template>
    </ExtensionDetailHeader>

    <div v-loading="loading" class="detail-body">
      <ElTabs v-if="server" v-model="activeTab">
        <ElTabPane label="信息" name="info">
          <ElDescriptions :column="1" border>
            <ElDescriptionsItem label="标识">{{ server.slug }}</ElDescriptionsItem>
            <ElDescriptionsItem label="描述">{{ server.description || '-' }}</ElDescriptionsItem>
            <ElDescriptionsItem label="传输">{{ server.transport || '-' }}</ElDescriptionsItem>
            <ElDescriptionsItem label="URL">{{ server.url || '-' }}</ElDescriptionsItem>
            <ElDescriptionsItem label="命令">{{ server.command || '-' }}</ElDescriptionsItem>
            <ElDescriptionsItem label="创建人">{{ server.created_by || '-' }}</ElDescriptionsItem>
            <ElDescriptionsItem label="标签">
              {{ (server.tags || []).join(', ') || '-' }}
            </ElDescriptionsItem>
          </ElDescriptions>
        </ElTabPane>
        <ElTabPane label="工具" name="tools">
          <McpToolList :slug="slug" />
        </ElTabPane>
      </ElTabs>
      <ElEmpty v-else-if="!loading" description="MCP 不存在或无权访问" />
    </div>

    <McpFormDialog v-model="editVisible" edit-mode :edit-data="server" @submitted="loadServer" />
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { useRoute, useRouter } from 'vue-router'
  import { mcpApi, type McpServer } from '@/api/mcp'
  import ExtensionDetailHeader from '@/components/extensions/common/ExtensionDetailHeader.vue'
  import McpFormDialog from '@/components/extensions/mcp/McpFormDialog.vue'
  import McpToolList from '@/components/extensions/mcp/McpToolList.vue'
  import { useAutoLayoutHeight } from '@/hooks/core/useLayoutHeight'
  import { useUserStore } from '@/store/modules/user'
  import { unwrapApiData } from '@/utils/apiData'

  defineOptions({ name: 'ExtensionMcpDetail' })

  const route = useRoute()
  const router = useRouter()
  const userStore = useUserStore()
  const { containerMinHeight } = useAutoLayoutHeight()

  const slug = computed(() => String(route.params.slug || ''))
  const loading = ref(false)
  const testing = ref(false)
  const server = ref<McpServer | null>(null)
  const activeTab = ref('info')
  const editVisible = ref(false)

  const goBack = () => router.push('/extensions/mcp')

  const loadServer = async () => {
    if (!userStore.isAdmin) {
      router.replace('/extensions/skills')
      return
    }
    loading.value = true
    try {
      const result = await mcpApi.getServer(slug.value)
      server.value = unwrapApiData<McpServer>(result)
    } catch {
      server.value = null
      ElMessage.error('加载 MCP 详情失败')
    } finally {
      loading.value = false
    }
  }

  const testConnection = async () => {
    testing.value = true
    try {
      await mcpApi.testServer(slug.value)
      ElMessage.success('连接测试成功')
    } catch (error) {
      ElMessage.error((error as Error)?.message || '连接测试失败')
    } finally {
      testing.value = false
    }
  }

  const openEdit = () => {
    editVisible.value = true
  }

  const handleDanger = async () => {
    if (!server.value) return
    if (server.value.enabled === false) {
      await mcpApi.updateServerStatus(slug.value, true)
      ElMessage.success('已添加')
      await loadServer()
      return
    }
    const isSystem = server.value.created_by === 'system'
    try {
      await ElMessageBox.confirm(
        isSystem ? '确认移除（禁用）该系统 MCP？' : '确认删除该 MCP？',
        '确认',
        { type: 'warning' }
      )
      if (isSystem) await mcpApi.updateServerStatus(slug.value, false)
      else await mcpApi.deleteServer(slug.value)
      ElMessage.success(isSystem ? '已移除' : '已删除')
      goBack()
    } catch {
      // 取消
    }
  }

  watch(slug, loadServer, { immediate: true })
</script>

<style scoped>
  .mcp-detail {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
  }
  .detail-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 16px;
  }
</style>
