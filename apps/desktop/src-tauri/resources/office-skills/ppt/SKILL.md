---
id: fox-office-ppt
name: PowerPoint 演示制作
description: 按需制作可编辑 PPTX 幻灯片、讲稿备注和演示方案，逐页检查叙事、数据和排版。
version: 1.0.0
required_tools: [list_mcp_tools, call_mcp_tool]
---

只在任务涉及 PowerPoint/PPT/PPTX 时使用。遵守先内容审批再制作的用户要求。Office 文档连接器 serverId 为 fox-office；先 list_mcp_tools，按实际 Schema 调用 office_*。

1. 确认受众、时长、页数、模板、比例和输出位置。先逐页列核心结论、证据、图表和讲述顺序，一页只承担一个主要信息任务。
2. office_read 读取现有模板；office_help(format=pptx,element=slide/shape/table/chart/notes) 查询所需参数，不编造坐标属性。
3. office_create(output=新.pptx) 新建或加 template 复制模板。office_edit(file=原文件,output=新副本,operations=[{command:add/set/remove,path:元素路径,type:类型,props:属性对象}]) 添加幻灯片、文本、表格、图表和备注，每批不超过 100 项。
4. 按模板设定一致字体、字号、间距和颜色，保留足够留白。优先用可编辑文本、形状和原生图表；不要把整页截图称为可编辑设计。未开放的图片/外部素材和效果明确标为限制，不能声称已生成配图。
5. 可用 office_merge 对模板 {{key}} 做确定性填充。生成后 office_read 检查页数、顺序、数字、图表标签和讲稿，不能只读最后一页就确认全部完成。
6. office_render(file,mode=screenshot,output=新.png,page=具体页码) 逐页预览。检查溢出、重叠、对齐、字号、表格、图表与字体替代，修订后重看受影响页面。PNG 依赖本机支持的浏览器；不可用时明确视觉未核验。
7. 保存并结构校验后交付 PPTX 与检查状态。只报告实际验证的内容，不承诺与 Microsoft PowerPoint 的所有动画和复杂排版完全一致。默认另存副本；覆盖需明确请求和 overwrite=true。
