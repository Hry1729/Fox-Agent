import '../src/styles/globals.css'
import '../src/styles/workbench.css'
import '../src/styles/workspace-pages.css'
import '../src/styles/areas/sidebar.css'
import '../src/styles/areas/chat.css'
import '../src/styles/areas/composer.css'
import '../src/styles/areas/pages.css'
import '../src/styles/areas/settings.css'
import { ENABLED_EXTERNAL_AGENTS_KEY } from '../src/features/conversations/model/external-agent-enablement'

// 独立预览端口的示例状态，不访问 Tauri 或实际桌面数据。
localStorage.setItem(ENABLED_EXTERNAL_AGENTS_KEY, JSON.stringify(['pi', 'codex', 'deepseek']))
import('./preview').then(({ mount }) => mount())
