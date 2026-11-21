---
id: fox-office-ppt
name: PowerPoint 演示制作
description: 按需制作可编辑 PPTX 幻灯片、讲稿备注和演示方案，逐页检查叙事、数据和排版。
version: 1.0.1
required_tools: [list_mcp_tools, call_mcp_tool]
---

只在任务涉及 PowerPoint/PPT/PPTX 时使用。遵守先内容审批再制作的用户要求。Office 文档连接器 serverId 为 fox-office；先 list_mcp_tools，按实际 Schema 调用 office_*。

1. 先从请求和材料确认受众、讲述时间、页数、模板、比例和交付位置。只问无法推断且影响结果的事项。先写大纲，只读页标题就应看懂叙事。每页只讲一个主要观点。把要求的数字、单位、派生指标和可编辑元素分配到相应页面。用户要求先审内容时，等其确认再制作。
2. office_read 读取现有模板；office_help(format=pptx,element=slide/shape/table/chart/notes) 查询所需参数，不编造坐标属性。
3. office_create(output=新.pptx) 新建或加 template 复制模板。office_edit(file=原文件,output=新副本,operations=[{command:add/set/remove,path:元素路径,type:类型,props:属性对象}]) 添加幻灯片、文本、表格、图表和备注，每批不超过 100 项。
4. 按模板设定一致字体、字号、间距和颜色，保留足够留白。优先用可编辑文本、形状和原生图表；不要把整页截图称为可编辑设计。未开放的图片/外部素材和效果明确标为限制，不能声称已生成配图。
5. 模板可用 office_merge 对 {{key}} 作确定性替换。用 office_read 重读成稿，检查页数、遗漏、错字、顺序和残留占位符。按逐页大纲核对数字、派生值、单位、图表标签和备注，用原生文字、形状或图表结构核对可编辑性。
6. 用 office_validate 校验结构。用 office_render（file、mode=screenshot、page）逐页渲染并查看预览。先看文字是否被裁切，再查遮挡、对齐、字号、对比度、表格和图表排版，以及缺字或字体替代。修订后重看受影响的页面。截图依赖本机支持的浏览器。预览由 Host 私下保存，用户要求图片文件时才另作交付。
7. 交付 PPTX，分别报告实际完成的内容、结构和视觉检查。未能看图时标明视觉未核验。预览无法证明复杂排版和动画在 Microsoft PowerPoint 中完全一致。默认另存副本，用户明确要求覆盖时传 overwrite=true。
