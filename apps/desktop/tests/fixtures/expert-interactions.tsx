import { useState } from 'react'
import { dispatchNotificationPreview, FOX_IN_APP_NOTIFICATION_EVENT } from '../../src/features/notifications'
import { createRoot } from 'react-dom/client'
import { ExpertLanyard } from '../../src/features/chat/components/ExpertLanyard'
import { FOX_ASSISTANT_AVATAR } from '../../src/features/chat/components/FoxAssistantAvatar'
import { ToastStack, ToastStackProvider, useToastStack } from '../../src/components/arc/toast-stack/toast-stack'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'

document.documentElement.dataset.colorTheme = 'graphite'
const previews: unknown[] = []
window.addEventListener(FOX_IN_APP_NOTIFICATION_EVENT, (event) => previews.push((event as CustomEvent).detail))
;(window as any).__notificationPreviews = previews
function Fixture() {
  const { toast } = useToastStack()
  const [expert, setExpert] = useState(0)
  const [sent, setSent] = useState(0)
  return <div style={{ height: '100vh', background: 'var(--background)', color: 'var(--foreground)' }}>
    <header style={{ height: 44, borderBottom: '1px solid var(--border)', padding: '12px 24px' }}>Fox · 专家卡片与通知交互验证（独立测试页面）</header>
    <div className="fox-chat-stage" style={{ height: 'calc(100vh - 44px)' }}>
      <ExpertLanyard expertId={`fixture-${expert}`} name={expert % 2 ? "写作专家" : "代码专家"} description="协助阅读代码与排查问题" frontImage={FOX_ASSISTANT_AVATAR} autoCollapseKey={sent} />
      <div style={{ margin: '180px auto', width: 500, maxWidth: '80%' }}>
        <button id="switch-expert" onClick={() => setExpert((value) => value + 1)}>更换专家</button>
        <button id="send-message" onClick={() => setSent(value => value + 1)}>发送消息</button>
        <button id="cycle-preview" onClick={() => dispatchNotificationPreview('soft')}>轮播通知</button>
        <h1 style={{ fontSize: 24, marginBottom: 20 }}>当前对话</h1>
        <p>向下拉一下卡片，松手收起。悬停顶部边缘，再向下拉展开。</p>
        <p style={{ margin: '16px 0', color: 'var(--muted-foreground)' }}>聊天内容与输入区保持原位。点击卡片可翻面，方向键也可展开或收起。</p>
        <button type="button" id="show-notice" onClick={() => toast({ title: '专家已更换', description: '当前对话和聊天记录已保留。', type: 'success', duration: Infinity })} style={{ border: '1px solid var(--border)', borderRadius: 8, padding: '8px 16px' }}>显示测试通知</button>
      </div>
    </div>
    <ToastStack label="通知更新" position="top-right" visibleToasts={3} />
  </div>
}
createRoot(document.getElementById('root')!).render(<ToastStackProvider><Fixture /></ToastStackProvider>)
