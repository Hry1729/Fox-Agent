<template>
  <div class="page-content !mb-5">
    <SkillCardList />
  </div>
</template>

<script setup lang="ts">
  import SkillCardList from '@/components/extensions/skills/SkillCardList.vue'

  defineOptions({ name: 'ExtensionSkills' })
</script>
