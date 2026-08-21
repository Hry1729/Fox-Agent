<template>
  <ElementsToolCall
    :tool-call="toolCall"
    :default-expanded="defaultExpanded"
    :appearance="appearance"
  />
</template>

<script setup>
import ElementsToolCall from './ElementsToolCall.vue'

defineProps({
  toolCall: {
    type: Object,
    required: true
  },
  defaultExpanded: {
    type: Boolean,
    default: false
  },
  appearance: {
    type: String,
    default: 'card'
  }
})
</script>
