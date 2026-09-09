# Kernel Office 修复包构建（2026-09-10）

Windows x64 NSIS 安装器已成功构建，源码提交 `0d5d9007049c0854dd8f98865d39b7aa56b91403`。包含两轮用户反馈修复；具体范围见 [Office 附件与知识库修复](Kernel-Office附件与知识库修复-2026-09-10.md)。

- ZIP：`output/kernel-office-20260910/Fox-NewKernel-Preview-0d5d900-windows-x64.zip`，115,255,334 字节。
- ZIP SHA-256：`c1cc40183a93b40a4d26f2eb234d82be4c3b6d72e6499a658318516b7b4874c1`。
- 安装器 SHA-256：`020c45b5b1cc7932107293cee1e1175a51540052bf7b85b6346a4d2c22039bd0`。
- Runtime SHA-256：`93a538272652bab0be784fc95093184fb9e0879f8b802c78642d674a6d648584`。
- 已确认 Fox Kernel Preview 产品标识和 `kernel-default` 构建特性，并核对 37 个打包文件摘要；ZIP 中 5 个文件全部读取并与来源摘要一致。
- 安装器仍未签名。关闭 Fox Kernel Preview 后覆盖安装，保留既有数据目录。用户电脑与真实供应商复测尚未完成。

构建日志、验证摘要与分发包保存在对应 output 目录；二进制未加入 Git。应用源码及构建记录只在本地提交，未推送远端。
