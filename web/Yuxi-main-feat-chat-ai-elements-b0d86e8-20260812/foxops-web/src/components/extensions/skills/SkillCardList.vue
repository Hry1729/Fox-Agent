<template>
  <div class="skill-card-list">
    <ExtensionToolbar
      v-model:search="searchQuery"
      class="skills-toolbar"
      search-placeholder="搜索 Skills..."
      :loading="loading"
      @refresh="fetchSkills"
    >
      <template #actions>
        <ElUpload :show-file-list="false" :before-upload="handleUpload" accept=".zip,.md">
          <ElButton>本地上传</ElButton>
        </ElUpload>
        <ElButton @click="remoteVisible = true">远程安装</ElButton>
        <ElButton
          v-if="selectionMode"
          type="danger"
          :disabled="!selectedSlugs.length"
          @click="batchDelete"
        >
          删除选中
        </ElButton>
        <ElButton @click="selectionMode = !selectionMode">
          {{ selectionMode ? '取消多选' : '批量管理' }}
        </ElButton>
      </template>
    </ExtensionToolbar>

    <ExtensionEmptyState
      v-if="!loading && !filteredSkills.length"
      :description="searchQuery ? '无匹配 Skill' : '暂无 Skill'"
    />

    <template v-else>
      <ExtensionCardGrid>
        <ExtensionInfoCard
          v-for="skill in filteredSkills"
          :key="skill.slug"
          :title="formatExtensionCardTitle(skill.name || skill.slug)"
          :subtitle="skill.slug"
          :description="skill.description || '暂无描述'"
          icon="ri:magic-line"
          accent="purple"
          :tags="skillTags(skill)"
          @click="handleCardClick(skill)"
        >
          <template v-if="selectionMode && canDelete(skill)" #action>
            <ElCheckbox
              :model-value="selectedSlugs.includes(skill.slug)"
              @change="(checked) => toggleSelect(skill.slug, Boolean(checked))"
            />
          </template>
          <template #footer>
            <button type="button" class="skill-manage-btn" @click="goDetail(skill.slug)">
              管理
            </button>
          </template>
        </ExtensionInfoCard>
      </ExtensionCardGrid>
    </template>

    <SkillPreviewDialog v-model="previewVisible" :skill="previewSkill" @changed="fetchSkills" />
    <SkillRemoteInstallDialog v-model="remoteVisible" @installed="fetchSkills" />
    <SkillInstallDraftDialog
      v-model="draftVisible"
      :drafts="pendingDrafts"
      @confirmed="fetchSkills"
    />
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { useRouter } from 'vue-router'
  import { skillsApi, type SkillItem } from '@/api/skills'
  import ExtensionCardGrid from '@/components/extensions/common/ExtensionCardGrid.vue'
  import ExtensionEmptyState from '@/components/extensions/common/ExtensionEmptyState.vue'
  import ExtensionInfoCard from '@/components/extensions/common/ExtensionInfoCard.vue'
  import ExtensionToolbar from '@/components/extensions/common/ExtensionToolbar.vue'
  import { unwrapApiData, unwrapList } from '@/utils/apiData'
  import { formatExtensionCardTitle } from '@/utils/extensionDisplayName'
  import SkillInstallDraftDialog from './SkillInstallDraftDialog.vue'
  import SkillPreviewDialog from './SkillPreviewDialog.vue'
  import SkillRemoteInstallDialog from './SkillRemoteInstallDialog.vue'

  const router = useRouter()
  const loading = ref(false)
  const skills = ref<SkillItem[]>([])
  const searchQuery = ref('')
  const selectionMode = ref(false)
  const selectedSlugs = ref<string[]>([])
  const previewVisible = ref(false)
  const previewSkill = ref<SkillItem | null>(null)
  const remoteVisible = ref(false)
  const draftVisible = ref(false)
  const pendingDrafts = ref<any[]>([])

  const isBuiltin = (skill: SkillItem) =>
    !!(skill.is_builtin || skill.source_type === 'builtin' || skill.source === 'builtin')

  const canDelete = (skill: SkillItem) => !isBuiltin(skill) && skill.can_manage !== false

  const skillTags = (skill: SkillItem) => {
    const tags: Array<
      | string
      | {
          name: string
          type?: 'success' | 'info' | 'warning'
          color?: 'blue' | 'purple' | 'green' | 'orange'
        }
    > = []
    if (!isBuiltin(skill)) {
      tags.push({ name: skill.source_type || skill.source || '自定义', color: 'purple' })
    }
    if (skill.enabled === false) tags.push({ name: '已禁用', color: 'orange' })
    else tags.push({ name: '已启用', color: 'green' })
    return tags
  }

  const filteredSkills = computed(() => {
    if (!searchQuery.value) return skills.value
    const q = searchQuery.value.toLowerCase()
    return skills.value.filter(
      (item) =>
        item.name?.toLowerCase().includes(q) ||
        item.slug?.toLowerCase().includes(q) ||
        item.description?.toLowerCase().includes(q)
    )
  })

  const fetchSkills = async () => {
    loading.value = true
    try {
      // 与 Yuxi 扩展页一致：管理员列表走 /api/system/skills
      const result = await skillsApi.listSkills()
      skills.value = unwrapList<SkillItem>(result)
    } catch {
      ElMessage.error('加载 Skills 失败')
      skills.value = []
    } finally {
      loading.value = false
    }
  }

  const goDetail = (slug: string) => router.push(`/extensions/skill/${slug}`)
  const openPreview = (skill: SkillItem) => {
    previewSkill.value = skill
    previewVisible.value = true
  }

  const handleCardClick = (skill: SkillItem) => {
    if (selectionMode.value && canDelete(skill)) {
      toggleSelect(skill.slug, !selectedSlugs.value.includes(skill.slug))
      return
    }
    openPreview(skill)
  }

  const toggleSelect = (slug: string, checked: boolean) => {
    if (checked) selectedSlugs.value = [...new Set([...selectedSlugs.value, slug])]
    else selectedSlugs.value = selectedSlugs.value.filter((item) => item !== slug)
  }

  const normalizeDrafts = (payload: any) => {
    const data = unwrapApiData(payload, payload)
    if (Array.isArray(data)) return data.filter((item) => item?.draft_id || item?.id)
    if (data?.draft_id || data?.id) return [data]
    if (Array.isArray(data?.drafts)) return data.drafts
    if (Array.isArray(data?.items) && data.draft_id) return [data]
    return []
  }

  const handleUpload = async (file: File) => {
    const lower = file.name.toLowerCase()
    if (!lower.endsWith('.zip') && lower !== 'skill.md') {
      ElMessage.error('仅支持上传 .zip 或 SKILL.md')
      return false
    }
    try {
      const result = await skillsApi.prepareUpload(file)
      const drafts = normalizeDrafts(result)
      if (!drafts.length) {
        ElMessage.warning('未解析到可安装 Skill')
        return false
      }
      pendingDrafts.value = drafts
      draftVisible.value = true
    } catch (error) {
      ElMessage.error((error as Error)?.message || '上传解析失败')
    }
    return false
  }

  const batchDelete = async () => {
    try {
      await ElMessageBox.confirm(
        `确认删除选中的 ${selectedSlugs.value.length} 个 Skill？`,
        '批量删除',
        { type: 'warning' }
      )
      await skillsApi.deleteBatch(selectedSlugs.value)
      ElMessage.success('删除成功')
      selectedSlugs.value = []
      selectionMode.value = false
      await fetchSkills()
    } catch {
      // 取消
    }
  }

  onMounted(fetchSkills)
  defineExpose({ fetchSkills, loading })
</script>

<style scoped>
  .skills-toolbar :deep(.toolbar-actions) {
    gap: 4px;
  }

  .skills-toolbar :deep(.toolbar-actions .el-button),
  .skills-toolbar :deep(.toolbar-actions .el-upload) {
    margin: 0;
  }

  .skills-toolbar :deep(.toolbar-actions .el-button + .el-button) {
    margin-left: 0;
  }

  .skill-manage-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 22px;
    padding: 0 8px;
    border: none;
    border-radius: 4px;
    background: var(--el-color-primary-light-9);
    color: var(--el-color-primary);
    font-size: 12px;
    line-height: 1;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .skill-manage-btn:hover {
    background: var(--el-color-primary-light-8);
  }
</style>
