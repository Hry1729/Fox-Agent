<!-- 登录页面：全屏背景视频 + 居中登录卡 -->
<template>
  <div class="login-page">
    <!-- 底层视频 / 兜底背景 -->
    <div class="login-bg" :class="{ 'has-video': videoReady }">
      <video
        v-show="videoReady"
        ref="videoRef"
        class="login-video"
        :src="videoSrc"
        autoplay
        muted
        loop
        playsinline
        preload="auto"
        @canplay="onVideoReady"
        @error="onVideoError"
      />
    </div>

    <!-- 居中登录卡 -->
    <main class="login-main">
      <div class="login-card">
        <div class="card-header">
          <div class="title-row">
            <ArtLogo class="brand-logo" size="36" />
            <h3 class="title">{{ $t('login.title') }}</h3>
          </div>
          <p class="sub-title">{{ $t('login.subTitle') }}</p>
        </div>

        <ElForm
          ref="formRef"
          :model="formData"
          :rules="rules"
          @keyup.enter="handleSubmit"
          class="login-form"
        >
          <ElFormItem prop="username" class="form-row">
            <ElInput
              class="custom-height"
              placeholder="UID 或手机号"
              v-model.trim="formData.username"
            />
          </ElFormItem>
          <ElFormItem prop="password" class="form-row">
            <ElInput
              class="custom-height"
              :placeholder="$t('login.placeholder.password')"
              v-model.trim="formData.password"
              type="password"
              autocomplete="off"
              show-password
            />
          </ElFormItem>

          <div class="form-row verify-row">
            <div
              class="relative z-[2] overflow-hidden select-none rounded-lg border border-transparent transition-all duration-300"
              :class="{ '!border-[#FF4E4F]': !isPassing && isClickPass }"
            >
              <ArtDragVerify
                ref="dragVerify"
                v-model:value="isPassing"
                :text="$t('login.sliderText')"
                textColor="var(--art-gray-700)"
                :successText="$t('login.sliderSuccessText')"
                progressBarBg="var(--main-color)"
                :background="isDark ? '#26272F' : '#F1F1F4'"
                handlerBg="var(--default-box-color)"
                :height="44"
              />
            </div>
            <p
              class="verify-tip text-xs text-[#f56c6c] transition-all duration-300"
              :class="{ 'is-show': !isPassing && isClickPass }"
            >
              {{ $t('login.placeholder.slider') }}
            </p>
          </div>

          <div class="flex-cb form-actions text-sm">
            <ElCheckbox v-model="formData.rememberPassword">
              {{ $t('login.rememberPwd') }}
            </ElCheckbox>
            <RouterLink class="text-theme" :to="{ name: 'ForgetPassword' }">
              {{ $t('login.forgetPwd') }}
            </RouterLink>
          </div>

          <div class="submit-row">
            <ElButton
              class="w-full custom-height"
              type="primary"
              :loading="loading"
              v-ripple
              @click="handleSubmit"
            >
              {{ $t('login.btnText') }}
            </ElButton>
          </div>
        </ElForm>
      </div>
    </main>
  </div>
</template>

<script setup lang="ts">
  import AppConfig from '@/config'
  import { useUserStore } from '@/store/modules/user'
  import { useI18n } from 'vue-i18n'
  import { fetchLogin } from '@/api/auth'
  import { ElNotification, type FormInstance, type FormRules } from 'element-plus'
  import { useSettingStore } from '@/store/modules/setting'

  defineOptions({ name: 'Login' })

  const settingStore = useSettingStore()
  const { isDark } = storeToRefs(settingStore)
  const { t } = useI18n()

  const dragVerify = ref()
  const videoRef = ref<HTMLVideoElement | null>(null)
  const videoReady = ref(false)
  /** public 静态资源，用脚本字符串避免 Vite 静态分析 source 路径 */
  const videoSrc = '/videos/login-bg.mp4'

  const userStore = useUserStore()
  const router = useRouter()
  const route = useRoute()
  const isPassing = ref(false)
  const isClickPass = ref(false)

  const systemName = AppConfig.systemInfo.name
  const formRef = ref<FormInstance>()

  const formData = reactive({
    username: '',
    password: '',
    rememberPassword: true
  })

  const rules = computed<FormRules>(() => ({
    username: [{ required: true, message: '请输入 UID 或手机号', trigger: 'blur' }],
    password: [{ required: true, message: t('login.placeholder.password'), trigger: 'blur' }]
  }))

  const loading = ref(false)

  const onVideoReady = () => {
    videoReady.value = true
    videoRef.value?.play().catch(() => {
      // 自动播放被拦截时仍展示首帧/兜底背景
    })
  }

  const onVideoError = () => {
    videoReady.value = false
  }

  /** 登录（对接 Yuxi /api/auth/token，支持 uid 或手机号） */
  const handleSubmit = async () => {
    if (!formRef.value) return

    try {
      const valid = await formRef.value.validate()
      if (!valid) return

      if (!isPassing.value) {
        isClickPass.value = true
        return
      }

      loading.value = true

      const { username, password } = formData
      const data = await fetchLogin(username, password)

      if (!data.access_token) {
        throw new Error('登录失败：未收到 token')
      }

      userStore.setToken(data.access_token)
      userStore.setLoginStatus(true)
      showLoginSuccessNotice()

      const redirect = route.query.redirect as string
      router.push(redirect || '/')
    } catch (error) {
      console.error('[Login] 登录失败:', error)
    } finally {
      loading.value = false
      resetDragVerify()
    }
  }

  const resetDragVerify = () => {
    dragVerify.value?.reset?.()
  }

  const showLoginSuccessNotice = () => {
    setTimeout(() => {
      ElNotification({
        title: t('login.success.title'),
        type: 'success',
        duration: 2500,
        zIndex: 10000,
        message: `${t('login.success.message')}, ${systemName}!`
      })
    }, 1000)
  }
</script>

<style scoped>
  @import './style.css';
</style>
