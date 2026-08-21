<template>
  <Alert v-if="questions.length" class="approval-card chat-ai-elements flex flex-col gap-3">
    <div class="approval-header">
      <ArtSvgIcon icon="ri:question-answer-line" class="header-icon" />
      <span class="header-title">{{ isApproval ? '需要确认' : '需要回答' }}</span>
    </div>

    <div v-for="(q, qIdx) in questions" :key="q.questionId" class="question-item">
      <div class="question-text">{{ q.question }}</div>

      <template v-if="q.multiSelect">
        <ElCheckboxGroup v-model="answers[q.questionId]" class="option-group">
          <ElCheckbox
            v-for="opt in q.options"
            :key="opt.value"
            :value="opt.value"
            :label="opt.label"
          />
        </ElCheckboxGroup>
      </template>

      <template v-else>
        <ElRadioGroup v-model="answers[q.questionId]" class="option-group">
          <ElRadio
            v-for="opt in q.options"
            :key="opt.value"
            :value="opt.value"
            :label="opt.label"
          />
        </ElRadioGroup>
      </template>

      <ElInput
        v-if="showOtherInput(q, answers[q.questionId])"
        v-model="customInputs[q.questionId]"
        placeholder="请输入..."
        class="other-input"
        clearable
      />
    </div>

    <div class="approval-actions">
      <Button type="button" variant="outline" @click="handleCancel">拒绝</Button>
      <Button type="button" @click="handleSubmit">确认</Button>
    </div>
  </Alert>
</template>

<script setup>
import { computed, reactive, watch } from 'vue'
import { Alert } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { isOtherOption, DEFAULT_OTHER_OPTION_VALUE } from '@/utils/questionUtils'

const props = defineProps({
  questions: { type: Array, default: () => [] },
  status: { type: String, default: '' }
})

const emit = defineEmits(['submit', 'cancel'])

const isApproval = computed(() => props.status === 'human_approval_required')

const answers = reactive({})
const customInputs = reactive({})

watch(
  () => props.questions,
  (newQuestions) => {
    newQuestions.forEach((q) => {
      if (!(q.questionId in answers)) {
        answers[q.questionId] = q.multiSelect ? [] : ''
      }
      if (!(q.questionId in customInputs)) {
        customInputs[q.questionId] = ''
      }
    })
  },
  { immediate: true }
)

const showOtherInput = (question, answer) => {
  if (!question.allowOther) return false
  if (Array.isArray(answer)) {
    return answer.some((v) => isOtherOption({ value: v }))
  }
  return isOtherOption({ value: answer })
}

const buildAnswer = () => {
  const result = {}
  for (const q of props.questions) {
    let answer = answers[q.questionId]
    const custom = customInputs[q.questionId]

    if (Array.isArray(answer)) {
      const hasOther = answer.some((v) => v === DEFAULT_OTHER_OPTION_VALUE)
      if (hasOther) {
        if (custom?.trim()) {
          answer = [...answer.filter((v) => v !== DEFAULT_OTHER_OPTION_VALUE), custom.trim()]
        } else {
          answer = answer.filter((v) => v !== DEFAULT_OTHER_OPTION_VALUE)
        }
      }
    } else if (answer === DEFAULT_OTHER_OPTION_VALUE) {
      answer = custom?.trim() || ''
    }

    result[q.questionId] = answer
  }
  return result
}

const handleSubmit = () => {
  emit('submit', buildAnswer())
}

const handleCancel = () => {
  emit('cancel')
}
</script>

<style scoped>
.approval-card {
  width: 100%;
  max-width: 520px;
  margin: 12px 0;
}
.approval-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 600;
}
.header-icon {
  font-size: 18px;
}
.question-item {
  margin-bottom: 4px;
}
.question-text {
  font-size: 14px;
  font-weight: 500;
  margin-bottom: 8px;
}
.option-group {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-left: 4px;
}
.other-input {
  margin-top: 8px;
  margin-left: 4px;
  width: calc(100% - 4px);
}
.approval-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 4px;
}
</style>
