<template>
  <ElDialog
    :model-value="modelValue"
    title="远程安装 Skill"
    width="720px"
    destroy-on-close
    class="extension-dialog-compact"
    @close="$emit('update:modelValue', false)"
  >
    <ElTabs v-model="mode">
      <ElTabPane label="仓库拉取" name="repo">
        <ElInput v-model="source" placeholder="owner/repo、GitHub URL 或 ModelScope 地址" />
        <div class="actions">
          <ElButton type="primary" :loading="loading" @click="listRemote">拉取列表</ElButton>
        </div>
      </ElTabPane>
      <ElTabPane label="市场搜索" name="search">
        <ElInput v-model="query" placeholder="关键词" @keyup.enter="searchRemote" />
        <div class="actions">
          <ElButton type="primary" :loading="loading" @click="searchRemote">搜索</ElButton>
        </div>
      </ElTabPane>
    </ElTabs>

    <ElTable
      v-if="items.length"
      :data="items"
      size="small"
      @selection-change="onSelect"
      height="240"
    >
      <ElTableColumn type="selection" width="44" />
      <ElTableColumn prop="name" label="名称" />
      <ElTableColumn prop="description" label="描述" show-overflow-tooltip />
    </ElTable>
    <ElEmpty v-else description="暂无结果" />

    <template #footer>
      <ElButton @click="$emit('update:modelValue', false)">取消</ElButton>
      <ElButton type="primary" :disabled="!selected.length" :loading="preparing" @click="prepare">
        准备安装
      </ElButton>
    </template>

    <SkillInstallDraftDialog v-model="draftVisible" :drafts="drafts" @confirmed="onInstalled" />
  </ElDialog>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { skillsApi } from '@/api/skills'
  import SkillInstallDraftDialog from './SkillInstallDraftDialog.vue'
  import { unwrapApiData, unwrapList } from '@/utils/apiData'

  defineProps<{ modelValue: boolean }>()
  const emit = defineEmits<{ 'update:modelValue': [value: boolean]; installed: [] }>()

  const mode = ref('repo')
  const source = ref('')
  const query = ref('')
  const loading = ref(false)
  const preparing = ref(false)
  const items = ref<any[]>([])
  const selected = ref<any[]>([])
  const draftVisible = ref(false)
  const drafts = ref<any[]>([])

  const listRemote = async () => {
    if (!source.value.trim()) {
      ElMessage.warning('请输入远程来源')
      return
    }
    loading.value = true
    try {
      const result: any = await skillsApi.listRemoteSkills(source.value.trim())
      items.value = unwrapList(result)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '拉取失败')
    } finally {
      loading.value = false
    }
  }

  const searchRemote = async () => {
    if (!query.value.trim()) {
      ElMessage.warning('请输入关键词')
      return
    }
    loading.value = true
    try {
      const result: any = await skillsApi.searchRemoteSkills(query.value.trim())
      items.value = unwrapList(result)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '搜索失败')
    } finally {
      loading.value = false
    }
  }

  const onSelect = (rows: any[]) => {
    selected.value = rows
  }

  const prepare = async () => {
    preparing.value = true
    try {
      const payload =
        mode.value === 'repo'
          ? { source: source.value.trim(), skills: selected.value }
          : { query: query.value.trim(), skills: selected.value }
      const result: any = await skillsApi.prepareRemoteSkills(payload)
      const data = unwrapApiData(result, result)
      drafts.value = Array.isArray(data) ? data : data?.drafts || (data?.draft_id ? [data] : [])
      if (!drafts.value.length) {
        ElMessage.warning('未生成安装草稿')
        return
      }
      draftVisible.value = true
    } catch (error) {
      ElMessage.error((error as Error)?.message || '准备安装失败')
    } finally {
      preparing.value = false
    }
  }

  const onInstalled = () => {
    emit('installed')
    emit('update:modelValue', false)
  }
</script>

<style scoped>
  .actions {
    margin: 8px 0;
  }
</style>
