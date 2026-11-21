---
id: fox-office-excel
name: Excel 表格处理
description: 按需读取和编辑 Excel XLSX 工作簿、数据台账、公式和图表，核对数据口径与计算结果。
version: 1.0.1
required_tools: [list_mcp_tools, call_mcp_tool]
---

只在任务涉及 Excel/XLSX 时使用。Office 文档连接器 serverId 为 fox-office；用 list_mcp_tools 发现实际 office_* 工具。CSV/TSV/JSON 简单分析继续使用 tabular_data，不能声称它直接支持 XLSX。

1. 先核对数据来源、字段、期间、单位、样本与原始工作表。用 office_read(file,mode=text/get/query,selector=所需单元格或选择器) 读取，禁止凭空补造数据。
2. 用 office_help(format=xlsx,element=cell/sheet/chart/pivottable 等) 查询结构与属性。office_create(output=新.xlsx) 新建；office_edit(file=原文件,output=新副本,operations=[{command:add/set/remove,path:元素路径,type:类型,props:属性对象}]) 编辑，每批不超过 100 项。
3. 保留原始数据，清洗和汇总另建工作表；公式应可追溯到输入，使用一致的格式、单位和列标题。上游函数支持不等于与 Excel 全面兼容，未支持公式必须明确说明。
4. 首版拒绝外部引用、网络、DDE 公式及网络资产属性，不用其他命令绕过。需要此类来源时先由 Fox 的授权检索取得数据，再以明确值写入。
5. 按用户指定的工作表名、列标题和公式制作。公式可以正常计算，也可能引用了错误的行或范围。批量铺开前先写两三个公式，核对取值。用 office_read 检查保存的公式和缓存值，独立核算合计、比例、边界值和分母。任务要求重算时，在副本中修改输入，用获准且可用的计算引擎核对变化。没有引擎时标明重算未验证。
6. 用 office_validate 校验结构。用 office_render 的截图或 HTML 预览检查表头、单位、图表标签和图例是否被截断。预览默认由 Host 私下保存。标明未完成的视觉检查。
7. 默认另存副本。用户明确要求覆盖时传 overwrite=true。报告计算口径和实际完成的检查。导出的 Fox 任务数据应注明快照时间，任务状态以 Host 记录为准。
