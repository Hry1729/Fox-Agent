<template>
  <div
    class="page-content skill-detail !border-0 !bg-transparent !shadow-none !p-0"
    :style="{ height: containerMinHeight }"
  >
    <!-- 标题卡片：返回 + 名称 + Tab + 操作 -->
    <div class="detail-title-card">
      <div class="title-main">
        <ElButton text class="back-btn" @click="goBack">
          <ArtSvgIcon icon="ri:arrow-left-line" />
          返回
        </ElButton>
        <div class="title-texts">
          <h1>{{ skill?.name || slug }}</h1>
          <p v-if="skill?.slug">{{ skill.slug }}</p>
        </div>
        <nav v-if="skill" class="detail-nav">
          <button
            v-for="tab in tabs"
            :key="tab.name"
            type="button"
            class="nav-item"
            :class="{ active: activeTab === tab.name }"
            @click="switchTab(tab.name)"
          >
            {{ tab.label }}
          </button>
        </nav>
      </div>
      <div v-if="skill" class="title-actions">
        <ElButton v-if="canManage" @click="exportSkill">导出</ElButton>
        <ElButton v-if="canDelete" type="danger" @click="removeSkill">删除</ElButton>
      </div>
    </div>

    <!-- 内容区 -->
    <div
      v-loading="loading"
      class="detail-body"
      :class="{ 'is-fill-tab': activeTab === 'files', 'is-config-tab': activeTab === 'config' }"
    >
      <template v-if="skill">
        <!-- 代码管理：统一大内容卡 -->
        <div v-show="activeTab === 'files'" class="detail-content-card is-fill">
          <SkillFileManager
            v-if="visitedTabs.has('files')"
            :slug="slug"
            :editable="canManage"
          />
        </div>

        <!-- 范围和依赖：双卡布局，无外层大内容卡 -->
        <div v-show="activeTab === 'config'" class="detail-content-plain">
          <div class="config-panel">
            <section class="section-card">
              <div class="section-card-header">
                <div class="section-card-header-text">
                  <h3>共享与启用状态</h3>
                  <p>控制此 Skill 是否可用，以及哪些用户可以选择和运行它。</p>
                </div>
                <ElButton
                  v-if="canManage && !skill.is_builtin"
                  type="primary"
                  :loading="savingShare"
                  @click="saveShare"
                >
                  保存设置
                </ElButton>
              </div>
              <div class="section-card-body">
                <div class="settings-row">
                  <div class="settings-row-main">
                    <div class="settings-card-title">启用状态</div>
                    <div class="settings-card-desc">
                      禁用后此 Skill 不会出现在可选资源中，也不会参与 Agent 运行时加载。
                    </div>
                  </div>
                  <ElSwitch
                    :model-value="skill.enabled !== false"
                    :disabled="!canManage"
                    @change="(value) => toggleEnabled(Boolean(value))"
                  />
                </div>
                <div class="settings-block">
                  <div class="settings-card-title">生效范围</div>
                  <div class="settings-card-desc">控制哪些用户可以选择并在运行时使用此 Skill。</div>
                  <p v-if="skill.is_builtin" class="readonly-hint">
                    内置 Skill 固定为全局生效范围，可通过启用状态控制是否参与运行时。
                  </p>
                  <ShareScopeForm
                    v-else
                    v-model="shareConfig"
                    :readonly="!canManage"
                  />
                </div>
              </div>
            </section>

            <section class="section-card">
              <div class="section-card-header">
                <div class="section-card-header-text">
                  <h3>依赖声明</h3>
                  <p>配置此 Skill 所需的工具、MCP 及其他 Skill 依赖。</p>
                </div>
              </div>
              <div class="section-card-body">
                <SkillDependencyPanel
                  v-if="visitedTabs.has('config')"
                  :slug="slug"
                  :current-slug="slug"
                  :editable="canManage"
                  embedded
                />
              </div>
            </section>
          </div>
        </div>
      </template>
      <ElEmpty v-else-if="!loading" description="Skill 不存在或无权访问" />
    </div>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { useRoute, useRouter } from 'vue-router'
  import { skillsApi, type SkillItem } from '@/api/skills'
  import ShareScopeForm, {
    type ShareConfig
  } from '@/components/extensions/common/ShareScopeForm.vue'
  import SkillDependencyPanel from '@/components/extensions/skills/SkillDependencyPanel.vue'
  import SkillFileManager from '@/components/extensions/skills/SkillFileManager.vue'
  import { useAutoLayoutHeight } from '@/hooks/core/useLayoutHeight'
  import { unwrapList } from '@/utils/apiData'
  import { parseDownloadFilename } from '@/utils/workspace'

  defineOptions({ name: 'ExtensionSkillDetail' })

  const route = useRoute()
  const router = useRouter()
  const { containerMinHeight } = useAutoLayoutHeight()
  const slug = computed(() => String(route.params.slug || ''))
  const loading = ref(false)
  const skill = ref<SkillItem | null>(null)
  const activeTab = ref('files')
  const visitedTabs = ref(new Set<string>(['files']))
  const shareConfig = ref<ShareConfig>({
    access_level: 'global',
    department_ids: [],
    user_uids: []
  })
  const savingShare = ref(false)

  const tabs = [
    { name: 'files', label: '代码管理' },
    { name: 'config', label: '范围和依赖' }
  ]

  const canManage = computed(() => skill.value?.can_manage !== false)
  const canDelete = computed(
    () => canManage.value && !skill.value?.is_builtin && skill.value?.source_type !== 'builtin'
  )

  const switchTab = (name: string) => {
    activeTab.value = name
    if (!visitedTabs.value.has(name)) {
      const next = new Set(visitedTabs.value)
      next.add(name)
      visitedTabs.value = next
    }
  }

  const goBack = () => router.push('/extensions/skills')

  const loadSkill = async () => {
    loading.value = true
    try {
      const [systemList, accessibleList] = await Promise.all([
        skillsApi.listSkills().catch(() => null),
        skillsApi.listAccessibleSkills().catch(() => null)
      ])
      const skills = [
        ...unwrapList<SkillItem>(systemList),
        ...unwrapList<SkillItem>(accessibleList)
      ]
      const unique = new Map(skills.map((item) => [item.slug, item]))
      skill.value = unique.get(slug.value) || null
      shareConfig.value = (skill.value?.share_config as ShareConfig) || {
        access_level: 'global',
        department_ids: [],
        user_uids: []
      }
    } catch {
      skill.value = null
    } finally {
      loading.value = false
    }
  }

  const toggleEnabled = async (enabled: boolean) => {
    try {
      await skillsApi.updateEnabled(slug.value, enabled)
      if (skill.value) skill.value.enabled = enabled
      ElMessage.success('状态已更新')
    } catch (error) {
      ElMessage.error((error as Error)?.message || '更新失败')
      await loadSkill()
    }
  }

  const saveShare = async () => {
    savingShare.value = true
    try {
      await skillsApi.updateShareConfig(slug.value, shareConfig.value as Record<string, unknown>)
      ElMessage.success('共享范围已保存')
    } catch (error) {
      ElMessage.error((error as Error)?.message || '保存失败')
    } finally {
      savingShare.value = false
    }
  }

  const exportSkill = async () => {
    try {
      const response = await skillsApi.exportSkill(slug.value)
      const blob = await response.blob()
      const filename =
        parseDownloadFilename(
          response.headers.get('Content-Disposition') ||
            response.headers.get('content-disposition') ||
            ''
        ) || `${slug.value}.zip`
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = filename
      link.click()
      URL.revokeObjectURL(url)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '导出失败')
    }
  }

  const removeSkill = async () => {
    try {
      await ElMessageBox.confirm('确认删除该 Skill？', '删除', { type: 'warning' })
      await skillsApi.deleteSkill(slug.value)
      ElMessage.success('已删除')
      goBack()
    } catch {
      // 取消
    }
  }

  watch(slug, loadSkill, { immediate: true })
</script>

<style scoped>
  .skill-detail {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-height: 0;
    padding: 0;
    overflow: hidden;
    box-sizing: border-box;
    background: transparent !important;
    border: none !important;
    box-shadow: none !important;
  }

  .detail-title-card {
    flex-shrink: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
  }

  .title-main {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    flex: 1;
  }

  .back-btn {
    flex-shrink: 0;
  }

  .title-texts {
    flex-shrink: 0;
    min-width: 0;
  }

  .title-texts h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    line-height: 1.3;
    white-space: nowrap;
  }

  .title-texts p {
    margin: 2px 0 0;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    white-space: nowrap;
  }

  .detail-nav {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-left: 8px;
    min-width: 0;
  }

  .nav-item {
    height: var(--el-component-size, 32px);
    padding: 0 14px;
    border: none;
    border-radius: var(--el-border-radius-base, 4px);
    background: transparent;
    color: var(--el-text-color-regular);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
    white-space: nowrap;
    box-sizing: border-box;
  }

  .nav-item:hover {
    background: var(--el-fill-color-light);
    color: var(--el-color-primary);
  }

  .nav-item.active {
    background: var(--el-color-primary-light-9);
    color: var(--el-color-primary);
    font-weight: 600;
  }

  .title-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .detail-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .detail-body.is-fill-tab,
  .detail-body.is-config-tab {
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .detail-content-plain,
  .detail-content-card {
    min-height: 0;
    box-sizing: border-box;
  }

  .detail-body.is-fill-tab .detail-content-card,
  .detail-body.is-config-tab .detail-content-plain {
    flex: 1;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .detail-content-card {
    padding: 16px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
  }

  .detail-content-card.is-fill {
    padding: 0;
    overflow: hidden;
  }

  .detail-body.is-fill-tab .detail-content-card > * {
    flex: 1;
    min-height: 0;
  }

  .detail-content-plain {
    overflow: auto;
  }

  .config-panel {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .section-card {
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
    overflow: hidden;
  }

  .section-card-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--el-border-color-lighter);
    background: var(--el-bg-color);
  }

  .section-card-header-text h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }

  .section-card-header-text p {
    margin: 4px 0 0;
    color: var(--el-text-color-secondary);
    font-size: 13px;
    line-height: 1.5;
  }

  .section-card-body {
    padding: 14px 16px 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .settings-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 14px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-bg-color);
  }

  .settings-block {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-bg-color);
  }

  .settings-card-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .settings-card-desc {
    margin-top: 4px;
    color: var(--el-text-color-secondary);
    font-size: 13px;
    line-height: 1.5;
  }

  .readonly-hint {
    margin: 0;
    color: var(--el-text-color-secondary);
    font-size: 13px;
    line-height: 1.5;
  }
</style>
