# Fox 桌面性能采集（profile 构建专用）

仅在性能分析构建中采集数据。普通 release 经 tree-shake 移除全部采集代码、诊断 UI、
`react-dom/profiling` 与 profiling chunk，不引入任何包体开销；`recordRegionRender` 的
telemetry 转发在 `__FOX_PROFILING__ === false` 时被静态消除，`onRender` 永不触发
（由 `scripts/check-build-isolation.mjs` 自动校验）。

> 注：包裹 Timeline/Composer 的 `<Profiler>` 元素本身保留在组件树中。在标准 React
> production renderer 里 `<Profiler>` 不做任何工作（不注册回调、不测量），因此没有
> 可观测的测量/回调开销；采集逻辑本身则完全不出现在 release 包里。

## 为什么需要专用构建

标准 React production renderer 会编译掉 `<Profiler onRender>`，普通 release 里加开关也采不到
`actualDuration`。profile 构建在 `vite.config.ts` 中按 `mode === 'profile'`：

- 注入静态常量 `__FOX_PROFILING__`（**不依赖任何 .env 文件**，干净 checkout 行为一致）；
- 把 `react-dom/client` 别名到 `react-dom/profiling`（优化构建，保留 Profiler，非 dev）。

## 构建与打开

```bash
pnpm --dir apps/desktop build:profile     # tsc + vite build --mode profile + profile 隔离校验
pnpm --dir apps/desktop preview            # 浏览器预览 profile 产物
```

在 **Tauri（真实 WebView2）** 中采集，用专用配置启动，不会触发普通 `pnpm build` 覆盖 profile 产物：

```bash
pnpm --dir apps/desktop tauri:profile          # tauri dev，Vite 以 --mode profile 跑在 1422 端口
pnpm --dir apps/desktop tauri:profile:build    # tauri build，beforeBuildCommand 为 build:profile
```

- `tauri:profile` 使用 `src-tauri/tauri.profile.conf.json`（`beforeDevCommand=pnpm dev:profile`、
  `devUrl=127.0.0.1:1422`），与普通 `tauri`（1421 端口、普通 dev）互不干扰。
- 启动后在窗口里点右下角 `PERF` 采集真实 App；或导航到 `http://127.0.0.1:1422/?foxPerf=1` 使用合成夹具。

CI 中 `build:profile` 与 release/profile 隔离检查已纳入（见 `.github/workflows/windows-ci.yml`）；
浏览器性能 smoke 因 CI runner 时序波动较大，保留为手动/定期任务：

- `scripts/prof-smoke.mjs` — headless Chrome 跑合成夹具（五场景/settle/dropped 断言）。
- `scripts/realapp-smoke.mjs` — headless Chrome 真实 App 页采集。
- `scripts/webview2-smoke.mjs` — 在**真实 Tauri WebView2**内驱动采集。启动带调试端口的
  `tauri:profile`：`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9335
  pnpm --dir apps/desktop tauri:profile`，然后 `bun scripts/webview2-smoke.mjs`（CDP_PORT 默认 9335），
  它会打开 PERF 控制器 → 开始采集 → composer 输入触发 Workbench 提交 → 停止并读取场景摘要。

`buildId` 在 `vite.config.ts` 中按 git 状态生成，纯行尾（CRLF）差异忽略，tracked/staged/untracked
分别检查；内容干净时为 `<mode>+<sha>`，无未提交改动则不带 `-dirty`。

两种采集方式（互斥）：

1. **真实 App 采集**：打开普通 profile 页面（**不带** `foxPerf`），右下角 `PERF` 按钮展开
   `RealProfilingController`。开始采集后在真实 App 内操作（会话切换、流式回答、滚动、composer 输入），
   停止并结束场景、导出 JSON。场景标签只允许短的非敏感值（real-conversation / real-stream /
   real-scroll / real-composer），不记录正文、标题、路径或输入。Workbench 的 `<Profiler onRender>`
   在 profile 构建里会转发到同一采集器。
2. **合成夹具**：打开 `http://<host>/?foxPerf=1`（或点 `HARNESS` 跳转）。互斥：此时**只**挂载夹具，
   不挂载真实 App/Workbench，后台不会混入提交。
   - `?foxPerf=auto`：自动依次运行 code-cold → code-warm → 混合 500 → Markdown 流 → 阻塞场景，
     JSON 写入 `#fox-perf-result`（供 `scripts/prof-smoke.mjs` 断言）。
   - 阻塞场景：基线建立后单次任务做重型 commit + 同步阻塞 ~95ms，由真实浏览器时序产生
     React commit、超预算帧（遗漏 vsync）和 Long Task。
   - 代码 cold→warm：cold 作为该页首个场景，Shiki 动态 import 确实从未加载；warm 是第二次独立测量。

每个场景在 `beginScenario` 后**先等一个 rAF 建立帧基线**，workload 在后续任务执行，确保首个重任务的
rAF gap / Long Task 不被漏采，也不把场景前空闲间隔计入。

## 采集口径（不要混淆）

- React `actualDuration`：React 该次提交的 **render duration**，**不含** layout/paint/GC。
- 帧间隔（rAF）：刷新率自动估算并 snap 到 60/90/120/144/165/240Hz；用**遗漏 vsync 数**
  （`round(gap/budget)-1`）判断超预算帧，而非固定阈值。隐藏/失焦/聚焦重置基线，不把后台节流算掉帧；
  前台真实冻结（>=200ms）保留。
- Long Task：W3C 阈值 **50ms**，经 `PerformanceObserver.supportedEntryTypes` 能力检测；按
  `entry.startTime` 与场景时间窗匹配 `scenarioRunId`，回调延迟也能正确归属；`endScenario` 前
  `takeRecords()` 防末尾丢失。
- 帧/Long Task 是全局指标，按场景时间窗相关性归属，不声称属于某组件。

每次 `beginScenario` 生成唯一 `scenarioRunId`，render/frame/longtask 全部归属该实例，连续同类
场景互不串样本；`onRender` 仅在采样启动且有活动场景时记录。Ring buffer 为 O(1) 循环缓冲。

## 夹具语义

- 规模数字（100/500/1000）是**消息总数**（user+assistant），轮数为 floor(size/2)。
- history 场景真正卸载后重挂载 `RuntimeTimeline`（key 翻转），在场景时间窗内产生 `mount` commit。
- markdown / code / tools / mixed 各生成真实内容：code 含 fenced ```typescript 块（懒加载 Shiki），
  tools 含 `tool.started/tool.completed` 生命周期事件。
- 流式回放包含 `run.started → message.started → [tool.*] → message.delta… → message.completed → run.completed`
  完整生命周期，复用生产 `enqueueRuntimeEvent` / `takeRuntimeEventFrame`（RAF 批处理）/
  `reduceRuntimeNotifications`，但由夹具手工驱动，**不是**完整 `useRuntimeEventStream` 订阅链路。
- 等待懒加载内容稳定用 **Profiler commit quiet window**（提交数连续 30 帧不变），结果
  `settle.status` 为 `settled` / `timed-out` 并附等待时长写入报告。
- 全程内存合成数据，不写用户数据库。导出 JSON 只含时长、计数、region 与元数据（build/Git ID、
  userAgent、viewport、DPR、刷新率、cold/warm、消息数、事件数、样本数、能力支持），**不含**
  消息正文、文件路径或用户数据。

## 自动校验

```bash
pnpm --dir apps/desktop build            # 预算门禁 + release 隔离（telemetry/harness/profiling 缺席）
pnpm --dir apps/desktop build:profile    # profile 隔离（profiling renderer/telemetry/harness 在场）
```

## 决策门槛（provisional，非 CI 硬门禁）

真实 500 条消息对话中 Timeline **update** commit P95 持续 > ~16ms，或滚动持续掉帧，再启动消息虚拟化。
16ms 是 60Hz 整帧预算；先固定机器、窗口尺寸、WebView 版本、预热与重复次数，记录 100/500/1000
增长曲线、mount/update 分开的 P50/P95、流式 commit 数与时长、超预算帧比、遗漏 vsync、Long Task 数与
相对回归幅度。公共 CI 不做绝对性能断言。
