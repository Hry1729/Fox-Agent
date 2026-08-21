<template>
  <ElDialog
    :model-value="modelValue"
    title="确认安装草稿"
    width="640px"
    destroy-on-close
    class="extension-dialog-compact"
    @close="handleClose"
  >
    <ElTable :data="drafts" size="small" max-height="280">
      <ElTableColumn prop="name" label="名称" />
      <ElTableColumn prop="description" label="描述" show-overflow-tooltip />
      <ElTableColumn prop="warning" label="警告" show-overflow-tooltip />
    </ElTable>

    <h4 class="share-title">生效范围</h4>
    <ShareScopeForm v-model="shareConfig" />

    <template #footer>
      <ElButton @click="handleClose">取消</ElButton>
      <ElButton type="primary" :loading="confirming" @click="confirmAll">确认安装</ElButton>
    </template>
  </ElDialog>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { skillsApi } from '@/api/skills'
  import ShareScopeForm, {
    type ShareConfig
  } from '@/components/extensions/common/ShareScopeForm.vue'

  const props = defineProps<{ modelValue: boolean; drafts?: any[] }>()
  const emit = defineEmits<{ 'update:modelValue': [value: boolean]; confirmed: [] }>()

  const confirming = ref(false)
  const shareConfig = ref<ShareConfig>({
    access_level: 'global',
    department_ids: [],
    user_uids: []
  })

  const discardAll = async () => {
    for (const draft of props.drafts || []) {
      const draftId = draft.draft_id || draft.id
      if (!draftId) continue
      try {
        await skillsApi.discardInstallDraft(draftId)
      } catch {
        // 忽略丢弃失败
      }
    }
  }

  const handleClose = async () => {
    await discardAll()
    emit('update:modelValue', false)
  }

  const confirmAll = async () => {
    confirming.value = true
    let success = 0
    let failed = 0
    try {
      for (const draft of props.drafts || []) {
        const draftId = draft.draft_id || draft.id
        if (!draftId) {
          failed += 1
          continue
        }
        try {
          await skillsApi.confirmInstallDraft(draftId, shareConfig.value as Record<string, unknown>)
          success += 1
        } catch {
          failed += 1
        }
      }
      ElMessage.success(`安装完成：成功 ${success}，失败 ${failed}`)
      emit('confirmed')
      emit('update:modelValue', false)
    } finally {
      confirming.value = false
    }
  }
</script>

<style scoped>
  .share-title {
    margin: 10px 0 6px;
    font-size: 13px;
  }
</style>
