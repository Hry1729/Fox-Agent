# Fox Office 文档连接器

固定版本：OfficeCLI 1.0.147。`release.json` 记录各平台下载资产与 SHA-256，构建前和调用前均校验。开发启动、桌面打包前会运行 `scripts/prepare-officecli.mjs`；下载失败或哈希不匹配会停止准备，不运行上游安装脚本。最终二进制随 Fox 资源分发，终端用户不需要另外安装 OfficeCLI。

Fox 启动后注册 `fox-office`，插件中心展示为“连接器 / Office 文档”。底层采用结构化 CLI 适配，共用 Fox 的连接器发现与调用入口，不把通用 CLI 命令行直接暴露给模型。可停用，不能在连接器设置中修改启动命令或删除；版本跟随 Fox 更新。

提供 `office_help`、`office_read`、`office_create`、`office_edit`、`office_merge`、`office_render`、`office_validate`。文件必须在会话授权项目内；只读模式拒绝写文件（包括预览输出），询问模式审批具体文件与操作，待审批的计划阻止写入。编辑默认另存副本；覆盖必须显式设置 `overwrite=true` 并保留备份。先处理临时文件，经过结构校验和再次读取后才发布输出。

首版仅支持 DOCX/XLSX/PPTX，不开放原始 XML、任意 CLI 参数、安装更新、图片资产插入或外部网络/DDE 公式。模板可能包含复杂外部关系，本适配器不是操作系统级文档沙箱；只使用可信模板。截图依赖本机浏览器；OpenXML 校验不能替代内容、公式和视觉检查，也不代表与 Microsoft Office 完全等价。

三个随附技能位于 `../office-skills/`，启动时写入 Fox 数据目录，保留已存在的本地文件。已适配专家按用途绑定这些技能；单独的技能本身不扩大工具权限。

许可证、NOTICE 和第三方依赖声明与二进制同目录随包分发。上游：[OfficeCLI](https://github.com/iOfficeAI/OfficeCLI)。

维护验证：在仓库根运行 `node scripts/prepare-officecli.mjs`；在 `apps/desktop/src-tauri` 运行 `cargo test --lib --no-default-features real_documents_roundtrip -- --ignored --nocapture`。该专项测试真实执行二进制，覆盖三类文档的创建、修改、读回、HTML 输出、源文件保持和失败不发布；其简单样例不是复杂文档兼容性认证。
