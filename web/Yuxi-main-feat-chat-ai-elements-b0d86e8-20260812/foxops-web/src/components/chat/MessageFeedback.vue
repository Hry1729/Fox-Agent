<template>
  <div v-if="message?.id" class="flex items-center gap-2">
    <ArtSvgIcon
      icon="ri:thumb-up-line"
      class="c-p text-g-500 hover:text-g-700 text-base transition-colors leading-none"
      :class="{ '!text-success': feedbackState.rating === 'like' }"
      @click="likeThisResponse"
    />
    <ArtSvgIcon
      icon="ri:thumb-down-line"
      class="c-p text-g-500 hover:text-g-700 text-base transition-colors leading-none"
      :class="{ '!text-danger': feedbackState.rating === 'dislike' }"
      @click="dislikeThisResponse"
    />
  </div>

  <ElDialog
    v-model="dislikeVisible"
    title="请告诉我们不满意的原因"
    width="420px"
    :close-on-click-modal="false"
  >
    <ElInput
      v-model="dislikeReason"
      type="textarea"
      :rows="4"
      maxlength="500"
      show-word-limit
      placeholder="您的反馈将帮助我们改进服务（可选）"
    />
    <template #footer>
      <ElButton @click="dislikeVisible = false">取消</ElButton>
      <ElButton type="primary" :loading="submitting" @click="submitDislike">提交</ElButton>
    </template>
  </ElDialog>
</template>

<script setup>
  import { ElMessage } from 'element-plus'
  import { agentApi } from '@/api/agent'

  const props = defineProps({
    message: { type: Object, default: () => ({}) }
  })

  const feedbackState = reactive({ hasSubmitted: false, rating: null, reason: null })
  const dislikeVisible = ref(false)
  const dislikeReason = ref('')
  const submitting = ref(false)

  const initFeedbackState = () => {
    if (props.message?.feedback) {
      feedbackState.hasSubmitted = true
      feedbackState.rating = props.message.feedback.rating
      feedbackState.reason = props.message.feedback.reason
    } else {
      feedbackState.hasSubmitted = false
      feedbackState.rating = null
      feedbackState.reason = null
    }
  }
  watch(() => props.message, initFeedbackState, { immediate: true })

  const likeThisResponse = async () => {
    if (feedbackState.hasSubmitted) return ElMessage.info('您已经提交过反馈了')
    if (!props.message?.id) return ElMessage.error('无法提交反馈：消息ID不存在')
    try {
      submitting.value = true
      await agentApi.submitMessageFeedback(props.message.id, 'like', null)
      feedbackState.hasSubmitted = true
      feedbackState.rating = 'like'
      ElMessage.success('感谢您的反馈！')
    } catch (e) {
      if (e?.message?.includes('already submitted')) {
        feedbackState.hasSubmitted = true
      } else {
        ElMessage.error('提交反馈失败，请稍后重试')
      }
    } finally {
      submitting.value = false
    }
  }

  const dislikeThisResponse = () => {
    if (feedbackState.hasSubmitted) return ElMessage.info('您已经提交过反馈了')
    dislikeVisible.value = true
  }

  const submitDislike = async () => {
    try {
      submitting.value = true
      await agentApi.submitMessageFeedback(
        props.message.id,
        'dislike',
        dislikeReason.value || null
      )
      feedbackState.hasSubmitted = true
      feedbackState.rating = 'dislike'
      feedbackState.reason = dislikeReason.value
      dislikeVisible.value = false
      dislikeReason.value = ''
      ElMessage.success('感谢您的反馈！')
    } catch (e) {
      if (e?.message?.includes('already submitted')) {
        feedbackState.hasSubmitted = true
        dislikeVisible.value = false
      } else {
        ElMessage.error('提交反馈失败，请稍后重试')
      }
    } finally {
      submitting.value = false
    }
  }
</script>
