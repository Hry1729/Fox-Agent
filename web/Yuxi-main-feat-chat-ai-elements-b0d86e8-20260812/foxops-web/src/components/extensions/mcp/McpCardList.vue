<template>
  <div class="mcp-card-list">
    <ExtensionToolbar
      v-model:search="searchQuery"
      search-placeholder="搜索 MCP..."
      :loading="loading"
      @refresh="fetchServers"
    >
      <template #actions>
        <ElButton type="primary" @click="formVisible = true">
          <ArtSvgIcon icon="ri:add-line" />
          添加 MCP
        </ElButton>
      </template>
    </ExtensionToolbar>

    <ExtensionEmptyState
      v-if="!loading && !filteredEnabled.length && !filteredDisabled.length"
      :description="searchQuery ? '无匹配 MCP' : '暂无 MCP，点击上方按钮添加'"
    />

    <template v-else>
      <div v-if="filteredEnabled.length" class="section-title">已添加</div>
      <ExtensionCardGrid v-if="filteredEnabled.length" class="extension-card-grid--dense">
        <ExtensionInfoCard
          v-for="server in filteredEnabled"
          :key="server.slug"
          :title="formatExtensionCardTitle(server.name)"
          :subtitle="server.slug"
          :description="server.description || '暂无描述'"
          accent="cyan"
          icon="ri:plug-line"
          :tags="mcpTags(server)"
          show-arrow
          @click="goDetail(server.slug)"
        >
          <template #icon>
            <span class="emoji">{{ server.icon || '🔌' }}</span>
          </template>
          <template #action>
            <ElButton
              text
              type="danger"
              :loading="actionSlug === server.slug"
              @click.stop="removeServer(server)"
            >
              <ArtSvgIcon icon="ri:delete-bin-line" />
            </ElButton>
          </template>
        </ExtensionInfoCard>
      </ExtensionCardGrid>

      <div v-if="filteredDisabled.length" class="section-title">可添加</div>
      <ExtensionCardGrid v-if="filteredDisabled.length" class="extension-card-grid--dense">
        <ExtensionInfoCard
          v-for="server in filteredDisabled"
          :key="server.slug"
          :title="formatExtensionCardTitle(server.name)"
          :subtitle="server.slug"
          :description="server.description || '暂无描述'"
          accent="cyan"
          icon="ri:plug-line"
          :tags="mcpTags(server)"
          @click="openPreview(server)"
        >
          <template #icon>
            <span class="emoji">{{ server.icon || '🔌' }}</span>
          </template>
          <template #action>
            <ElButton
              text
              type="primary"
              :loading="actionSlug === server.slug"
              @click.stop="setEnabled(server, true)"
            >
              <ArtSvgIcon icon="ri:add-line" />
            </ElButton>
          </template>
        </ExtensionInfoCard>
      </ExtensionCardGrid>
    </template>

    <ElDialog
      v-model="previewVisible"
      width="760px"
      destroy-on-close
      :show-close="false"
      class="extension-dialog extension-card-detail-dialog extension-card-detail-dialog--mcp"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader title="MCP 基础信息" @close="previewVisible = false" />
      </template>
      <div v-if="previewServer" class="mcp-preview">
        <p><strong>描述：</strong>{{ previewServer.description || '暂无描述' }}</p>
        <p><strong>传输类型：</strong>{{ previewServer.transport || '-' }}</p>
        <p><strong>创建人：</strong>{{ previewServer.created_by || '-' }}</p>
      </div>
      <template #footer>
        <ElButton @click="previewVisible = false">关闭</ElButton>
        <ElButton
          type="primary"
          :loading="actionSlug === previewServer?.slug"
          @click="previewServer && setEnabled(previewServer, true)"
        >
          添加
        </ElButton>
      </template>
    </ElDialog>

    <McpFormDialog v-model="formVisible" @submitted="fetchServers" />
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { useRouter } from 'vue-router'
  import { mcpApi, type McpServer } from '@/api/mcp'
  import ExtensionCardGrid from '@/components/extensions/common/ExtensionCardGrid.vue'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import ExtensionEmptyState from '@/components/extensions/common/ExtensionEmptyState.vue'
  import ExtensionInfoCard from '@/components/extensions/common/ExtensionInfoCard.vue'
  import ExtensionToolbar from '@/components/extensions/common/ExtensionToolbar.vue'
  import { formatExtensionCardTitle } from '@/utils/extensionDisplayName'
  import { unwrapList } from '@/utils/apiData'
  import McpFormDialog from './McpFormDialog.vue'

  const router = useRouter()
  const loading = ref(false)
  const servers = ref<McpServer[]>([])
  const searchQuery = ref('')
  const formVisible = ref(false)
  const previewVisible = ref(false)
  const previewServer = ref<McpServer | null>(null)
  const actionSlug = ref('')

  const filtered = computed(() => {
    const sorted = [...servers.value].sort((a, b) =>
      String(a.name || '').localeCompare(String(b.name || ''), 'zh-Hans-CN')
    )
    if (!searchQuery.value) return sorted
    const q = searchQuery.value.toLowerCase()
    return sorted.filter(
      (item) =>
        item.name?.toLowerCase().includes(q) ||
        item.slug?.toLowerCase().includes(q) ||
        item.description?.toLowerCase().includes(q) ||
        (item.tags || []).some((tag) => tag.toLowerCase().includes(q))
    )
  })
  const filteredEnabled = computed(() => filtered.value.filter((item) => !!item.enabled))
  const filteredDisabled = computed(() => filtered.value.filter((item) => !item.enabled))

  const transportLabels: Record<string, string> = {
    streamable_http: 'HTTP',
    sse: 'SSE',
    stdio: 'STDIO'
  }

  const mcpTags = (server: McpServer) => {
    const tags: Array<{ name: string; color?: 'blue' | 'purple' | 'green' | 'orange' }> = []
    if (server.enabled) tags.push({ name: '已添加', color: 'green' })
    else tags.push({ name: '可添加', color: 'blue' })
    if (server.transport) {
      tags.push({
        name: transportLabels[String(server.transport)] || String(server.transport),
        color: 'purple'
      })
    }
    return tags
  }

  const fetchServers = async () => {
    loading.value = true
    try {
      const result = await mcpApi.getServers()
      // Yuxi 后端返回 { success, data: McpServer[] }
      servers.value = unwrapList<McpServer>(result)
    } catch {
      ElMessage.error('加载 MCP 失败')
      servers.value = []
    } finally {
      loading.value = false
    }
  }

  const goDetail = (slug: string) => router.push(`/extensions/mcp/${slug}`)
  const openPreview = (server: McpServer) => {
    previewServer.value = server
    previewVisible.value = true
  }

  const setEnabled = async (server: McpServer, enabled: boolean) => {
    actionSlug.value = server.slug
    try {
      await mcpApi.updateServerStatus(server.slug, enabled)
      ElMessage.success(enabled ? '已添加' : '已移除')
      previewVisible.value = false
      await fetchServers()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '操作失败')
    } finally {
      actionSlug.value = ''
    }
  }

  const removeServer = async (server: McpServer) => {
    const isSystem = server.created_by === 'system'
    try {
      await ElMessageBox.confirm(
        isSystem ? '系统 MCP 将禁用而非删除，确认继续？' : '确认删除该 MCP？',
        isSystem ? '移除 MCP' : '删除 MCP',
        { type: 'warning' }
      )
      actionSlug.value = server.slug
      if (isSystem) await mcpApi.updateServerStatus(server.slug, false)
      else await mcpApi.deleteServer(server.slug)
      ElMessage.success(isSystem ? '已移除' : '已删除')
      await fetchServers()
    } catch {
      // 取消
    } finally {
      actionSlug.value = ''
    }
  }

  onMounted(fetchServers)
  defineExpose({ fetchServers, loading })
</script>

<style scoped>
  .section-title {
    margin: 8px 0 12px;
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-secondary);
  }

  :deep(.extension-card-grid--dense) {
    gap: 12px;
  }

  @media (min-width: 1080px) {
    :deep(.extension-card-grid--dense) {
      grid-template-columns: repeat(4, minmax(0, 1fr));
    }
  }

  .emoji {
    font-size: 18px;
    line-height: 1;
  }

  .mcp-preview {
    display: flex;
    flex-direction: column;
    gap: 12px;
    line-height: 1.6;
  }

  .mcp-preview p {
    margin: 0;
    word-break: break-word;
  }
</style>
