<template>
  <div class="skill-dependency-panel" :class="{ embedded }" v-loading="loading">
    <div class="dep-grid">
      <section v-for="group in groups" :key="group.key" class="dep-card">
        <h4>{{ group.label }}</h4>
        <p class="dep-card-count">已选 {{ selected[group.key].length }}</p>
        <ElSelect
          v-model="selected[group.key]"
          multiple
          filterable
          :disabled="!editable"
          style="width: 100%"
          :placeholder="`选择${group.label}`"
        >
          <ElOption
            v-for="option in group.options"
            :key="option.value"
            :label="option.label"
            :value="option.value"
            :disabled="option.value === currentSlug"
          />
        </ElSelect>
      </section>
    </div>
    <div v-if="editable" class="dep-actions">
      <ElButton type="primary" :loading="saving" @click="save">保存依赖</ElButton>
    </div>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { skillsApi } from '@/api/skills'
  import { unwrapApiData } from '@/utils/apiData'

  const props = withDefaults(
    defineProps<{
      slug: string
      editable?: boolean
      currentSlug?: string
      embedded?: boolean
    }>(),
    { embedded: false }
  )
  const loading = ref(false)
  const saving = ref(false)
  const selected = reactive<{ tools: string[]; mcp: string[]; skills: string[] }>({
    tools: [],
    mcp: [],
    skills: []
  })
  const groups = ref([
    {
      key: 'tools' as const,
      label: '内置工具',
      options: [] as Array<{ label: string; value: string }>
    },
    { key: 'mcp' as const, label: 'MCP', options: [] as Array<{ label: string; value: string }> },
    {
      key: 'skills' as const,
      label: 'Skills',
      options: [] as Array<{ label: string; value: string }>
    }
  ])

  const load = async () => {
    loading.value = true
    try {
      const result: any = await skillsApi.getDependencyOptions(props.slug)
      const data = unwrapApiData(result, result) || {}
      groups.value[0].options = (data?.tools || []).map((item: any) => ({
        label: item.name || item.slug || item.id,
        value: item.slug || item.id || item.name
      }))
      groups.value[1].options = (data?.mcp || data?.mcp_servers || []).map((item: any) => ({
        label: item.name || item.slug,
        value: item.slug || item.name
      }))
      groups.value[2].options = (data?.skills || []).map((item: any) => ({
        label: item.name || item.slug,
        value: item.slug || item.name
      }))
      selected.tools = data?.selected?.tools || data?.current?.tools || []
      selected.mcp = data?.selected?.mcp || data?.current?.mcp || []
      selected.skills = data?.selected?.skills || data?.current?.skills || []
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载依赖失败')
    } finally {
      loading.value = false
    }
  }

  const save = async () => {
    saving.value = true
    try {
      await skillsApi.updateDependencies(props.slug, {
        tools: selected.tools,
        mcp: selected.mcp,
        skills: selected.skills.filter((item) => item !== props.currentSlug)
      })
      ElMessage.success('依赖已保存')
      await load()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '保存失败')
    } finally {
      saving.value = false
    }
  }

  watch(() => props.slug, load, { immediate: true })
</script>

<style scoped>
  .skill-dependency-panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .dep-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 12px;
  }

  .dep-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
    padding: 12px 14px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-fill-color-lighter);
  }

  .dep-card h4 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .dep-card-count {
    margin: 0;
    font-size: 12px;
    color: var(--el-text-color-secondary);
    line-height: 1.4;
  }

  .dep-actions {
    display: flex;
    justify-content: flex-end;
  }

  @media (max-width: 960px) {
    .dep-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
