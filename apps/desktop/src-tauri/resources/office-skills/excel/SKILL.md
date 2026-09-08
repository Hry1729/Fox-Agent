---
id: fox-office-excel
name: Excel 表格处理
description: 按需读取和编辑 Excel XLSX 工作簿、数据台账、公式和图表，核对数据口径与计算结果。
version: 1.0.0
required_tools: [list_mcp_tools, call_mcp_tool]
---

只在任务涉及 Excel/XLSX 时使用。Office 文档连接器 serverId 为 fox-office；用 list_mcp_tools 发现实际 office_* 工具。CSV/TSV/JSON 简单分析继续使用 tabular_data，不能声称它直接支持 XLSX。

1. 先核对数据来源、字段、期间、单位、样本与原始工作表。用 office_read(file,mode=text/get/query,selector=所需单元格或选择器) 读取，禁止凭空补造数据。
2. 用 office_help(format=xlsx,element=cell/sheet/chart/pivottable 等) 查询结构与属性。office_create(output=新.xlsx) 新建；office_edit(file=原文件,output=新副本,operations=[{command:add/set/remove,path:元素路径,type:类型,props:属性对象}]) 编辑，每批不超过 100 项。
3. 保留原始数据，清洗和汇总另建工作表；公式应可追溯到输入，使用一致的格式、单位和列标题。上游函数支持不等于与 Excel 全面兼容，未支持公式必须明确说明。
4. 首版拒绝外部引用、网络、DDE 公式及网络资产属性，不用其他命令绕过。需要此类来源时先由 Fox 的授权检索取得数据，再以明确值写入。
5. 写入后 office_read 重读关键单元格和公式结果，用已知小样本或独立计算复核合计、比例、边界值和分母。OpenXML 校验通过只说明结构，不代表公式或业务结论正确。
6. 用 office_render(file,mode=screenshot,output=新.png) 或 HTML 预览检查表头、单位、截断、图表标签和图例。无视觉能力则如实注明。
7. 默认另存副本，只有用户明确要求覆盖时设置 overwrite=true。交付工作簿、口径、计算核对结果和局限；Fox 任务的导出表注明快照时间，不替代任务事实源。
