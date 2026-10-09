# r16 P0-2 文本与 CSV 交付与交接

> 角色范围对应方案 §11.5（B 文本与 CSV）。**执行者：协调者**（原因见 §7 执行模式变更）。
> 基点：`6e0bb5d`（C0）→ 本次 head 见 §1。
> 期望来源：方案 §8 R01–R09、§4.1、§11.10；本文件同时对每一处**语义判断**给出理由。

## 1 本次变更

| 文件 | 变更 |
|---|---|
| `apps/desktop/src-tauri/src/runtime_host/delivery_checks_v2/text_csv.rs` | 由 C0 占位改为**真实实现**：`check_text_concat`、`parse_table`、`check_table_columns`；随后按 G 审查修正（`f61a288`，见 §4.1），再按 G 独立复验修正 R1/R2/R3（`8b4b063`，见 §4.2）。**本文件共 36 项测试** |
| `apps/desktop/src-tauri/src/runtime_host/delivery_checks_v2/mod.rs` | 接口扩展 3 处（见 §2）：`equivalence` 字段与 `passed_with_equivalence`；2 个行结束 reason code；`TableError.limit` 与 `TableError::budget` |

**未变更**：`delivery.rs`、`delivery_content_requirements.rs`、`data_compute.rs`、`Cargo.*`、数据库、前端。本模块**尚未接入任何生产路径**，因此不可能因为"忘记接线"而改变现有运行行为，也不可能因为占位而误放行。

## 2 接口扩展（C0 冻结后发现的缺口，按方案 §11.1 由 A 发布新基点）

期望设计时发现 C0 接口不足以表达 R01/R05 的要求。三处扩展都是**最小必要**的，且不改变既有三态语义：

| 扩展 | 为什么 C0 不够 | 为什么这样表达 |
|---|---|---|
| `CheckObservation.equivalence: Option<String>` + `equivalence::OPTIONAL_FINAL_LF` | R01 要求"通过**并记录等价**"，而 C0 的 `passed()` 把 `expected`/`actual`/`detail` 全部留空，等价只能写进自由文本，无法被断言或审计 | 结构化字面量，可序列化、可断言。`passed` 依然不带 reason code（保持"通过无原因码"的既有设计） |
| `reason_code::TEXT_LINE_ENDING_VIOLATION` / `TEXT_LINE_ENDING_UNSUPPORTED` | R05 要求"按显式政策处理；不暗中全局转换，**不支持则未核验**"，C0 没有任何码能表达行结束问题 | **分成两个码**，各自只对应一个状态：声明被违反 = `failed`；声明无法决定 = `unverified`。同一个码绝不同时表示两种状态 |
| `TableError.limit: Option<LimitKind>` + `TableError::budget` | 方案要求"解析器超预算返回明确限制，不循环调用模型"，但 C0 的 `TableError` 只有文本，调用方无法知道**是哪个**上限被触及，也就无法复用 `budget_exceeded` 的显式报告 | 与 `CheckObservation.limit` 同名同类型，调用方可直接映射，不需要解析字符串 |

## 3 实现语义（含判断与理由）

### 3.1 文本合并

1. **冻结期望的构造逐字节复刻 v1**（`expected_concat`）。因此 v1 合同语义**完全不变**，v2 的差异只可能出现在其政策明确允许处。
2. **v1 或未声明 `TextSegments` 的合同保持逐字节比较**。F01 的修法不是"放宽所有比较"，而是"新合同可声明等价、旧合同不静默升级"。
3. **唯一的等价**：`EofPolicy::AllowSingleTrailingLf` 下，两侧各至多一个终止 LF 视为等价。**两个或更多不算等价**（那是多余空行，不是 EOF）。正文一律不 `trim`、不丢尾空白、不做 CRLF→LF 全局转换。
4. **等价只在真正依赖它时记录**。字节完全一致的候选是普通通过，`equivalence` 保持 `None`。
   - 依据：`passed_with_equivalence` 的语义是"这次通过**依赖了**某条被允许的等价"。给普通通过也贴上等价标记，会让审计无法区分"本来就一样"和"差一个 LF 被容忍"——那正是 F01 要消除的模糊性。方案 R01 的"两者通过并记录等价"由"无 LF 的那份记等价、有 LF 的那份……"这一读法会产生歧义，本实现取**更可审计**的一读，并在此明确记录该判断。
5. **失败按最早可定位的原因分级**，顺序为：末尾 LF 数不同 → 来源集合缺失 → 来源顺序 → 段间空行 → 正文差异。每级都给出 `expected`/`actual` 与字节位置，并**限长渲染**（前后各 16 字节）。
   - 这样 O19 类的失败不会再得到"整体格式不符"这种推动整篇重写的信息。
6. **行结束处理**（R05）：
   - 候选与冻结期望**逐字节相同** → 直接通过，不进行任何行结束裁决（冻结合并本身可以合法地携带其来源的换行符）。
   - 孤立 CR → `unverified`（`TEXT_LINE_ENDING_UNSUPPORTED`）。没有任何声明覆盖它，猜它是换行就是方案禁止的隐式转换。
   - 声明 `LF` 而候选含 CRLF（或反过来）→ `failed`（`TEXT_LINE_ENDING_VIOLATION`）。这是合同明确声明过的要求，属于真实格式错误。
   - 声明 `Any`（或 v1 未声明）而候选含 CRLF 且**未能逐字节相同** → `unverified`。理由：此时合同并未决定使用哪一种约定，而冻结合并的分隔符是 LF；既不能转换（禁止），也不能据此断言内容错（未证明）。
   - 判断记录：R05 的"不支持则未核验"被实现为"**未声明约定** → 未核验"，"**声明被违反** → 失败"。这两者在原文里是同一句话，本实现按"是否有显式声明"切开，以免把真实的违规错误降级为未核验。
7. **诊断扫描有上限**（`DIAGNOSIS_SCAN_BYTES = 1 MiB`）。超过上限时仍定位首个差异字节，但不再搜索"标题是否乱序"。
   - 理由：定位"哪条规则坏了"的代价不得超过检查本身。无上限的乱序搜索在大文件上是 O(文件 × 来源数)。超限时的降级是**把乱序报成内容差异**，只会让诊断略粗，不会产生误放行。

### 3.2 有界表格解析器（F02 的核心）

**同一个 `parse_table` 就是"记录"和"字段"的唯一权威定义**，基础检查、内容检查与统计行数必须都改用它（接线见 §5）。

- 显式方言：`delimiter` / `quote` / `bom` / `line_ending` / `blank_line` / `header`，全部来自冻结合同，不猜。
- 支持引号内分隔符、引号内换行、`""` 转义；`quoted` 标记被保留，供后续区分数值与文本型数值（P1 的 F03 需要）。
- **记录号与物理行号分开**：`record` 是逻辑记录号（表头为 1），`line` 是它开始的物理行号。因此"引号内换行"不会让两者意外一致。
- 空行按 `BlankLinePolicy`：默认 `Skip`——**空白行不是错宽记录**（这正是现有 `delivery_content_requirements.rs::delimited` 与目标方言的"旧拒/新过"差异点，属 G 影子比对必须解释的项）。
- **从不改值、从不去空白、从不转类型**：`" "` 保留为 `" "`。
- 失败分类严格区分"读不了"与"数据不对"：`TABLE_UNTERMINATED_QUOTE`（引号未闭合）、`TABLE_CHARACTER_AFTER_QUOTE`、`TABLE_UNESCAPED_QUOTE`、`TABLE_ENCODING_UNSUPPORTED`（非 UTF-8 / BOM 违规）、`TABLE_DIALECT_UNSUPPORTED`（孤立 CR、声明被违反）、`BUDGET_EXCEEDED`（带上限名称）。
- 列检查：空列名、**仅在合同声明唯一性时**才查重复列名、真错宽（定位到记录与行）、零记录按合同声明决定。**不擅自新增未声明的列名唯一性或连续日期约束。**

## 4 R01–R09 覆盖对照

| 编号 | 覆盖测试 | 断言的状态与 reason code |
|---|---|---|
| R01 | `r01_single_optional_terminating_lf_passes_and_records_the_equivalence`、`r01_a_v1_envelope_keeps_byte_exact_semantics_and_stays_strict` | 普通通过 `equivalence=None`；依赖等价通过 `equivalence=optionalFinalLf` 且 `blocking=false`；v1 下同一输入 `failed/TEXT_EOF_MISMATCH` |
| R02 | `r02_byte_exact_and_more_than_one_lf_are_refused_with_an_exact_eof_difference`、`r02_require_trailing_lf_demands_exactly_one` | `failed/TEXT_EOF_MISMATCH`，`expected`/`actual` 为确切 LF 个数 |
| R03 | `r03_content_changes_fail_even_when_a_trim_would_hide_them`、`r03_a_deleted_inner_blank_line_is_a_separator_difference` | 删字/改缩进/加尾空格 → `failed/TEXT_CONTENT_MISMATCH` 且**有定位**；段间空行 → `failed/TEXT_SEPARATOR_MISMATCH` |
| R04 | `r04_a_reordered_selection_is_an_order_finding`、`r04_a_missing_source_is_a_source_finding_not_a_content_finding`、`r04_a_contract_without_sources_is_unverified_not_failed`、`r04_chinese_names_and_size_ties_are_accepted` | 乱序 → `TEXT_ORDER_MISMATCH`；缺来源 → `TEXT_SOURCE_MISSING`；合同无来源 → `unverified/CONTRACT_UNREADABLE`（非 failed）；中文文件名与等长大小并列 → 通过 |
| R05 | `r05_crlf_against_an_lf_declaration_is_a_located_violation`、`r05_an_undeclared_convention_is_never_silently_converted_and_never_a_false_reject`、`r05_a_lone_cr_is_unverified_and_never_silently_converted` | 声明 LF 遇 CRLF → `failed/TEXT_LINE_ENDING_VIOLATION`；未声明约定遇 CRLF → `unverified/TEXT_LINE_ENDING_UNSUPPORTED`（**且字节相同者仍通过**）；孤立 CR → `unverified` |
| R06 | `r06_quoted_commas_escapes_and_embedded_newlines_are_single_fields` | 引号内逗号/换行/`""` 转义均不被当作分隔；`record` 与 `line` 分离 |
| R07 | `r07_bom_crlf_and_tsv_all_parse_under_their_own_dialect`、`r07_a_required_bom_that_is_absent_is_an_encoding_finding` | BOM 剥离并记录 `bom_stripped`；CRLF 正常；TSV 分隔正确；要求 BOM 而缺失 → `TABLE_ENCODING_UNSUPPORTED` |
| R08 | `r08_a_real_eight_versus_seven_width_error_is_located_by_record`、`r08_an_unterminated_quote_is_a_parse_finding_not_a_width_finding`、`r08_an_unescaped_quote_is_reported_with_its_position`、`r08_duplicate_headers_are_refused_only_when_the_rule_declares_uniqueness`、`r08_an_empty_header_field_is_reported` | 8/7 → `failed/TABLE_WIDTH_MISMATCH`（8 个字段 vs 7 个字段，record/line 均定位）；未闭合引号 → `TABLE_UNTERMINATED_QUOTE`（**不报错宽**）；重复列名仅在声明时 `TABLE_HEADER_DUPLICATE`，未声明时**通过** |
| R09 | `r09_blank_lines_are_not_counted_as_wrong_width_records`、`r09_zero_records_is_decided_by_the_declaration`、`r09_whitespace_only_fields_keep_their_value` | 空行不计为错宽记录且行号保持物理值；零记录按声明 `TABLE_ZERO_RECORDS` 或通过；空白值原样保留 |
| 资源 | `a_ceiling_is_reported_as_an_explicit_budget_stop` | `BUDGET_EXCEEDED` 且 `limit` 分别命中 `Records`/`Fields`/`Bytes`，不误报为内容错误 |

**每个用到的 reason code 都至少有一条断言**（`TEXT_EOF_MISMATCH`、`TEXT_CONTENT_MISMATCH`、`TEXT_SEPARATOR_MISMATCH`、`TEXT_ORDER_MISMATCH`、`TEXT_SOURCE_MISSING`、`TEXT_LINE_ENDING_VIOLATION`、`TEXT_LINE_ENDING_UNSUPPORTED`、`CONTRACT_UNREADABLE`、`TABLE_ENCODING_UNSUPPORTED`、`TABLE_UNTERMINATED_QUOTE`、`TABLE_UNESCAPED_QUOTE`、`TABLE_CHARACTER_AFTER_QUOTE`、`TABLE_HEADER_EMPTY`、`TABLE_HEADER_DUPLICATE`、`TABLE_WIDTH_MISMATCH`、`TABLE_ZERO_RECORDS`、`TABLE_DIALECT_UNSUPPORTED`、`BUDGET_EXCEEDED`）。其中 `TABLE_CHARACTER_AFTER_QUOTE` 与 `TABLE_DIALECT_UNSUPPORTED` **尚无直接用例**——见 §8 已知缺口。

### 4.1 F-P02 逐项处置（依据 `G-P02-review.md`，修正提交 `f61a288`）

所有修正都在 `text_csv.rs` 的文本路径内；未改表格 parser、共享生产入口、`mod.rs`、公共 DTO、数据库、前端或 Cargo。

| 编号 | 级别 | 处置 | 定点证据 |
|---|---|---|---|
| F-P02-01 | P0 | **已修**。v1 完整保留旧字节比较：不匹配即确定 `failed` 且**阻断**，不再因含 CR/CRLF 降级为 `unverified`。依据：旧 `Contract::Concat` 只做 `bytes == expected`；且 `freeze_concat` 在 `delivery_content_requirements.rs:309` 直接拒绝含 `\r` 的来源，所以冻结的 v1 合并无 CR、比较完全可判定 | `f_p02_01_v1_keeps_a_total_byte_comparison`（覆盖字节相同／尾部 LF／CRLF／孤立 CR／真正文变化） |
| F-P02-02 | P0 | **已修**。行结束裁决不再抢先返回：只有在字节**没有**证明出"行结束无法解释的差异"时才报未核验；否则保持阻断失败并保留定位。判定用的行结束归一化**只用于在 `failed`/`unverified` 之间选择，永不产生通过** | `f_p02_02_a_known_body_error_behind_crlf_still_blocks` |
| F-P02-03 | P0 | **已修**。`RequireTrailingLf` 在任何候选上执行，且**先于**字节相同早退。反例可达的原因：`expected_concat` 末段不补 LF | `f_p02_03_require_trailing_lf_survives_byte_equality` |
| F-P02-04 | P0 | **已修**。明确声明的行结束政策不再被"等于构造"豁免，分两种情形：候选违反声明 → `failed`/`TEXT_LINE_ENDING_VIOLATION`；**冻结来源自身**与声明矛盾（合同自相矛盾）→ `unverified` 并附"矛盾"诊断，**不修改来源、不静默放行** | `f_p02_04_a_contradictory_contract_is_reported_not_waived`（字节相同反例）+ `r05_crlf_against_an_lf_declaration_is_a_located_violation`（候选违反） |
| F-P02-06 | P1 | **已修**。`RequireTrailingLf` 的通过不再记 `optionalFinalLf`，改为普通 `passed`（`equivalence = None`）：满足硬性要求不等于"依赖可选容忍"。G 报告原示例实测是**字节相同**、走不到所声称的不等路径，故本卡改用**可达**反例（来源无 LF、候选恰好 1 个 LF）核实后处理 | `f_p02_06_require_trailing_lf_never_claims_the_optional_equivalence` |
| F-P02-08 | P1 | **部分处置，仍未关闭**。已收窄为"仅当差异确实只限行结束"才未核验，真正文错误恢复阻断；`Any` 语义按裁定明确为"允许出现某种字节 ≠ 允许两种字节相等"，故 CRLF 候选仍不能通过。**要让 CRLF 文本产物可判等，必须新增一条被声明的等价规则（公共接口变更），本卡按"不新增转换规则"只提提案、不实现** | `f_p02_02_...` 后半 + `r05_an_undeclared_convention_is_never_silently_converted_and_never_a_false_reject` |
| F-P02-05 / 07 / 09 / 10 / 11 / 12 | P1/P2 | **本卡范围外**（表格 parser 零记录、文本/两侧预算、覆盖率与死分支、表头 `trim` 口径、诊断标签启发式）。其中 **F-P02-11 的表述已在本文件更正**（见下） | 见 §9 与交付卡"本卡完成不代表"清单 |

**新增的最小接口提案（未实施，待协调者/用户裁定）**
1. 需要一个新的 reason code 表达"**合同自相矛盾**"（例如 `delivery.check.text.contract_conflict`）。当前以 `TEXT_LINE_ENDING_UNSUPPORTED` + 明确文案替代：语义可辨，但不够专用。
2. 若要让行结束差异可判等，需要新增一条**被声明的**等价（例如 `EofPolicy` / `equivalence` 上的 `lineEndingNormalized`）。这会扩大等价范围，必须明确授权；本卡按边界未实现。

**期望变更记录（保留原样本，不静默修改）**
- **移除** `r04_a_contract_without_sources_is_unverified_not_failed`。其原断言为：`check(&envelope, &[], b"anything")` → `Unverified` / `CONTRACT_UNREADABLE`。该期望是我此前**自行发明、无证据支持**的第三种语义，且偏离旧 checker 对空期望的行为。替代权威：`empty_selection_mirrors_the_old_byte_comparison`。证据：`delivery_content_requirements.rs:284-285`（选择数量被限制在 `1..=64`）、`:313`、`:316`（`selected.len() == count`）、`:626`（真正的"未绑定"是**独立契约变体** `Contract::Unbound` → `Unverified`，不能与空 `Concat` 混同）。原断言的文本与理由已**就地保留在源码注释**中，可审计。
- **F-P02-11 更正**：本模块过滤器总数在 `f61a288` 时为 **47** = `mod.rs` 14 + `text_csv.rs` 30 + 3 个叶子占位；在 `8b4b063`（见 §4.2）后为 **53** = `mod.rs` 14 + `text_csv.rs` **36** + 3 个叶子占位。此前交付单写"42 = 本次文本模块新增"**不准确**，已在此更正。

### 4.2 R1/R2/R3/R6 处置（依据 `G-P02-text-reverify-result.md`，修正提交 `8b4b063`）

| 编号 | 处置 | 证据 |
|---|---|---|
| R1 | **已修**。行结束"无法判定"只可由**授予了至少一条等价**的政策到达（`TextSegments` + `AllowSingleTrailingLf`/`RequireTrailingLf`）；`BytesExact`、以及 `TextSegments` 下的 `ByteExact`/`SourceVerbatim` **自行判定**每个字节差异。修前 `Any`+CRLF 必然先落到 `Unsupported`，把已判定的失败报成 `blocking=false` 的未核验。 | `r1_strict_modes_decide_every_difference`（5 种严格组合 × 4 种候选 = 20 个用例，**全部执行后一次性报告**，首条失败不遮蔽其余）；G 的 `g_v2_strict_modes_fail_on_a_crlf_only_difference` 由失败转绿 |
| R2 | **已修**。冲突只在残差**完全**可由"未决约定 + 至多声明的 EOF 容忍"解释时才报未核验；否则保留**准确的**失败结论（内容/EOF/分隔）及其定位与 reason code，并把冲突说明**追加**到 detail。 | `r2_conflict_plus_a_provable_body_error_still_blocks`、`r2_conflict_plus_allowed_eof_difference_stays_undecided`；G 的 `g_contract_conflict_must_not_swallow_a_provable_body_error` 由失败转绿 |
| R3 | **已修**。归因分侧：`Crlf` 声明被**构造器自身**的 LF 字节违反（任何候选都无法满足），`Lf` 声明只可能被**来源正文**的 CRLF 违反。政策仍约束**整个产物**——构造器字节参与检测、不被豁免。**未改** `expected_concat` 的历史字节、未把标题转成 CRLF、未缩小政策范围；无解的 `Crlf` 组合明确报未核验。 | `r3_a_crlf_declaration_blames_the_constructor_format_not_the_source`、`r3_an_lf_declaration_blames_the_source_body`；G 的 `g_declared_crlf_records_whether_any_candidate_can_pass` 保持绿 |
| R6 | **已按"标注"口径处置**。v1 空集合现有**明确对照**测试（记录旧字节语义，并注明该状态不可达、**不是**业务验收）；v2 空集合被标注为**畸形合同、行为未决**——只断言唯一确定的安全属性（非空候选绝不通过），**不主张**"与 v1 一致"即验收通过，并登记为需要独立接口决定的开放项。 | `v1_empty_selection_documents_the_legacy_byte_comparison`、`v2_empty_selection_is_a_malformed_contract_left_undecided` |

**与 G 观测值的一处行为变化（须记录）**：`Any` + CRLF + 多一个末尾 LF 的候选，修前为 `Failed(source_missing)`，修后为 `Unverified`。理由：该残差 = 未授权的 CRLF↔LF 转换（未决）+ 已声明的单个末尾 LF 容忍（允许），两者都不构成"可独立证明的正文错误"，故未核验比"内容失败"更准确；安全属性（**绝不 Passed**）保持不变，G 自己的断言只要求 `!= Passed`，仍为绿。且修前那种情况报的 `source_missing` 正是 G 记为 R4 的**码不准**问题，此变化顺带移除了一条码不准的路径。

**与 G 的一处分歧（未擅自处理）**：R6 要求"v2 空集合行为单独标注为畸形合同未决项"；而 G 的 overlay 测试 `g_v2_empty_selection_is_recorded_not_prescribed` **断言** v2 空集合 = `Passed`/`Failed`（即把 v1 行为钉在 v2 上，尽管其测试名说"recorded not prescribed"）。本卡**未改行为**（以免破坏独立期望），改在**文档与自有测试**中标注未决。若协调者要求连状态一起改为未核验，需同时裁定 G 的该条测试。

## 5 仍需接线（本轮未做，属 P0-1 / P0-3）

本模块是**叶子**，方案规定的接线只能改共享生产文件。以下三项必须做，且**必须由同一人串行完成**以免双写：

1. `delivery.rs::csv_consistency`（现为 `lines()` + `split(',')`，F02 的源头）改为调用 `parse_table`，使基础检查与内容检查共用同一方言。
2. `delivery_content_requirements.rs::delimited` 改为调用 `parse_table`；`Contract::Concat` 改为经由 `check_text_concat` 并读取 `RuleEnvelope::from_spec`（**旧记录按 v1 解释**）。
3. 检查结果落库时携带 `CheckObservation` 的新字段（`equivalence`、`reason_code`、`locator`、`limit`），并在 `artifact_delivery_view.rs` / 前端区分"执行终态 / 文件可用 / 验证范围"（P0-3，`runtime-artifacts.tsx` 与 `RunReliabilityPanel.tsx`）。

**在这些接线完成前，F01 与 F02 在实际运行中仍然存在。** 本交付只是让修好它们成为可能，不构成"已经修好"。

## 6 独立复核状态与局限（方案 §11.10 要求披露）

- **局限**：本模块的**实现与期望清单由同一个执行者（协调者）产出**，这削弱了方案要求的"独立验收者不复核自己刚实现的部分"。
- **部分缓解**：C0 契约层已由独立微任务 agent 复核（4/4 断言成立，证据见 `worktrees/r16-g-20261010/docs/评审/r16-C0-独立复核.md`），该复核确认了 `CheckLimits` 常量精确相等、三态词表逐字一致、`SecurityVeto` 在类型上不可降级、reason code 零碰撞。
- **仍然需要的**：由**未参与本模块实现**的一方，在本交付的 head SHA 上复跑 `cargo test delivery_checks_v2`，并按 §4 逐条核对标签是否与方案 §8 一致；尤其复核 §3.1 第 4 点（等价只在依赖时记录）与第 6 点（未声明约定 → 未核验）这两处**语义判断**是否被接受。

## 7 执行模式变更（必须记录的事实）

方案 §11.2 的 7 角色 / 3 并发设计在本环境下**未能成立**：

| 实例 | 结果 |
|---|---|
| A（契约与集成） | C0 中途**上下文耗尽失败**，无交接信息；留下未提交骨架（质量可用） |
| A2（收尾） | **上下文耗尽失败**，零改动 |
| G（独立验收） | **上下文耗尽失败**（"ran out of room"），其全部复核结论与期望清单**丢失**，工作树无提交 |
| 微任务复核 agent | **成功**（4/4 断言，逐条落盘） |

**结论**：子 agent 的上下文预算付不起"读文档 + 读代码 + 写长交付"的重型任务；而**极窄、先落盘再汇报**的微任务稳定可用。经用户批准，改为**协调者直接实现，微任务 agent 只做独立核对**。

因此本文件同时承担方案中 B 的交接单与 G 的期望清单两份职责，**并由同一个人完成**——这正是 §6 披露的局限。

## 8 已知缺口与需要决定的事项

**已知缺口（本轮如实标注，未覆盖即未覆盖）**
1. `TABLE_CHARACTER_AFTER_QUOTE` 与 `TABLE_DIALECT_UNSUPPORTED` 有实现路径但**无用例**。
2. 文本等价政策下的 **CRLF 来源 + LF 候选**组合只按"未声明约定 → 未核验"处理；若用户希望此类可自动判等，需要新增声明项（会扩大等价范围，需明确授权）。
3. `QuotePolicy::None`（引号是普通数据）与 `Delimiter::{Semicolon,Pipe}` 已实现但**无用例**。
4. 本模块**未接入生产路径**，实际端到端修复未验证。

**需要用户决定**
| # | 事项 | 建议 |
|---|---|---|
| D1 | §3.1 第 4 点：普通通过是否也标记"合同声明了可选终止 LF 的等价"？ | 建议**保持不标记**（可审计性更强） |
| D2 | §3.1 第 6 点：把 R05 那句话按"有无显式声明"切开（声明被违反=failed，未声明=unverified）是否接受？ | 建议**接受**，否则真实违规会被降级为未核验 |
| D3 | 是否现在就开始 P0-1/P0-3 的**接线**（改 `delivery.rs`、`delivery_content_requirements.rs`、前端）？这是让 F01/F02 真正生效的一步，也是风险最高的一步 | 建议**先接线 + 影子比对**，再动 P1 |
| D4 | `delivery_content_requirements.rs::delimited` 现行的"空白行算记录"与目标方言的 `Skip` 是**行为变化**（旧拒/新过），需要 G 影子比对逐条解释 | 建议保留 `Skip` 并在影子比对中列为已解释差异 |

## 9 证据

| 项 | 值 |
|---|---|
| C0 基点 | `6e0bb5d` |
| 构建/测试命令 | `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml`；`cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml delivery_checks_v2` |
| 共用 target | `CARGO_TARGET_DIR=D:\python\projects\Fox\Fox\apps\desktop\src-tauri\target` |
| 本模块测试（`8b4b063`） | **53 passed / 0 failed**（1575 filtered out）；`text_csv.rs` **36** 项、`mod.rs` 14 项、叶子占位 3 项 |
| 修正提交 | `f61a288`（F-P02 文本路径）、`8b4b063`（R1/R2/R3/R6），均在 `r16/a-contract` |
| 精简 harness 证据（独占 target） | **71 passed / 0 failed** = 模块 53 + G overlay 18（G 的 18 条全部通过，含此前失败的 R1/R2 两条）。harness 位于仓库之外 `D:\python\projects\Fox\a-p02-scratch\a-harness`，独占 target `D:\python\projects\Fox\a-p02-harness-target`。经核验：其五个被测文件与 G 复制件**去 CR 后逐字节一致**（G 的副本是 CRLF，哈希对换行敏感），overlay 由 `G-P02-logs/g-overlay.patch` 原样追加 526 行、未改 patch |
| 正式 crate 状态（E2） | 本轮**未被 E2 阻塞**：`cargo check` 9.74s 通过，且正式 crate 的 `delivery_checks_v2` 过滤测试 **53 passed / 0 failed**，日志含本树 `Compiling fox-engine-protocol / fox-agent-kernel / fox-desktop` 三行。**这不能用于宣称"E2 已排除"**——它是特定窗口下的观察，与 G 窗口的失败并不矛盾（章程 §13 点 5） |
| 修前反例证据 | **6 条反例在修正前 HEAD（`07c2657`）上全部失败**，逐条对应一个缺陷；日志 `A-P02-logs/01b-counterexample-before-fix-recompiled.log` |
| 修正提交 | `f61a288`（`r16/a-contract`，仅 `text_csv.rs`，439 insertions / 140 deletions） |
| 修后证据 | `A-P02-logs/03-after-fix-final.log`，含 `Compiling fox-agent-kernel … (…/r16-a-20261010/crates/…)` 等三行，证明从**本工作树**编译 |
| **测试环境告警（重要，影响证据可信度）** | 共享 `CARGO_TARGET_DIR` 在**多个不同提交的工作树之间发生了产物复用**：本树源码完整（`ModelRetryAdmission` 确实存在于 `crates/fox-agent-kernel/src/controller.rs:38`），但编译一度链接到**非本树**的内核 crate，报 22 个 API 不匹配错误。因此**每次取证都必须记录 `Compiling <pkg> (<本树路径>)` 行**，并且正在被多个提交共享的 target 目录里的日志名不能证明编译身份。**本文件早前记录的 169/17 对照与 42/42 运行是在同一共享目录下取得的，尚未按"强制从本树重编"的纪律复核，不应作为最终证据使用。** |
| `delivery_checks_v2` 相关警告 | **0** |
| 既有交付回归 | 见 §9.1 |
| 未运行 | 真实四题重跑、GUI/安装包验收、影子比对、端到端接线验证 |
| 生产数据 | 全程未读写 `%APPDATA%\com.fox.agent\fox.db`，未触碰用户原始输入与历史结论 |

### 9.1 既有交付回归（含基线对照，**重要发现**）

命令（两个工作树完全相同的过滤器）：`cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml delivery`

| 运行 | 分支 | HEAD | 通过 | 失败 |
|---|---|---|---|---|
| 本交付 | `r16/a-contract` | 本文件提交的 head | **169** | **17** |
| **基线对照** | `r16/g-verification`（G 的工作树，无任何改动） | `0df91625197fbd1fd0d529176e3a941a0d6c36b5` | **127** | **17** |

通过数之差 **169 − 127 = 42**，恰好等于本模块新增的 42 项测试；失败数 **17 = 17 完全相同**。

**结论：这 17 个失败是冻结诊断基线 `0df9162` 自带的既有失败，不是本次变更引入的。** 因此本交付**无回归**。

**对项目的影响（必须写进证据链）**：方案 §9.5 的发布门槛要求"28 项及既有相关安全回归通过"。但**冻结基线本身就有 17 个既有失败**，所以"既有测试全绿"在当前基点上不可达，不能作为门槛的字面条件；必须先由用户决定这 17 项是既有缺陷、环境相关还是时序抖动。

**17 项失败的分类（按证据可判断的部分）**

| 类别 | 数量 | 依据 | 判断 |
|---|---|---|---|
| 时序型活体测试 | 11 | 失败点全在 `delivery_live_tests.rs:32:25`、`kernel_coordinator/tests.rs:2045/1277`，即本地回环 mock 服务器的 `assert!(Instant::now() < deadline, "missing model request")`——**25 秒内没等到内核发起模型请求** | 与并行测试负载高度相关，**倾向环境/时序**，但本轮未做隔离复跑证明 |
| 交付统计断言 | 6 | `delivery.rs:8757`（要求失败原因含"源数据…已变化"）、`delivery.rs:8895`（要求统计项 `state == "passed"`）、`delivery_r3_tests.rs:305/332/350/365`、`delivery_content_requirements.rs:723` | 断言具体业务字符串，**倾向既有逻辑缺陷或口径变化**，需单独定位 |

**本轮限制（不得夸大）**：两次运行**都在 worktree 中**进行，未在主检出 `Fox\Fox` 上复跑，也未在无生产数据的环境上复跑；未逐项隔离复跑以区分"真失败"与"时序抖动"。上述分类是**基于失败点代码的判断**，不是已证实的根因。真实生产数据目录 `%APPDATA%\com.fox.agent` 存在于本机，这些测试是否读取了它**未核实**。
