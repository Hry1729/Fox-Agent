<template>
  <div class="chat-ai-elements elements-prompt-shell">
    <!--
      用 PromptInput 做视觉外壳（InputGroup 边框/阴影）；
      发送仍由内部 MessageInputComponent 驱动，故忽略 PromptInput 自身 submit。
    -->
    <PromptInput class="elements-prompt-shell__form" @submit="onPromptSubmit">
      <slot />
    </PromptInput>
  </div>
</template>

<script setup>
  import { PromptInput } from '@/components/ai-elements/prompt-input'

  defineEmits(['submit'])

  const onPromptSubmit = () => {
    // 编辑器为 contenteditable，不走 PromptInputTextarea 状态；忽略空 submit
  }
</script>

<style scoped>
  .elements-prompt-shell {
    width: 100%;
  }

  .elements-prompt-shell__form {
    width: 100%;
  }

  .elements-prompt-shell :deep([data-slot='input-group']) {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    height: auto;
    min-height: 0;
    background: var(--el-bg-color, #fff);
    border-radius: 0.875rem;
  }

  /* 去掉内层 MessageInput 重复边框，统一由 InputGroup 承担 chrome */
  .elements-prompt-shell :deep(.input-box) {
    padding: 0.75rem 0.875rem 0.625rem;
    background: transparent;
    border: none;
    border-radius: 0;
    box-shadow: none;
  }
</style>
