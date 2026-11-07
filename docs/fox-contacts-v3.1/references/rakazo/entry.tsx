import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { MessageView } from './upstream/message-view';

const messages = [
  { id: 'user-1', role: 'user', text: 'Find time with Sam next week.' },
  { id: 'bot-progress', role: 'bot', text: 'Checking your calendars…' },
  { id: 'bot-reply', role: 'bot', text: 'Tuesday afternoon is free.' },
  { id: 'user-2', role: 'user', text: 'Tuesday works. Please prepare the invite.' },
  { id: 'bot-2', role: 'bot', text: 'I’ll prepare it for Tuesday at 2 PM. You can review the invitation before it is sent.' },
].map(({text, ...m}, i) => ({ ...m, threadId: 'preview', botId: 'chief', seq: i + 1, createdAt: '2026-11-07T09:00:00Z', blocks: [{ kind: 'text', text }] }));
const noop = () => {};
const resolved = async () => {};

function App() {
  const [theme, setTheme] = useState('light');
  const [notice, setNotice] = useState('鼠标移到任意气泡旁，查看表情、回复和更多操作。');
  function changeTheme() {
    const value = theme === 'light' ? 'dark' : 'light';
    document.documentElement.dataset.theme = value;
    setTheme(value);
  }
  return <main className="preview-page">
    <header className="preview-header"><div><h1>Rakazo 消息组件预览</h1><p>上游原版气泡与悬停组件 · 示例对话 · 无需登录</p></div><button className="preview-theme" onClick={changeTheme}>{theme === 'light' ? '切换深色' : '切换浅色'}</button></header>
    <div className="preview-note">对话示例：请安排与 Sam 的会议 → 正在查日历 → 周二下午有空 → 准备邀请，发送前确认。</div>
    <section className="preview-chat" aria-label="Rakazo 原版消息气泡">
      <div className="preview-chat-title">Chief <span>示例联系人</span></div>
      <div className="rk-scroll isolate flex min-h-0 flex-1 flex-col gap-2 overflow-x-hidden overflow-y-auto px-7 py-8" data-testid="transcript">
        {messages.map(message => <div key={message.id} data-message-id={message.id} className="group/message relative hover:z-[1]">
          <div className={`relative flex ${message.role === 'user' ? 'justify-end' : 'justify-start'}`}>
            <div data-testid="message-bubble-frame" className={`relative w-fit min-w-0 ${message.role === 'user' ? 'max-w-[min(84%,calc(100%_-_8rem),39rem)] [@media(hover:none)]:max-w-[min(84%,39rem)]' : 'max-w-[min(88%,calc(100%_-_8rem),39rem)] [@media(hover:none)]:max-w-[min(88%,39rem)]'}`}>
              <MessageView message={message as any} artifactTarget={{botId:'chief'}} canAnswer={false} canDismiss={false} peerBot={() => undefined} onAnswer={resolved} onOpenBot={noop} onOpenPeerMessages={noop} onRefresh={resolved} onBotChanged={resolved} onAddRoutine={noop} voiceReady={false} speaking={false} callHoldsSpeaker={false} onSpeak={noop} onOpenComputer={noop} rpc={{} as any} showToolActivity={false} onReply={() => setNotice('这里展示回复按钮。预览没有连接聊天后台。')} onMarkUnread={() => setNotice('这里展示标为未读操作。预览不会保存聊天状态。')} onReact={async (_message, reaction) => setNotice(`选择了 ${reaction}。预览不会发送表情。`)} />
            </div>
          </div>
        </div>)}
      </div>
    </section>
    <p className="preview-help" role="status">{notice}</p>
    <footer>代码快照 171a20e3 · 使用原版 MessageView、MessageHoverActions、MessageHoverMetadata、菜单、弹出层和主题。<br/>纯文本示例适配了翻译和数据接口；没有启动模型、日历或审批服务。<a href="https://github.com/elie222/rakazo/blob/171a20e389fe93296dc1f5b4224e76b00a029b36/apps/web/src/pages/shell/message-view.tsx" target="_blank" rel="noreferrer">查看上游源码</a></footer>
  </main>;
}
createRoot(document.getElementById('root')!).render(<App />);
