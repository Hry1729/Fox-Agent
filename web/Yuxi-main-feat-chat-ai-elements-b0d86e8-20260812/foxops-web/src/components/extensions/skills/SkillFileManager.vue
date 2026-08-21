<template>
  <div class="skill-file-manager" v-loading="treeLoading">
    <aside class="tree-pane">
      <div class="tree-header">
        <span class="tree-label">项目结构</span>
        <div class="tree-actions">
          <ElTooltip v-if="editable" content="新建文件" placement="top">
            <button type="button" class="icon-btn" @click="createFile">
              <ArtSvgIcon icon="ri:file-add-line" />
            </button>
          </ElTooltip>
          <ElTooltip v-if="editable" content="新建目录" placement="top">
            <button type="button" class="icon-btn" @click="createDirectory">
              <ArtSvgIcon icon="ri:folder-add-line" />
            </button>
          </ElTooltip>
          <ElTooltip content="刷新" placement="top">
            <button type="button" class="icon-btn" @click="loadTree">
              <ArtSvgIcon icon="ri:refresh-line" />
            </button>
          </ElTooltip>
        </div>
      </div>
      <div class="tree-content">
        <ElTree
          v-if="treeData.length"
          ref="treeRef"
          :data="treeData"
          node-key="path"
          :props="{ label: 'name', children: 'children' }"
          highlight-current
          default-expand-all
          @node-click="openNode"
        >
          <template #default="{ data }">
            <span class="tree-node">
              <FileTypeIcon :name="data.name" :is-dir="data.is_dir" />
              <span class="tree-node-label" :title="data.path">{{ data.name }}</span>
            </span>
          </template>
        </ElTree>
        <ElEmpty v-else description="暂无文件" :image-size="72" />
      </div>
    </aside>

    <section class="editor-pane">
      <div class="editor-header">
        <div class="editor-title">
          <FileTypeIcon v-if="currentPath" :name="currentPath" />
          <span class="editor-path" :title="currentPath">
            {{ currentPath || '选择文件以开始编辑' }}
          </span>
        </div>
        <div class="editor-actions">
          <ElButton
            v-if="editable && currentPath && canPreviewMarkdown"
            size="small"
            :disabled="saving"
            @click="toggleEditing"
          >
            {{ editing ? '取消编辑' : '编辑' }}
          </ElButton>
          <ElButton
            v-if="editable && currentPath && (editing || !canPreviewMarkdown)"
            type="primary"
            size="small"
            :loading="saving"
            @click="saveFile"
          >
            保存
          </ElButton>
        </div>
      </div>
      <div class="editor-body">
        <ElInput
          v-if="currentPath && editing"
          v-model="content"
          type="textarea"
          class="file-editor"
          resize="none"
        />
        <div
          v-else-if="currentPath && canPreviewMarkdown"
          class="markdown-body"
          v-html="renderedHtml"
        ></div>
        <pre v-else-if="currentPath" class="text-preview">{{ content }}</pre>
        <ElEmpty v-else description="选择左侧文件以开始编辑" :image-size="88" />
      </div>
    </section>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import type { ElTree } from 'element-plus'
  import { skillsApi } from '@/api/skills'
  import FileTypeIcon from '@/components/workspace/FileTypeIcon.vue'
  import { ensureHighlightTheme, renderMarkdown } from '@/utils/markdown'
  import { unwrapApiData } from '@/utils/apiData'
  import 'katex/dist/katex.min.css'

  type SkillTreeNode = {
    name: string
    path: string
    is_dir: boolean
    children?: SkillTreeNode[]
  }

  const props = defineProps<{ slug: string; editable?: boolean }>()
  const treeRef = ref<InstanceType<typeof ElTree> | null>(null)
  const treeData = ref<SkillTreeNode[]>([])
  const treeLoading = ref(false)
  const currentPath = ref('')
  const content = ref('')
  const saving = ref(false)
  const editing = ref(false)

  const canPreviewMarkdown = computed(() => /\.(md|markdown|mdx)$/i.test(currentPath.value))
  const renderedHtml = computed(() => renderMarkdown(content.value))

  const normalizeTree = (nodes: any[]): SkillTreeNode[] =>
    (Array.isArray(nodes) ? nodes : []).map((node) => {
      const isDir = !!(node.is_dir || node.type === 'directory')
      const path = String(node.path || node.name || '').replace(/\\/g, '/')
      const item: SkillTreeNode = {
        name: node.name || path.split('/').pop() || path,
        path,
        is_dir: isDir
      }
      if (isDir) {
        item.children = normalizeTree(node.children || [])
      }
      return item
    })

  const extractTreeNodes = (payload: any): any[] => {
    const data = unwrapApiData(payload, payload)
    if (Array.isArray(data)) return data
    if (Array.isArray(data?.entries)) return data.entries
    if (Array.isArray(data?.tree)) return data.tree
    return []
  }

  const findNodeByPath = (nodes: SkillTreeNode[], path: string): SkillTreeNode | null => {
    for (const node of nodes) {
      if (node.path === path) return node
      if (node.children?.length) {
        const found = findNodeByPath(node.children, path)
        if (found) return found
      }
    }
    return null
  }

  const findFirstFile = (nodes: SkillTreeNode[]): SkillTreeNode | null => {
    for (const node of nodes) {
      if (!node.is_dir) return node
      if (node.children?.length) {
        const found = findFirstFile(node.children)
        if (found) return found
      }
    }
    return null
  }

  const loadTree = async () => {
    if (!props.slug) return
    treeLoading.value = true
    try {
      const result = await skillsApi.getTree(props.slug)
      treeData.value = normalizeTree(extractTreeNodes(result))
      await nextTick()
      await pickDefaultFile()
    } catch (error) {
      treeData.value = []
      ElMessage.error((error as Error)?.message || '加载文件树失败')
    } finally {
      treeLoading.value = false
    }
  }

  const openNode = async (node: SkillTreeNode) => {
    if (!node || node.is_dir) return
    currentPath.value = node.path
    editing.value = false
    treeRef.value?.setCurrentKey(node.path)
    try {
      const result = await skillsApi.getFile(props.slug, node.path)
      const data = unwrapApiData(result, result)
      content.value = data?.content || (typeof data === 'string' ? data : '') || ''
    } catch (error) {
      content.value = ''
      ElMessage.error((error as Error)?.message || '读取文件失败')
    }
  }

  const pickDefaultFile = async () => {
    const preferred = findNodeByPath(treeData.value, 'SKILL.md') || findFirstFile(treeData.value)
    if (!preferred) {
      currentPath.value = ''
      content.value = ''
      editing.value = false
      return
    }
    await openNode(preferred)
  }

  const toggleEditing = () => {
    if (saving.value) return
    editing.value = !editing.value
  }

  const saveFile = async () => {
    if (!currentPath.value) return
    saving.value = true
    try {
      await skillsApi.updateFile(props.slug, { path: currentPath.value, content: content.value })
      ElMessage.success('已保存')
      editing.value = false
    } catch (error) {
      ElMessage.error((error as Error)?.message || '保存失败')
    } finally {
      saving.value = false
    }
  }

  const createFile = async () => {
    try {
      const { value } = await ElMessageBox.prompt('请输入文件路径', '新建文件')
      const path = value?.trim()
      if (!path) return
      await skillsApi.createFile(props.slug, { path, content: '', is_dir: false })
      await loadTree()
      const node = findNodeByPath(treeData.value, path)
      if (node) await openNode(node)
    } catch {
      // 取消
    }
  }

  const createDirectory = async () => {
    try {
      const { value } = await ElMessageBox.prompt('请输入目录路径', '新建目录')
      const path = value?.trim()
      if (!path) return
      await skillsApi.createFile(props.slug, { path, is_dir: true })
      await loadTree()
    } catch {
      // 取消
    }
  }

  onMounted(() => ensureHighlightTheme())

  watch(
    () => props.slug,
    () => {
      currentPath.value = ''
      content.value = ''
      editing.value = false
      loadTree()
    },
    { immediate: true }
  )
</script>

<style scoped>
  .skill-file-manager {
    display: grid;
    grid-template-columns: 280px minmax(0, 1fr);
    min-height: 0;
    height: 100%;
    overflow: hidden;
  }

  .tree-pane,
  .editor-pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    background: var(--el-bg-color);
  }

  .tree-pane {
    border-right: 1px solid var(--el-border-color-lighter);
  }

  .tree-header,
  .editor-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 44px;
    padding: 0 12px;
    border-bottom: 1px solid var(--el-border-color-lighter);
    flex-shrink: 0;
  }

  .tree-label {
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .tree-actions,
  .editor-actions {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--el-text-color-regular);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease;
  }

  .icon-btn:hover {
    background: var(--el-fill-color-light);
    color: var(--el-color-primary);
  }

  .tree-content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 8px 6px 12px;
  }

  .tree-content :deep(.el-tree-node__content) {
    height: 32px;
  }

  .tree-node {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    width: 100%;
  }

  .tree-node-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    color: var(--el-text-color-primary);
  }

  .editor-title {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }

  .editor-path {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--el-text-color-primary);
    font-size: 13px;
    font-weight: 600;
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  }

  .editor-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 16px 18px;
  }

  .file-editor {
    height: 100%;
    min-height: 320px;
  }

  .file-editor :deep(textarea) {
    height: 100% !important;
    min-height: 320px;
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
    font-size: 13px;
    line-height: 1.6;
  }

  .text-preview {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font:
      13px/1.65 ui-monospace,
      SFMono-Regular,
      Consolas,
      monospace;
    color: var(--el-text-color-primary);
  }

  .markdown-body {
    font-size: 13px;
    line-height: 1.65;
    word-break: break-word;
    color: var(--el-text-color-primary);
  }
</style>

<style>
  .skill-file-manager .markdown-body {
    font-size: 13px;
    line-height: 1.65;
  }

  .skill-file-manager .markdown-body h1,
  .skill-file-manager .markdown-body h2 {
    margin: 8px 0 4px;
    font-size: 14px;
    font-weight: 600;
    line-height: 1.4;
  }

  .skill-file-manager .markdown-body h3,
  .skill-file-manager .markdown-body h4,
  .skill-file-manager .markdown-body h5,
  .skill-file-manager .markdown-body h6 {
    margin: 6px 0 4px;
    font-size: 13px;
    font-weight: 600;
    line-height: 1.4;
  }

  .skill-file-manager .markdown-body p {
    margin: 0 0 8px;
    font-size: 13px;
    line-height: 1.65;
  }

  .skill-file-manager .markdown-body ul,
  .skill-file-manager .markdown-body ol {
    list-style: revert;
    padding-left: 1.625rem;
    margin: 6px 0 10px;
  }

  .skill-file-manager .markdown-body ul {
    list-style-type: disc;
  }

  .skill-file-manager .markdown-body ol {
    list-style-type: decimal;
  }

  .skill-file-manager .markdown-body li {
    display: list-item;
    margin: 2px 0;
    font-size: 13px;
    line-height: 1.65;
  }

  .skill-file-manager .markdown-body pre {
    margin: 8px 0;
    padding: 10px 12px;
    border-radius: 8px;
    overflow: auto;
    font-size: 12px;
    line-height: 1.5;
  }

  .skill-file-manager .markdown-body :not(pre) > code {
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--el-fill-color);
    font-size: 11px;
  }
</style>
