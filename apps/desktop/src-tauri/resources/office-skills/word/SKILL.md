---
id: fox-office-word
name: Word 文档制作
description: 按需创建、读取和编辑 Word DOCX 汇报稿、制度、纪要和手册，使用 Office 文档连接器保存并验证。
version: 1.0.1
required_tools: [list_mcp_tools, call_mcp_tool]
---

只在任务涉及 Word/DOCX 时使用，不改变用户要求的先审稿阶段。Office 文档连接器 serverId 为 fox-office。调用 list_mcp_tools 后使用受控 office_* 工具；工具 Schema 是参数事实源。

1. 先确认交付内容、受众、模板、输出路径和已有文件处理方式。读取模板用 office_read（file、mode=text/outline/get/query，get/query 另给 selector）。
2. 用 office_help(format=docx, element=paragraph/table/style 等) 查所需属性，不将 Word 对象模型和 CLI 参数混用。
3. 创建用 office_create(output=项目内的新.docx)，已有模板加 template；编辑用 office_edit(file=源文件,output=新副本,operations=[{command:add/set/remove,path:元素路径,type:元素类型,props:属性对象}])。不需要另行安装软件或开启 shell。每批不超过 100 项。
4. 常用结构是标题、段落、表格、页眉页脚和样式。优先复用模板，样式语义一致，中文字体以本机实际可用字体为准。长文档注意分页、表格跨页、编号和层级。
5. 批量套版可用 office_merge(file=模板,output=新文件,data=键值对象) 填充 {{key}}。涉及图片文件、外链、低层 XML 或未开放属性时明确限制，不绕过适配器。
6. 用 office_read 对照要求核对成稿的章节、事实、数字、表格、引用和待确认事项。写完 DOCX 后渲染并查看页面。结构用 office_validate，页面用 office_render（file、mode=screenshot、page），也可用 mode=html 生成 HTML 预览。查看标题、分页、跨页表格、文字裁切和字体替代。修改后重看受影响的页面。预览默认由 Host 私下保存，实际排版仍以目标软件打开的文件为准。
7. 默认另存副本。用户明确要求覆盖时传 overwrite=true。交付 DOCX，报告文件路径和实际完成的内容、结构与版式检查。未能查看预览时，标明版式未核验。
