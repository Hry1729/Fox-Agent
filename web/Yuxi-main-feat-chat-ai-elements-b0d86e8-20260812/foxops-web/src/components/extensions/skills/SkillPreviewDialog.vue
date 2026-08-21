<template>
  <ElDialog
    :model-value="modelValue"
    width="680px"
    destroy-on-close
    :show-close="false"
    class="skill-preview-dialog"
    @close="$emit('update:modelValue', false)"
  >
    <div v-if="skill" class="skill-preview-panel">
      <div class="skill-preview-header">
        <div class="skill-preview-title-area">
          <div class="skill-preview-icon">
            <ArtSvgIcon icon="ri:magic-line" />
          </div>
          <div class="skill-preview-title-text">
            <div class="skill-preview-title">
              {{ formatExtensionCardTitle(skill.name || skill.slug) }}
            </div>
            <div class="skill-preview-meta">
              <span>{{ sourceTypeLabel(skill) }} Skill</span>
              <span v-if="skill.enabled === false" class="skill-preview-disabled-tag">已禁用</span>
            </div>
          </div>
        </div>
        <div v-if="skill.can_manage !== false" class="skill-preview-actions">
          <ElSwitch
            :model-value="skill.enabled !== false"
            size="small"
            :loading="toggling"
            @change="toggleEnabled"
          />
        </div>
      </div>

      <div v-loading="loading" class="skill-preview-body">
        <div v-if="content" class="markdown-body" v-html="renderedHtml"></div>
        <ElEmpty v-else :description="loadError || '未读取到 SKILL.md'" />
      </div>

      <div class="skill-preview-footer">
        <div class="skill-preview-footer-left">
          <ElButton
            v-if="canDelete"
            type="danger"
            :loading="deleting"
            @click="confirmDelete"
          >
            卸载
          </ElButton>
        </div>
        <div class="skill-preview-footer-right">
          <ElButton @click="$emit('update:modelValue', false)">关闭</ElButton>
          <ElButton type="primary" @click="goManage">去管理</ElButton>
        </div>
      </div>
    </div>
  </ElDialog>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { useRouter } from 'vue-router'
  import { skillsApi, type SkillItem } from '@/api/skills'
  import { formatExtensionCardTitle } from '@/utils/extensionDisplayName'
  import { ensureHighlightTheme, renderMarkdown } from '@/utils/markdown'
  import { unwrapApiData } from '@/utils/apiData'
  import 'katex/dist/katex.min.css'

  const props = defineProps<{ modelValue: boolean; skill?: SkillItem | null }>()
  const emit = defineEmits<{ 'update:modelValue': [value: boolean]; changed: [] }>()
  const router = useRouter()
  const loading = ref(false)
  const toggling = ref(false)
  const deleting = ref(false)
  const content = ref('')
  const loadError = ref('')

  const renderedHtml = computed(() => renderMarkdown(content.value))

  const isBuiltin = (skill?: SkillItem | null) =>
    !!(skill?.is_builtin || skill?.source_type === 'builtin' || skill?.source === 'builtin')

  const sourceTypeLabel = (skill: SkillItem) => {
    if (isBuiltin(skill)) return '内置'
    if (skill.source_type === 'remote' || skill.source === 'remote') return '远程'
    return '上传'
  }

  const canDelete = computed(
    () => props.skill?.can_manage !== false && !isBuiltin(props.skill)
  )

  const load = async () => {
    if (!props.skill?.slug) return
    loading.value = true
    loadError.value = ''
    content.value = ''
    try {
      const result: any = await skillsApi.getFile(props.skill.slug, 'SKILL.md')
      const data = unwrapApiData(result, result)
      content.value = data?.content || (typeof data === 'string' ? data : '') || ''
      if (!content.value) loadError.value = '未读取到 SKILL.md'
    } catch (error) {
      loadError.value = (error as Error)?.message || '读取 SKILL.md 失败'
      content.value = ''
    } finally {
      loading.value = false
    }
  }

  const toggleEnabled = async (value: string | number | boolean) => {
    if (!props.skill) return
    const enabled = Boolean(value)
    toggling.value = true
    try {
      await skillsApi.updateEnabled(props.skill.slug, enabled)
      ElMessage.success(enabled ? '已启用' : '已禁用')
      emit('changed')
    } catch (error) {
      ElMessage.error((error as Error)?.message || '更新失败')
    } finally {
      toggling.value = false
    }
  }

  const confirmDelete = async () => {
    if (!props.skill || !canDelete.value) return
    try {
      await ElMessageBox.confirm(`确认卸载 Skill「${props.skill.name || props.skill.slug}」？`, '卸载 Skill', {
        type: 'warning'
      })
      deleting.value = true
      await skillsApi.deleteSkill(props.skill.slug)
      ElMessage.success('Skill 已卸载')
      emit('update:modelValue', false)
      emit('changed')
    } catch {
      // 取消或失败
    } finally {
      deleting.value = false
    }
  }

  const goManage = () => {
    if (!props.skill) return
    emit('update:modelValue', false)
    router.push(`/extensions/skill/${props.skill.slug}`)
  }

  watch(
    () => [props.modelValue, props.skill?.slug],
    ([open]) => {
      if (open) {
        ensureHighlightTheme()
        load()
      }
    }
  )
</script>

<style scoped>
  .skill-preview-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .skill-preview-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 14px;
  }

  .skill-preview-title-area {
    display: flex;
    align-items: center;
    min-width: 0;
    gap: 10px;
  }

  .skill-preview-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 32px;
    height: 32px;
    border-radius: 9px;
    background: var(--el-color-primary-light-9);
    color: var(--el-color-primary);
    font-size: 18px;
  }

  .skill-preview-title-text {
    min-width: 0;
  }

  .skill-preview-title {
    overflow: hidden;
    color: var(--el-text-color-primary);
    font-size: 16px;
    font-weight: 700;
    line-height: 22px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .skill-preview-meta {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 2px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    line-height: 18px;
  }

  .skill-preview-disabled-tag {
    display: inline-flex;
    align-items: center;
    height: 18px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--el-fill-color);
    color: var(--el-text-color-secondary);
    font-size: 11px;
    font-weight: 600;
  }

  .skill-preview-actions {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    padding-top: 2px;
  }

  .skill-preview-body {
    min-height: 260px;
    max-height: min(56vh, 520px);
    padding: 14px 16px;
    overflow-y: auto;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 12px;
    background: var(--el-fill-color-lighter);
  }

  .skill-preview-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 12px;
  }

  .skill-preview-footer-left,
  .skill-preview-footer-right {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
</style>

<style>
  .skill-preview-dialog.el-dialog {
    padding: 20px 20px 18px !important;
  }

  .skill-preview-dialog.el-dialog .el-dialog__header {
    display: none;
  }

  .skill-preview-dialog.el-dialog .el-dialog__body {
    padding: 0 !important;
  }

  /* Markdown 预览：覆盖 Tailwind Preflight 的 list-style:none，恢复序号与项目符号 */
  .skill-preview-body .markdown-body {
    font-size: 14px;
    line-height: 1.65;
    word-break: break-word;
    overflow-wrap: anywhere;
    color: var(--el-text-color-primary);
  }

  .skill-preview-body .markdown-body > :first-child {
    margin-top: 0;
  }

  .skill-preview-body .markdown-body h1,
  .skill-preview-body .markdown-body h2 {
    margin: 10px 0 6px;
    font-size: 15px;
    font-weight: 600;
  }

  .skill-preview-body .markdown-body h3,
  .skill-preview-body .markdown-body h4,
  .skill-preview-body .markdown-body h5,
  .skill-preview-body .markdown-body h6 {
    margin: 8px 0 4px;
    font-size: 14px;
    font-weight: 600;
  }

  .skill-preview-body .markdown-body p {
    margin: 0 0 8px;
    font-size: 13px;
    line-height: 1.65;
  }

  .skill-preview-body .markdown-body p:last-child {
    margin-bottom: 0;
  }

  .skill-preview-body .markdown-body ul,
  .skill-preview-body .markdown-body ol {
    list-style: revert;
    padding-left: 1.625rem;
    margin: 6px 0 10px;
  }

  .skill-preview-body .markdown-body ul {
    list-style-type: disc;
  }

  .skill-preview-body .markdown-body ol {
    list-style-type: decimal;
  }

  .skill-preview-body .markdown-body li {
    display: list-item;
    margin: 2px 0;
    font-size: 13px;
    line-height: 1.65;
  }

  .skill-preview-body .markdown-body li > p {
    margin: 0.25rem 0;
  }

  .skill-preview-body .markdown-body li > ul,
  .skill-preview-body .markdown-body li > ol {
    margin: 4px 0;
    padding-left: 1.25rem;
  }

  .skill-preview-body .markdown-body blockquote {
    margin: 8px 0;
    padding: 0 0 0 1rem;
    border-left: 3px solid var(--el-border-color);
    color: var(--el-text-color-secondary);
  }

  .skill-preview-body .markdown-body pre {
    margin: 8px 0;
    padding: 12px 14px;
    border-radius: 8px;
    overflow: auto;
    max-width: 100%;
    font-size: 12px;
    line-height: 1.5;
  }

  .skill-preview-body .markdown-body pre code {
    background: transparent;
    padding: 0;
  }

  .skill-preview-body .markdown-body :not(pre) > code {
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--el-fill-color);
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
    font-size: 12px;
  }

  .skill-preview-body .markdown-body table {
    width: 100%;
    max-width: 100%;
    border-collapse: collapse;
    margin: 8px 0;
    font-size: 13px;
  }

  .skill-preview-body .markdown-body th,
  .skill-preview-body .markdown-body td {
    padding: 6px 10px;
    border: 1px solid var(--el-border-color-lighter);
  }

  .skill-preview-body .markdown-body hr {
    height: 1px;
    margin: 12px 0;
    border: 0;
    background: var(--el-border-color-lighter);
  }

  .skill-preview-body .markdown-body img {
    max-width: 100%;
  }

  .skill-preview-body .markdown-body .katex-display {
    overflow-x: auto;
    overflow-y: hidden;
    max-width: 100%;
  }
</style>
