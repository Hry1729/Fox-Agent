# Fox Preview Dependency Installation

PDF、DOCX、电子表格与 PPTX 查看器已经接入。所有查看器均在对应文件被打开时动态加载，不进入 Fox 首屏包。

当前依赖：

- `pdfjs-dist`: 基于 Fox 受控原件缓存的单页 PDF 渲染、缩放与全文搜索。
- `docx-preview`: DOCX 正文、图片、表格、页眉页脚的本地只读预览；可识别分页时逐页展示，否则使用受限连续滚动。
- `@js-preview/excel`: XLS/XLSX/ODS 本地只读预览，多工作表切换。
- `@aiden0z/pptx-renderer`: PPTX 本地单页预览和翻页。

`pptx-react-viewer` 2.x 因发布包保留 `pptx-viewer-mcp@workspace:*` 而无法通过标准包管理器安装，不能作为 Fox 的稳定依赖。Fox 已改用 `@aiden0z/pptx-renderer` 1.2.4（Apache-2.0）。安装命令：

From `D:\python\projects\Fox\Fox`, run:

```powershell
pnpm.cmd --store-dir D:\.pnpm-store\v11 --dir apps/desktop add @aiden0z/pptx-renderer
```

Then verify:

```powershell
pnpm.cmd --dir apps/desktop build
bun test ./apps/desktop/tests
```

PPTX 适配器使用包内公开的 `PptxViewer.open`、`goToSlide` 和 `destroy` API，采用 `renderMode: 'slide'`、惰性幻灯片和惰性媒体。适配器必须保持动态导入、原文件缓存、64 MB 限制、Fox ZIP 安全检查、只读模式和卸载清理。

生产构建仍会报告若干大 chunk，主要来自 PPTX、WASM、图谱布局、Mermaid 和 Shiki 语言/主题集合。这些依赖已经与首屏路由部分隔离，进一步拆分登记为后续性能专项，不通过提高 `chunkSizeWarningLimit` 隐藏问题。
