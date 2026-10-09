//! Finite content contracts derived by Host from user-authorized source bytes.
//! Material text is data: only content restrictions are extracted, never tools,
//! permissions or instructions about the agent. No model assertion proves one.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SOURCE_LIMIT: u64 = 8 * 1024 * 1024;
const TOTAL_LIMIT: usize = 32 * 1024 * 1024;
const MAX_SOURCES: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
struct Source {
    path: String,
    sha256: String,
    bytes: u64,
    first_line: usize,
    last_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag="type", rename_all="camelCase", deny_unknown_fields)]
enum Contract {
    Unbound { reason: String },
    ForbiddenMention { source: Source, literal: String },
    ForbiddenAvailability { source: Source, literal: String },
    FactQualifiers { sources: Vec<Source> },
    Concat {
        sources: Vec<Source>,
        selected: Vec<String>,
        selection_order: String,
        tie_order: String,
        merge_order: String,
        #[serde(default)]
        read_only: bool,
    },
    DataSource { source: Source, sheet: Option<String>, headers: Vec<String>, date_required: bool, metrics: Vec<Metric> },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct Metric { column:String, operation:String }

pub(super) fn model_reference(spec:&Value)->Value {
    let Ok(contract)=serde_json::from_value::<Contract>(spec["rule"].clone()) else {return json!({"schemaVersion":1,"type":"unbound","reason":"内容契约不支持"});};
    match contract {
        Contract::Concat{sources,selected,selection_order,tie_order,merge_order,read_only}=>json!({"schemaVersion":1,"type":"concat",
            "selectionOrder":selection_order,"tieOrder":tie_order,"mergeOrder":merge_order,"selected":selected,
            "sourceManifestHash":digest(&serde_json::to_vec(&sources).unwrap_or_default()),"readOnly":read_only,"format":"完整文件名 LF 原文，相邻段恰好一个空行，最后不添空行"}),
        Contract::DataSource{source,sheet,headers,date_required,metrics}=>json!({"schemaVersion":1,"rule":{
            "type":"dataSource","source":source,"sheet":sheet,"headers":headers,"date_required":date_required,"metrics":metrics},
            "computeSourceSheet":if Path::new(&source.path).extension().and_then(|e|e.to_str()).is_some_and(|e|matches!(e.to_ascii_lowercase().as_str(),"csv"|"tsv")) {Some("Sheet1")}else{sheet.as_deref()}}),
        other=>json!({"schemaVersion":1,"rule":other}),
    }
}

/// Refuse a write that would overwrite a *Host-frozen read-only merge input*.
///
/// Scope of this gate: only runs that actually froze a `Concat` contract whose
/// inputs the user asked to keep read-only have a protected input set. An
/// ordinary task has none, so the gate must not bound its writes at all — an
/// earlier version resolved the target first and turned any path its folder
/// identity could not fold into a fabricated refusal, which refused every
/// ordinary write in a project whose frozen root carries the Windows
/// extended-length prefix.
pub(super) fn ensure_concat_writable(database:&Database,run:&str,task:Option<&str>,root:&str,target:&Path)->Result<(),String> {
    let current_readonly=readonly_requested(task.unwrap_or(""));
    // Collect the protected identities first: with none, this gate has nothing
    // to say and the target never has to be resolved.
    let mut protected:Vec<String>=Vec::new();
    for item in database.delivery_checklist(run)? {
        for requirement in database.delivery_item_requirements(run,&item.item_key)? {
            let StoredRequirementKind::ContentContract{spec}=requirement.kind else{continue;};
            let contract:Contract=serde_json::from_value(spec["rule"].clone()).map_err(|_|
                crate::tool_host::ToolErrorCode::PermissionDenied.error("冻结内容契约不可读，不能解除输入保护"))?;
            if let Contract::Concat{sources,read_only,..}=contract {
                if !read_only && !current_readonly {continue;}
                protected.extend(sources.into_iter().map(|source|path_key(&source.path)));
            }
        }
    }
    if protected.is_empty() {return Ok(());}
    // A frozen read-only input set exists, so the target identity has to be
    // decided now; a target this project cannot place fails closed with the
    // real reason instead of an opaque policy denial.
    let relative=project_relative_path(target.to_str().ok_or_else(||
        crate::tool_host::ToolErrorCode::PermissionDenied.error("输入保护路径编码不支持"))?,Some(root))
        .ok_or_else(||crate::tool_host::ToolErrorCode::PermissionDenied.error(format!(
            "写入目标不在本次任务冻结的项目目录内，无法确认它是否覆盖只读输入：{}",target.display())))?;
    if protected.contains(&path_key(&relative)) {
        return Err(crate::tool_host::ToolErrorCode::ReadOnlyInput.error(format!("「{relative}」是 Host 冻结的初始合并输入；用户要求原输入保持只读")));
    }
    Ok(())
}
fn readonly_requested(task:&str)->bool {
    ["不要修改原文件","不得修改原文件","不要修改输入","不得修改输入","不能修改原文件","do not modify input"]
        .iter().any(|marker|task.to_ascii_lowercase().contains(marker))
}

fn digest(bytes:&[u8])->String {hex::encode(Sha256::digest(bytes))}
fn source(root:&Path,path:&str)->Result<(Source,Vec<u8>),String> {
    let relative=project_relative_path(path,root.to_str()).ok_or("来源不在授权项目内")?;
    let bytes=super::super::attachment_compute::read_authorized_delivery_bytes(root,&relative,SOURCE_LIMIT)?;
    let reference=Source{path:relative,sha256:digest(&bytes),bytes:bytes.len() as u64,first_line:1,
        last_line:bytes.iter().filter(|b|**b==b'\n').count()+1};
    Ok((reference,bytes))
}
fn frozen_bytes(root:&Path,reference:&Source)->Result<Vec<u8>,String> {
    let (_,bytes)=source(root,&reference.path)?;
    if digest(&bytes)!=reference.sha256 || bytes.len() as u64!=reference.bytes {
        return Err(format!("来源 {} 已偏离冻结哈希，未核验",reference.path));
    }
    Ok(bytes)
}
fn readable(bytes:&[u8],path:&str)->Result<String,String> {
    match Path::new(path).extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "txt"|"md"|"csv"|"tsv"=>String::from_utf8(bytes.to_vec()).map_err(|_|"不是支持的 UTF-8 文本".into()),
        "json"=>{
            let value:Value=serde_json::from_slice(bytes).map_err(|_|"JSON 内容无法解析")?;
            let mut text=String::new();decoded_strings(&value,&mut text)?;Ok(text)
        }
        "docx"=>{
            let entries=crate::local_knowledge_import::zip_entries(bytes)?;
            let body=entries.iter().find(|(name,_)|name=="word/document.xml").ok_or("Word 正文不存在")?;
            let xml=std::str::from_utf8(&body.1).map_err(|_|"Word 正文编码不支持")?;
            Ok(xml_to_text(xml))
        }
        _=>Err("此内容契约尚不支持该文件格式".into()),
    }
}
fn decoded_strings(value:&Value,text:&mut String)->Result<(),String> {
    fn append(text:&mut String,value:&str)->Result<(),String>{
        if text.len().saturating_add(value.len()).saturating_add(1)>MAX_VERIFY_BYTES as usize{return Err("解码文本超过核验预算".into());}
        text.push_str(value);text.push('\n');Ok(())
    }
    match value {
        Value::String(value)=>append(text,value)?,
        Value::Array(values)=>for value in values{decoded_strings(value,text)?;},
        // A content ban includes decoded keys as well as values: changing the
        // JSON spelling cannot hide a prohibited named feature in the output.
        Value::Object(values)=>for (key,value) in values{append(text,key)?;decoded_strings(value,text)?;},
        _=>{},
    }Ok(())
}
fn input_paths(text:&str)->Vec<String> {
    let mentions=collect_mentions(text);
    let roles=classify_mentions(text,&mentions);
    let mut paths=BTreeSet::new();
    for (mention,role) in mentions.iter().zip(roles) {
        if role==MentionRole::Input && !mention_is_example(text,mention.name_start)
            && !is_explanation_request(text,mention.name_start) {paths.insert(mention.name.clone());}
    }
    paths.into_iter().take(17).collect()
}
fn add(seed:&mut DeliveryChecklistSeed,id:&str,contract:Contract,description:&str)->Result<(),String> {
    seed.requirements.push(StoredRequirement{item_key:Some(seed.item_key.clone()),id:format!("content:{id}"),
        source_text:description.into(),kind:StoredRequirementKind::ContentContract{spec:json!({"schemaVersion":1,
            "rule":serde_json::to_value(contract).map_err(|e|e.to_string())?})}});
    Ok(())
}
fn material_rules(reference:&Source,text:&str)->Vec<Contract> {
    // Quoted literals have a definite extent. Bare restrictions are accepted
    // only as short names after explicit content verbs, never arbitrary prose.
    let quoted=regex::Regex::new(r#"(?:禁止提及|不得提及|不要提及|不能提及|不得出现)\s*[“\"'`](?P<literal>[^”\"'`\r\n]{1,80})[”\"'`]|(?:禁止提及|不得提及|不要提及|不能提及)\s*(?P<bare>[^，。；：\s]{1,40})"#).unwrap();
    let availability=regex::Regex::new(r#"(?:不得|禁止|不要)(?:声称|宣称)\s*[“\"'`]?(?P<literal>[^”\"'`，。；\r\n]{1,80}?)[”\"'`]?(?:已经|已)(?:发布|上线|可用)"#).unwrap();
    let english=regex::Regex::new(r#"(?i)(?:do not|don't|must not)\s+mention\s+(?:unreleased\s+|unapproved\s+)?[\"'`]?(?P<literal>[^.\r\n\"'`]{1,80})"#).unwrap();
    let english_availability=regex::Regex::new(r#"(?i)(?:do not|don't|must not)\s+claim\s+[\"'`](?P<literal>[^\"'`\r\n]{1,80})[\"'`]\s+(?:is|has been)\s+(?:released|launched|available)"#).unwrap();
    let mut rules=Vec::new();
    for (index,line) in text.lines().enumerate() {
        let line=line.trim();
        if line.starts_with("例如") || line.starts_with("示例") {continue;}
        let mut location=reference.clone();location.first_line=index+1;location.last_line=index+1;
        for captures in quoted.captures_iter(line) {
            let literal=captures.name("literal").or_else(||captures.name("bare")).unwrap().as_str().trim();
            if literal.contains(['/', '\\', ':']) || ["工具","权限","系统","提示词","指令"].iter().any(|word|literal.contains(word)) {continue;}
            rules.push(Contract::ForbiddenMention{source:location.clone(),literal:literal.into()});
        }
        for captures in availability.captures_iter(line) {
            let literal=captures["literal"].trim().trim_matches(['“','"','\'','`']).to_owned();
            if !literal.is_empty() && !["工具","权限","系统","指令"].iter().any(|word|literal.contains(word)) {
                rules.push(Contract::ForbiddenAvailability{source:location.clone(),literal});
            }
        }
        for captures in english.captures_iter(line) {
            let literal=captures["literal"].trim();
            if !["tool","permission","system","instruction","prompt"].iter().any(|word|literal.to_ascii_lowercase().contains(word)) {
                rules.push(Contract::ForbiddenMention{source:location.clone(),literal:literal.into()});
            }
        }
        for captures in english_availability.captures_iter(line) {
            let literal=captures["literal"].trim();
            if !["tool","permission","system","instruction","prompt"].iter().any(|word|literal.to_ascii_lowercase().contains(word)) {
                rules.push(Contract::ForbiddenAvailability{source:location.clone(),literal:literal.into()});
            }
        }
        if rules.len()>32 {break;}
    }
    let overflow=rules.len()>32;rules.truncate(32);
    if overflow {rules.push(Contract::Unbound{reason:"材料明确约束超过 32 条；其余内容约束未核验".into()});}rules
}

pub(super) fn freeze(database:&Database,run:&str,text:&str,seeds:&mut [DeliveryChecklistSeed])->Result<(),String> {
    if seeds.is_empty(){return Ok(());}
    let root=super::super::attachment_compute::authorized_delivery_root(database,run);
    let inputs=input_paths(text);
    if inputs.len()>16 {for seed in seeds.iter_mut(){add(seed,"budget",Contract::Unbound{reason:"明确来源超过 16 项，有限内容核验未绑定".into()},"内容来源预算")?;}return Ok(());}
    let material_requested=["遵守","按材料","依据","基于","根据","参考","按模板","不能发明","未确认功能"].iter().any(|v|text.contains(v));
    if material_requested && !inputs.is_empty() {
        let mut references=Vec::new();let mut rules=Vec::new();let mut total=0;
        for path in inputs.iter().filter(|path|Path::new(path).extension().and_then(|e|e.to_str()).is_some_and(|e|
            matches!(e.to_ascii_lowercase().as_str(),"txt"|"md"|"docx"|"csv"|"tsv"))) {
            let result=root.as_ref().map_err(|e|e.clone()).and_then(|root|source(root,path))
                .and_then(|(reference,bytes)|{total+=bytes.len();if total>TOTAL_LIMIT{return Err("材料总量超过核验预算".into());}
                    let content=readable(&bytes,path)?;Ok((reference,content))});
            match result {
                Ok((reference,content))=>{if !matches!(Path::new(path).extension().and_then(|e|e.to_str()).map(str::to_ascii_lowercase).as_deref(),Some("csv"|"tsv")) {
                    rules.extend(material_rules(&reference,&content));}references.push(reference);}
                Err(reason)=>rules.push(Contract::Unbound{reason:format!("材料 {path}：{reason}")}),
            }
        }
        let overflow=rules.len()>32;rules.truncate(32);
        if overflow {rules.push(Contract::Unbound{reason:"跨材料明确约束超过 32 条；未覆盖部分未核验".into()});}
        if !references.is_empty(){rules.push(Contract::FactQualifiers{sources:references});}
        for seed in seeds.iter_mut(){for (index,rule) in rules.iter().enumerate(){add(seed,&format!("material:{index}"),rule.clone(),"用户授权材料的明确内容约束与事实限定语来源")?;}}
    }
    if let Some(contract)=freeze_concat(root.as_ref().map_err(|e|e.as_str()),text,seeds) {
        let targets=seeds.iter().filter(|seed|seed.target_path.as_deref().is_some_and(|path|
            matches!(Path::new(path).extension().and_then(|e|e.to_str()),Some("txt"|"md")))).map(|seed|seed.item_key.clone()).collect::<Vec<_>>();
        if targets.len()==1 {if let Some(seed)=seeds.iter_mut().find(|seed|seed.item_key==targets[0]) {add(seed,"concat",contract,"分别冻结大小选择集合、平手规则和文件名合并顺序")?;}}
        else if let Some(seed)=seeds.first_mut(){add(seed,"concat",Contract::Unbound{reason:"无法唯一绑定文本合并产物".into()},"合并顺序未绑定")?;}
    }
    if asks_for_statistics(text) {
        let data_inputs=inputs.iter().filter(|path|Path::new(path).extension().and_then(|e|e.to_str()).is_some_and(|e|
            matches!(e.to_ascii_lowercase().as_str(),"xlsx"|"xlsm"|"csv"|"tsv"))).collect::<Vec<_>>();
        if data_inputs.len()==1 {
            let path=data_inputs[0];
            let result=root.as_ref().map_err(|e|e.clone()).and_then(|root|source(root,path))
                .and_then(|(reference,bytes)|{let sheet=selected_sheet(&bytes,path,source_sheet_from_task(text)?.as_deref())?;
                    let matrix=matrix(&bytes,path,sheet.as_deref())?;
                    let headers=matrix.first().ok_or("源没有表头")?.iter().filter(|name|!name.is_empty()&&text.contains(name.as_str())).cloned().collect::<Vec<_>>();
                    if headers.is_empty(){return Err("任务未明确绑定实际统计字段".into());}
                    let metrics=headers.iter().filter_map(|header| {
                        text.match_indices(header).find_map(|(at,_)| {
                            let following=text[at+header.len()..].split(['。','；','，',',',';','\n']).next().unwrap_or("");
                            let operation=if following.trim_start().starts_with("去重"){Some("distinct_count")}
                                else if following.trim_start().starts_with("合计")||following.contains("作为")&&following.contains("口径"){Some("sum")}else{None};
                            operation.map(|operation|Metric{column:header.clone(),operation:operation.into()})
                        })
                    }).collect::<Vec<_>>();
                    Ok(Contract::DataSource{source:reference,sheet,headers,date_required:text.contains("日销售")||text.contains("日期键")||text.contains("按日期")||text.contains("按日"),metrics})});
            let contract=result.unwrap_or_else(|reason|Contract::Unbound{reason});
            for seed in seeds.iter_mut(){
                if matches!(&contract,Contract::DataSource{metrics,date_required,..} if !metrics.is_empty() || *date_required) {
                    // Replace only this generic unbound source marker with the
                    // real finite source consumer; keep all older semantics.
                    seed.requirements.retain(|r|!matches!(r.kind,StoredRequirementKind::SourceStatsUnbound{..}));
                }
                let mut scoped=contract.clone();
                if let Contract::DataSource{date_required,..}=&mut scoped {
                    let target=seed.target_path.as_deref().unwrap_or("");
                    let extension=Path::new(target).extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase();
                    if !matches!(extension.as_str(),"csv"|"tsv") {
                        // Do not borrow a CSV-only date-key demand for an
                        // unrelated PDF/report. Named or explicit PDF clauses
                        // keep their own daily requirement (e.g. a daily plot).
                        *date_required=*date_required&&text.split(['。','；',';','\n','，',',']).any(|clause| {
                            ((!target.is_empty()&&clause.contains(target))||extension=="pdf"&&clause.to_ascii_lowercase().contains("pdf"))
                                &&["日销售","日期键","按日期","按日"].iter().any(|marker|clause.contains(marker))
                        });
                    }
                }
                add(seed,"data",scoped,"结构化日期、度量及整体占比必须绑定授权源与真实产物哈希")?;
            }
        }
    }
    Ok(())
}

fn freeze_concat(root:Result<&PathBuf,&str>,text:&str,seeds:&[DeliveryChecklistSeed])->Option<Contract> {
    if !text.contains("合并") || !(text.contains("大小")||text.contains("字节")) {return None;}
    let result=(||->Result<Contract,String>{
        if !(text.contains("文件名升序")&&text.contains("空行")&&text.contains("完整文件名")) {return Err("未绑定支持的文件名标题/空行拼接格式".into());}
        if !(text.contains("大小相同按文件名升序")||text.contains("平手按文件名升序")) {return Err("未明确大小平手选择规则".into());}
        let count=regex::Regex::new(r"(?:选出|选取|取前)\s*(\d{1,2})\s*(?:个|份)?").unwrap().captures(text)
            .and_then(|c|c[1].parse::<usize>().ok()).filter(|n|*n>0&&*n<=64).ok_or("选择数量未明确或超过预算")?;
        let ascending=if text.contains("从小到大")||text.contains("最小"){true}else if text.contains("从大到小")||text.contains("最大"){false}else{return Err("大小选择方向未绑定".into());};
        let extension=regex::Regex::new(r"(?i)\.(txt|md)\s*(?:文件|的文件)").unwrap().captures(text).map(|c|c[1].to_lowercase()).ok_or("输入文件类型未明确")?;
        let root=root.map_err(str::to_owned)?;
        // Root scope is explicit. A quoted directory is accepted as a safe
        // relative path; no output directory is borrowed as an input scope.
        let selection_at=["选出","选取","取前"].iter().filter_map(|marker|text.find(marker)).min().ok_or("选择动作未明确")?;
        let selection_clause=sentence_tail(&text[..selection_at]);
        let matcher=regex::Regex::new(r#"在\s*[“\"'`](?P<dir>[^”\"'`\r\n]{1,256})[”\"'`]\s*目录"#).unwrap();
        let directory=if let Some(captures)=matcher.captures(selection_clause) {
            super::super::capability_tools::resolve_project_path(root,&captures["dir"],true)?
        }else if selection_clause.contains("初始目录")||selection_clause.contains("根目录"){root.clone()}
        else{return Err("选择子句的输入目录未明确，不能借用输出目录".into());};
        crate::data_compute::validate_input_path(&directory)?;
        let excluded=seeds.iter().filter_map(|seed|seed.target_path.as_deref()).map(path_key).collect::<BTreeSet<_>>();
        let mut sources=Vec::new();let mut total=0usize;let mut inspected=0;
        for entry in fs::read_dir(&directory).map_err(|e|e.to_string())? {
            inspected+=1;if inspected>MAX_SOURCES{return Err("初始目录超过有限枚举预算".into());}
            let entry=entry.map_err(|e|e.to_string())?;let path=entry.path();
            if path.extension().and_then(|e|e.to_str()).is_none_or(|e|!e.eq_ignore_ascii_case(&extension)){continue;}
            let relative=path.strip_prefix(root).map_err(|_|"输入目录不在项目内")?.to_str().ok_or("输入路径编码不支持")?.replace('\\',"/");
            if excluded.contains(&path_key(&relative)){continue;}
            let (reference,bytes)=source(root,&relative)?;
            let body=std::str::from_utf8(&bytes).map_err(|_|"输入不是 UTF-8，无法确认拼接格式")?;
            if body.contains('\r')||body.ends_with("\n\n"){return Err("源换行格式不能可靠保留并满足恰好一个空行".into());}
            total+=bytes.len();if total>TOTAL_LIMIT{return Err("拼接源总量超过核验预算".into());}
            sources.push(reference);
        }
        if sources.len()<count{return Err("初始输入数量少于要求的选择数量".into());}
        sources.sort_by(|a,b|{let order=a.bytes.cmp(&b.bytes);let order=if ascending{order}else{order.reverse()};
            order.then_with(||file_name(&a.path).cmp(file_name(&b.path)))});
        let mut selected=sources.iter().take(count).map(|source|source.path.clone()).collect::<Vec<_>>();
        selected.sort_by(|a,b|file_name(a).cmp(file_name(b)));
        Ok(Contract::Concat{sources,selected,selection_order:if ascending{"bytes_ascending"}else{"bytes_descending"}.into(),
            tie_order:"filename_ascending".into(),merge_order:"filename_ascending".into(),read_only:readonly_requested(text)})
    })();
    Some(result.unwrap_or_else(|reason|Contract::Unbound{reason}))
}
fn file_name(path:&str)->&str {path.rsplit('/').next().unwrap_or(path)}

fn selected_sheet(bytes:&[u8],path:&str,requested:Option<&str>)->Result<Option<String>,String> {
    if Path::new(path).extension().and_then(|e|e.to_str()).is_some_and(|e|matches!(e.to_ascii_lowercase().as_str(),"csv"|"tsv")) {
        return if requested.is_none(){Ok(None)}else{Err("文本表格不能绑定工作表".into())};
    }
    crate::data_compute::validate_zip_expansion(path,bytes)?;
    let workbook=open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())).map_err(|_|"源工作簿无法打开")?;
    let names=workbook.sheet_names();
    match requested {Some(name) if names.iter().any(|n|n==name)=>Ok(Some(name.into())),Some(_)=>Err("指定工作表不存在".into()),
        None if names.len()==1=>Ok(Some(names[0].clone())),None=>Err("多个工作表且未明确唯一统计表".into())}
}

fn matrix(bytes:&[u8],path:&str,sheet:Option<&str>)->Result<Vec<Vec<String>>,String> {
    let extension=Path::new(path).extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(extension.as_str(),"csv"|"tsv") {
        if sheet.is_some(){return Err("文本表格不能绑定工作表".into());}
        return validate_matrix(delimited(bytes,if extension=="tsv"{'\t'}else{','})?);
    }
    crate::data_compute::validate_zip_expansion(path,bytes)?;
    let workbook=open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())).map_err(|_|"源工作簿无法打开")?;
    let names=workbook.sheet_names().to_vec();
    let name=match sheet {Some(name) if names.iter().any(|n|n==name)=>name.to_owned(),Some(_)=>return Err("指定工作表不存在".into()),
        None if names.len()==1=>names[0].clone(),None=>return Err("多个工作表且未明确唯一统计表".into())};
    validate_matrix(workbook_matrix(bytes,&name)?)
}
pub(super) fn workbook_matrix(bytes:&[u8],sheet:&str)->Result<Vec<Vec<String>>,String> {
    crate::data_compute::validate_zip_expansion("delivery workbook",bytes)?;
    let mut workbook=open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())).map_err(|_|"源工作簿无法打开")?;
    let calamine::Sheets::Xlsx(book)=&mut workbook else{return Err("有限源核验仅支持安全流式 XLSX/XLSM".into());};
    let mut reader=book.worksheet_cells_reader(sheet).map_err(|_|"工作表无法读取")?;
    let mut cells=BTreeMap::new();let mut bounds:Option<(u32,u32,u32,u32)>=None;let mut budget=crate::data_compute::LoadBudget::default();
    while let Some(cell)=reader.next_cell().map_err(|_|"工作表单元格无法读取")? {
        let (row,column)=cell.get_position();let value=CellData::from(cell.get_value().clone());
        if matches!(value,CellData::Empty){continue;}
        let (top,left,bottom,right)=bounds.unwrap_or((row,column,row,column));
        let (top,left,bottom,right)=(top.min(row),left.min(column),bottom.max(row),right.max(column));
        let rows=u64::from(bottom)-u64::from(top)+1;let columns=u64::from(right)-u64::from(left)+1;
        if rows>100_001||columns>128||rows.checked_mul(columns).is_none_or(|extent|extent>200_000){return Err("源单元格跨度超过有限核验预算".into());}
        bounds=Some((top,left,bottom,right));let value=crate::data_compute::cell_to_value(&value,&mut budget)?;
        cells.insert((row,column),match value{Value::String(text)=>text,Value::Null=>String::new(),_=>value.to_string()});
    }
    let Some((top,left,bottom,right))=bounds else{return Ok(Vec::new());};
    let mut matrix=vec![vec![String::new();(right-left+1) as usize];(bottom-top+1) as usize];
    for ((row,column),value) in cells {matrix[(row-top) as usize][(column-left) as usize]=value;}Ok(matrix)
}
fn validate_matrix(rows:Vec<Vec<String>>)->Result<Vec<Vec<String>>,String> {
    let headers=rows.first().ok_or("源表格为空")?;
    if headers.is_empty()||headers.len()>128||headers.iter().any(|h|h.is_empty())
        ||headers.iter().collect::<BTreeSet<_>>().len()!=headers.len(){return Err("真实表头为空或重复，不能唯一绑定".into());}
    if rows.iter().any(|r|r.len()!=headers.len()){return Err("表格列数不一致".into());}Ok(rows)
}

/// RFC-style quoted cells, including embedded newlines. No new CSV dependency
/// and no implicit object/string/date conversion at this read boundary.
fn delimited(bytes:&[u8],delimiter:char)->Result<Vec<Vec<String>>,String> {
    let text=std::str::from_utf8(bytes).map_err(|_|"表格不是 UTF-8")?.trim_start_matches('\u{feff}');
    let mut chars=text.chars().peekable();let mut rows=Vec::new();let mut row=Vec::new();let mut cell=String::new();
    let mut quoted=false;let mut closed=false;
    while let Some(character)=chars.next() {
        if quoted {
            if character=='"' {if chars.peek()==Some(&'"'){chars.next();cell.push('"');}else{quoted=false;closed=true;}}
            else {cell.push(character);}continue;
        }
        if closed && character!=delimiter && !matches!(character,'\r'|'\n'){return Err("引号单元格后的字符不支持".into());}
        if character=='"' {if !cell.is_empty(){return Err("单元格内引号未转义".into());}quoted=true;}
        else if character==delimiter {row.push(std::mem::take(&mut cell));closed=false;if row.len()>128{return Err("列数超过核验预算".into());}}
        else if matches!(character,'\r'|'\n') {
            if character=='\r'&&chars.peek()==Some(&'\n'){chars.next();}
            row.push(std::mem::take(&mut cell));rows.push(std::mem::take(&mut row));closed=false;
            if rows.len()>100_001{return Err("行数超过核验预算".into());}
        }else{cell.push(character);}
    }
    if quoted{return Err("单元格引号未闭合".into());}
    if !cell.is_empty()||!row.is_empty()||closed {row.push(cell);rows.push(row);}Ok(rows)
}

fn numeric(value:&str)->Result<f64,String> {
    let value=value.parse::<f64>().map_err(|_|"度量包含非数值或空值")?;
    if !value.is_finite(){return Err("度量不是有限数值".into());}Ok(value)
}
fn close(a:f64,b:f64)->bool {a.is_finite()&&b.is_finite()&&(a-b).abs()<=a.abs().max(b.abs())*1e-9}

/// Read only settled Host results in this Run's legitimate continuation chain.
/// The real published artifact identity and byte hash must agree. A model's
/// sourceVerified field is deliberately ignored.
fn compute_contract(database:&Database,run:&str,hash:&str,finding:&Value)->Result<(Value,Value),String> {
    use rusqlite::OptionalExtension;
    let mut artifact_id=finding["artifactId"].as_str().map(str::to_owned);
    if artifact_id.is_none() {
        if let (Some(owner),Some(call))=(finding["sourceRunId"].as_str(),finding["sourceToolCallId"].as_str()) {
            let raw=database.with_connection(|connection|connection.query_row(
                "WITH RECURSIVE chain(run_id,depth) AS (SELECT ?1,0 UNION ALL SELECT k.continued_from_run_id,chain.depth+1 FROM kernel_runs k JOIN chain ON k.run_id=chain.run_id WHERE k.continued_from_run_id IS NOT NULL AND chain.depth<100)
                 SELECT t.canonical_input_json FROM kernel_tool_calls t JOIN chain ON chain.run_id=t.run_id JOIN runs r ON r.id=t.run_id
                 WHERE t.run_id=?2 AND t.tool_call_id=?3 AND t.tool='write_file' AND t.state='completed'
                 AND r.conversation_id=(SELECT conversation_id FROM runs WHERE id=?1) AND length(t.canonical_input_json)<=65536",
                rusqlite::params![run,owner,call],|row|row.get::<_,String>(0)).optional())?;
            artifact_id=raw.and_then(|raw|serde_json::from_str::<Value>(&raw).ok()).and_then(|input|input["artifactId"].as_str().map(str::to_owned));
        }
    }
    let artifact_id=artifact_id.ok_or("缺少实际发布的 compute artifactId，数据契约未核验")?;
    let records=database.with_connection(|connection|{
        let mut statement=connection.prepare("WITH RECURSIVE chain(run_id,depth) AS (SELECT ?1,0 UNION ALL SELECT k.continued_from_run_id,chain.depth+1 FROM kernel_runs k JOIN chain ON k.run_id=chain.run_id WHERE k.continued_from_run_id IS NOT NULL AND chain.depth<100)
            SELECT t.result_json FROM kernel_tool_calls t JOIN chain ON chain.run_id=t.run_id JOIN runs r ON r.id=t.run_id
            WHERE t.tool='attachment_compute' AND t.state='completed' AND r.conversation_id=(SELECT conversation_id FROM runs WHERE id=?1)
            AND length(t.result_json)<=2097152 ORDER BY t.settled_at DESC LIMIT 129")?;
        let records=statement.query_map([run],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;Ok(records)
    })?;
    if records.len()>128{return Err("计算契约查询超过有限窗口，未核验".into());}
    for raw in records {
        let Ok(value)=serde_json::from_str::<Value>(&raw) else{continue;};
        let details=if value["details"].is_object(){&value["details"]}else{&value};
        if details["computedBy"]!="fox-quickjs"{continue;}
        if let Some(file)=details["files"].as_array().and_then(|files|files.iter().find(|file|file["id"]==artifact_id)) {
            if file["sha256"].as_str()!=Some(hash)||file["renderedHash"].as_str()!=Some(hash)
                ||file["dataContract"]["renderedHash"].as_str()!=Some(hash){return Err("数据契约与真实发布字节哈希不一致".into());}
            let contract=&file["dataContract"];
            if contract["schemaVersion"]!=1{return Err("数据契约版本不支持".into());}
            return Ok((contract.clone(),details["inputSources"].clone()));
        }
    }
    Err("真实发布文件没有同 Run 链的 Host 结构化导出契约，未核验".into())
}
fn matched_source<'a>(input_sources:&Value,reference:&Source,declared:&'a Value)->Result<&'a Value,String> {
    let id=declared["inputId"].as_str().ok_or("数据契约没有 inputId")?;
    if input_sources.as_array().is_none_or(|sources|!sources.iter().any(|source|source["id"]==id
        &&source["kind"]=="project_snapshot"&&source["projectPath"]==reference.path&&source["sha256"]==reference.sha256
        &&source["bytes"].as_u64()==Some(reference.bytes))){return Err("声明来源未绑定本次授权项目快照路径/哈希/字节数".into());}
    Ok(declared)
}
fn matching_compute_sheet(reference:&Source,physical_sheet:Option<&str>,declared:Option<&str>)->bool {
    if Path::new(&reference.path).extension().and_then(|e|e.to_str()).is_some_and(|e|matches!(e.to_ascii_lowercase().as_str(),"csv"|"tsv")) {
        // Both whole and chunked loaders expose the actual CSV/TSV payload as
        // Sheet1. It is a logical table name, never a physical worksheet.
        physical_sheet.is_none()&&declared==Some("Sheet1")
    }else{physical_sheet.is_some()&&declared==physical_sheet}
}
#[allow(clippy::too_many_arguments)]
fn evaluate_data(root:&Path,reference:&Source,sheet:Option<&str>,headers:&[String],date_required:bool,metrics:&[Metric],bytes:&[u8],path:&str,
    database:&Database,run:&str,finding:&Value)->Result<RequirementVerdict,String> {
    let source_bytes=frozen_bytes(root,reference)?;
    let source_rows=matrix(&source_bytes,&reference.path,sheet)?;
    let source_headers=source_rows.first().ok_or("源表头缺失")?;
    let extension=Path::new(path).extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase();
    let actual_rows=if matches!(extension.as_str(),"csv"|"tsv"){Some(matrix(bytes,path,None)?)}else{None};
    // This guard is tied to an explicit date/metric demand. A literal object
    // citation in arbitrary text or in a table's note column remains legal.
    if date_required {
        if let Some(rows)=&actual_rows {
            let columns=&rows[0];
            for row in rows.iter().skip(1) {for (index,column) in columns.iter().enumerate() {
                let lower=column.to_ascii_lowercase();
                let date_column=lower.contains("date")||lower=="day"||column.contains("日期")
                    ||lower=="key"&&row.first().is_some_and(|metric|metric.contains("day")||metric.contains("daily")||metric.contains('日'));
                if date_column && !row[index].is_empty() && crate::data_compute::normalize_date_key(&json!(row[index])).is_err() {
                    return Ok(RequirementVerdict::Unmet(format!("明确日期键列「{column}」包含不支持的日期/对象字符串；请使用源日期的 dateKey 与 saveTable")));
                }
            }}
        }
    }
    if let Some(rows)=&actual_rows {
        if rows[0].iter().map(String::as_str).eq(["metric","key","value"]) && !metrics.is_empty() {
            return evaluate_metrics(&source_rows,rows,metrics,date_required);
        }
    }
    let (contract,input_sources)=compute_contract(database,run,&digest(bytes),finding)?;
    if contract["kind"]=="table" {
        let declared=matched_source(&input_sources,reference,&contract["source"])?;
        if !matching_compute_sheet(reference,sheet,declared["sheet"].as_str()) {
            return Err("表格 source.sheet 与冻结工作表不一致".into());
        }
        let actual=actual_rows.ok_or("结构化表格产物格式不支持")?;
        let mapping=declared["columnMapping"].as_array().filter(|mapping|!mapping.is_empty()).ok_or("表格列映射缺失，不能独立确认源度量")?;
        if actual[0]!=contract["columns"].as_array().ok_or("导出列契约缺失")?.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>()
            ||actual.len().saturating_sub(1) as u64!=contract["rowCount"].as_u64().ok_or("导出行数契约缺失")? {return Ok(RequirementVerdict::Unmet("真实 CSV 与导出列/行数契约不同".into()));}
        if mapping.len()!=actual[0].len(){return Err("当前仅支持每一列都有明确源映射的直接投影；聚合表需明确度量契约，未核验".into());}
        if actual.len()!=source_rows.len(){return Err("当前仅支持保留记录的直接投影；汇总/筛选行来源未核验".into());}
        for entry in mapping {
            let output=entry["outputColumn"].as_str().ok_or("输出列映射不完整")?;let input=entry["inputColumn"].as_str().ok_or("源列映射不完整")?;
            let output_index=actual[0].iter().position(|h|h==output).ok_or("映射输出列不存在")?;
            let input_index=source_headers.iter().position(|h|h==input).ok_or("映射源字段不存在")?;
            if !headers.iter().any(|h|h==input){return Err("映射字段不属于用户明确统计字段".into());}
            let dates=contract["dateColumns"].as_array().is_some_and(|dates|dates.iter().any(|date|date==output));
            for (actual,source) in actual.iter().skip(1).zip(source_rows.iter().skip(1)) {
                let expected=if dates&&!source[input_index].is_empty(){crate::data_compute::normalize_date_key(&json!(source[input_index]))?}else{source[input_index].clone()};
                if actual[output_index]!=expected{return Ok(RequirementVerdict::Unmet(format!("导出列 {output} 与授权源字段 {input} 的实际记录不一致")));}
            }
        }
        if !metrics.is_empty()||date_required {
            return Ok(RequirementVerdict::Unverified(format!("真实表格的源列/记录直接投影已核对；{}{}未由明细投影证明，需要独立度量结果",
                if date_required{"该产物的日度量/日期要求；"}else{""},metrics.iter().map(|metric|format!("{}:{}",metric.operation,metric.column)).collect::<Vec<_>>().join("、"))));
        }
        return Ok(RequirementVerdict::Met);
    }
    let charts=contract["charts"].as_array().filter(|charts|!charts.is_empty()).ok_or("缺少受支持的源绑定图表契约")?;
    let mut checked=0;let mut checked_totals=BTreeSet::new();
    for chart in charts {
        let basis=&chart["shareBasis"];
        if basis["scope"]!="overall"{continue;}
        let declared=matched_source(&input_sources,reference,&basis["source"])?;
        if !matching_compute_sheet(reference,sheet,declared["sheet"].as_str()) {return Err("图表源工作表/逻辑表未绑定冻结来源".into());}
        let column=declared["totalColumn"].as_str().ok_or("整体占比缺少真实度量列")?;
        if !headers.iter().any(|h|h==column){return Err("整体占比度量列不属于用户明确字段".into());}
        let index=source_headers.iter().position(|h|h==column).ok_or("整体度量字段不存在")?;
        let mut total=0.0;for row in source_rows.iter().skip(1){let value=numeric(&row[index])?;if value<0.0{return Err("源整体份额度量为负，口径不支持".into());}total+=value;}
        if !total.is_finite()||total<=0.0{return Err("授权源整体分母不是有限正数".into());}
        if basis["total"].as_f64().is_none_or(|declared|!close(total,declared))
            ||chart["renderedTotal"].as_f64().is_none_or(|rendered|!close(total,rendered)) {
            return Ok(RequirementVerdict::Unmet("整体占比的声明/实际渲染分母与授权源独立统计不一致".into()));
        }
        let label_column=declared["labelColumn"].as_str().ok_or("整体图表没有真实分类映射，子集度量未核验")?;
        if !headers.iter().any(|h|h==label_column){return Err("图表分类不属于用户明确字段".into());}
        let label_index=source_headers.iter().position(|h|h==label_column).ok_or("图表分类字段不存在")?;
        let mut values=BTreeMap::<String,f64>::new();for row in source_rows.iter().skip(1){*values.entry(row[label_index].clone()).or_default()+=numeric(&row[index])?;}
        let labels=chart["labels"].as_array().ok_or("图表原分类未记录")?;
        let series=chart["series"].as_array().filter(|series|series.len()==1).ok_or("当前整体占比仅支持一个度量序列")?;
        let data=series[0]["values"].as_array().ok_or("图表原数据未记录")?;
        if labels.len()!=data.len(){return Err("图表分类/度量长度不一致".into());}
        let mut seen=BTreeSet::new();for (label,value) in labels.iter().zip(data) {
            let label=label.as_str().ok_or("图表分类不是标量文本")?;
            if !seen.insert(label){return Ok(RequirementVerdict::Unmet("整体占比子集分类重复".into()));}
            if value.as_f64().zip(values.get(label).copied()).is_none_or(|(actual,expected)|!close(actual,expected)) {
                return Ok(RequirementVerdict::Unmet(format!("整体占比「{label}」的子集值与授权源独立汇总不一致")));
            }
        }
        checked+=1;checked_totals.insert(column.to_owned());
    }
    if checked==0{return Ok(RequirementVerdict::Unverified("图表仅记录普通/子集占比，整体分母及跨产物口径未核验；不改变普通饼图默认语义".into()));}
    let uncovered=metrics.iter().filter(|metric|metric.operation!="sum"||!checked_totals.contains(&metric.column))
        .map(|metric|format!("{}:{}",metric.operation,metric.column)).collect::<Vec<_>>();
    if date_required||!uncovered.is_empty() {
        return Ok(RequirementVerdict::Unverified(format!("已独立核对 {checked} 个整体 pie 的源分母/分类；{}{}仍未核验，不将该产物完整源语义标为通过",
            if date_required{"该产物明确要求的日度量/日期图；"}else{""},uncovered.join("、"))));
    }
    Ok(RequirementVerdict::Met)
}

fn evaluate_metrics(source:&[Vec<String>],actual:&[Vec<String>],metrics:&[Metric],date_required:bool)->Result<RequirementVerdict,String> {
    let mut expected=BTreeMap::<String,f64>::new();
    for metric in metrics {
        let index=source[0].iter().position(|header|header==&metric.column).ok_or("冻结度量字段不存在")?;
        let value=match metric.operation.as_str() {
            "sum"=>{let mut total=0.0;for row in source.iter().skip(1){total+=numeric(&row[index])?;}if !total.is_finite(){return Err("源合计超过有限数值范围".into());}total},
            "distinct_count"=>{let values=source.iter().skip(1).map(|row|row[index].as_str()).collect::<BTreeSet<_>>();
                if values.contains(""){return Err("去重键含空值，统计口径未确认".into());}values.len() as f64},
            _=>return Err("冻结度量运算不支持".into()),
        };
        expected.insert(format!("{}:{}",metric.operation,metric.column),value);
    }
    let mut seen=BTreeSet::new();let mut daily_checked=false;let mut daily_days=BTreeMap::<String,BTreeSet<String>>::new();
    for row in actual.iter().skip(1) {
        let id=&row[0];let key=&row[1];let value=numeric(&row[2])?;
        if !seen.insert((id.clone(),key.clone())){return Ok(RequirementVerdict::Unmet("度量导出包含重复 metric/key".into()));}
        if let Some(expected)=expected.get(id) {
            if !matches!(key.as_str(),""|"all"){return Err("总量 key 不是空或 all，范围未绑定".into());}
            if !close(*expected,value){return Ok(RequirementVerdict::Unmet(format!("度量 {id} 与授权源独立计算不一致")));}
        } else if let Some(column)=id.strip_prefix("daily_sum:") {
            if !date_required||!metrics.iter().any(|metric|metric.operation=="sum"&&metric.column==column){return Err("日度量没有冻结的日期/合计要求".into());}
            let date_columns=(0..source[0].len()).filter(|index|source.iter().skip(1).any(|row|!row[*index].is_empty())
                &&source.iter().skip(1).all(|row|!row[*index].is_empty()&&crate::data_compute::normalize_date_key(&json!(row[*index])).is_ok())).collect::<Vec<_>>();
            if date_columns.len()!=1{return Err("源日期列不唯一或类型不确定，日度量未核验".into());}
            let index=source[0].iter().position(|header|header==column).ok_or("日度量字段不存在")?;
            let day=crate::data_compute::normalize_date_key(&json!(key))?;
            let mut expected=0.0;let mut found=false;
            for source_row in source.iter().skip(1) {if crate::data_compute::normalize_date_key(&json!(source_row[date_columns[0]]))?==day {
                found=true;expected+=numeric(&source_row[index])?;
            }}
            if !found||!close(expected,value){return Ok(RequirementVerdict::Unmet(format!("日度量 {id}/{day} 与授权源不一致")));}
            if !daily_days.entry(id.clone()).or_default().insert(day) {
                return Ok(RequirementVerdict::Unmet(format!("日度量 {id} 包含归一到同一天的重复日期键")));
            }
            daily_checked=true;
        }else{return Err(format!("度量 {id} 没有有限源运算契约；当前支持 sum:字段、distinct_count:字段、daily_sum:字段"));}
    }
    if expected.keys().any(|id|!seen.iter().any(|(seen_id,key)|seen_id==id&&matches!(key.as_str(),""|"all"))) {
        return Ok(RequirementVerdict::Unmet("度量导出缺少冻结的源合计/去重统计项".into()));
    }
    for (id,days) in daily_days {
        let date_columns=(0..source[0].len()).filter(|index|source.iter().skip(1).any(|row|!row[*index].is_empty())
            &&source.iter().skip(1).all(|row|crate::data_compute::normalize_date_key(&json!(row[*index])).is_ok())).collect::<Vec<_>>();
        if date_columns.len()!=1{return Err("源日期列无法唯一绑定".into());}
        let expected_days=source.iter().skip(1).map(|row|crate::data_compute::normalize_date_key(&json!(row[date_columns[0]])))
            .collect::<Result<BTreeSet<_>,_>>()?;
        if days!=expected_days {return Ok(RequirementVerdict::Unmet(format!("日度量 {id} 缺少源日期或包含多余日期")));}
    }
    Ok(if date_required&&!daily_checked{RequirementVerdict::Unverified("源总量已核对，明确要求的日度量尚未提供，日期口径未核验".into())}
        else{RequirementVerdict::Met})
}

fn availability_claim(line:&str,literal:&str)->bool {
    let positive=regex::Regex::new(r#"(?i)^\s*[”\"'`]?\s*(?:功能\s*)?(?:已经发布|已发布|已经上线|已上线|现已可用|已可用|is\s+released|has\s+been\s+released|is\s+launched|is\s+available)"#).unwrap();
    line.split(['，',',','。','；',';','\n']).any(|clause|clause.match_indices(literal).any(|(at,_)| {
        !qualifier_negated(clause,at) && positive.is_match(&clause[at+literal.len()..])
    }))
}
fn qualifier_negated(line:&str,at:usize)->bool {
    let prefix=line[..at].chars().rev().take(8).collect::<String>().chars().rev().collect::<String>();
    ["并非","不是","尚未","没有","未","不得声称","不能声称","不得宣称","不能宣称","假设","目标是"]
        .iter().any(|marker|prefix.trim_end().ends_with(marker))
}
fn evaluate(contract:&Contract,root:&Path,bytes:&[u8],path:&str,database:&Database,run:&str,finding:&Value)->RequirementVerdict {
    let result=(||->Result<RequirementVerdict,String>{
        match contract {
            Contract::Unbound{reason}=>Ok(RequirementVerdict::Unverified(reason.clone())),
            Contract::ForbiddenMention{source,literal}=>{frozen_bytes(root,source)?;let text=readable(bytes,path)?;
                Ok(if text.to_lowercase().contains(&literal.to_lowercase()){RequirementVerdict::Unmet(format!("材料 {} 第 {} 行禁止提及「{literal}」；否定句中的提及也不满足",source.path,source.first_line))}else{RequirementVerdict::Met})},
            Contract::ForbiddenAvailability{source,literal}=>{frozen_bytes(root,source)?;let text=readable(bytes,path)?;
                Ok(if text.split(['。','；','\n']).any(|line|availability_claim(line,literal)) {
                    RequirementVerdict::Unmet(format!("材料 {} 第 {} 行不允许声称「{literal}」已发布/上线/可用",source.path,source.first_line))}else{RequirementVerdict::Met})},
            Contract::FactQualifiers{sources}=>{
                let mut evidence=String::new();for source in sources {evidence.push_str(&readable(&frozen_bytes(root,source)?,&source.path)?);evidence.push('\n');}
                let text=readable(bytes,path)?;
                for marker in ["上线后","实测","已经上线","已上线","已验证","人民币","美元","欧元","CNY","USD","EUR","¥","$"] {
                    for line in text.split(['。','；','\n']).filter(|line|line.contains(marker)) {
                        if line.match_indices(marker).all(|(at,_)|qualifier_negated(line,at)){continue;}
                        // An isolated matching keyword is not evidence of the
                        // attributed fact. Finite support requires this clause.
                        if !evidence.contains(line.trim()) {return Ok(RequirementVerdict::Unverified(format!("限定语「{marker}」的完整陈述未在冻结材料中找到依据；请给出来源或改为真实未知状态")));}
                    }
                }
                Ok(RequirementVerdict::Met)
            }
            Contract::Concat{sources,selected,..}=>{
                let mut contents=BTreeMap::new();for source in sources {contents.insert(source.path.clone(),frozen_bytes(root,source)?);}
                let mut expected=Vec::new();for (index,name) in selected.iter().enumerate() {
                    let body=contents.get(name).ok_or("冻结选择引用缺失")?;
                    expected.extend_from_slice(file_name(name).as_bytes());expected.push(b'\n');expected.extend_from_slice(body);
                    if index+1<selected.len(){if !body.ends_with(b"\n"){expected.push(b'\n');}expected.push(b'\n');}
                }
                Ok(if bytes==expected {RequirementVerdict::Met}else{RequirementVerdict::Unmet(format!("真实合并字节不符合冻结的文件名升序/完整内容/单空行格式；要求顺序：{}",selected.iter().map(|p|file_name(p)).collect::<Vec<_>>().join("、")))})
            }
            Contract::DataSource{source,sheet,headers,date_required,metrics}=>evaluate_data(root,source,sheet.as_deref(),headers,*date_required,metrics,bytes,path,database,run,finding),
        }
    })();
    result.unwrap_or_else(RequirementVerdict::Unverified)
}

pub(super) fn apply(database:&Database,run:&str,checklist:&[DeliveryChecklistItem],verdicts:&mut [ItemVerdict])->Result<(),String> {
    let root=super::super::attachment_compute::authorized_delivery_root(database,run);
    for (item,verdict) in checklist.iter().zip(verdicts.iter_mut()) {
        let requirements=database.delivery_item_requirements(run,&item.item_key)?;
        let contracts=requirements.iter().filter_map(|r|match &r.kind{StoredRequirementKind::ContentContract{spec}=>Some((r,spec)),_=>None}).collect::<Vec<_>>();
        if contracts.is_empty(){continue;}
        let mut finding:Value=serde_json::from_str(&verdict.finding_json).map_err(|_|"持久交付 finding 不是有效 JSON")?;
        let mut outcomes=finding["requirements"].as_array().cloned().unwrap_or_default();
        for (requirement,spec) in contracts {
            let result=(||->Result<RequirementVerdict,String>{
                if spec["schemaVersion"]!=1{return Err("内容契约版本不支持".into());}
                let contract:Contract=serde_json::from_value(spec["rule"].clone()).map_err(|_|"内容契约字段不完整或不支持")?;
                let root=root.as_ref().map_err(|e|e.clone())?;
                let path=verdict.bound_path.as_ref().ok_or("内容核验没有绑定真实产物")?;
                let relative=project_relative_path(path.to_str().ok_or("产物路径编码不支持")?,root.to_str()).ok_or("产物不在当前授权项目内")?;
                let bytes=super::super::attachment_compute::read_authorized_delivery_bytes(root,&relative,MAX_VERIFY_BYTES)?;
                if finding["checkedHash"].as_str().is_none_or(|hash|hash.trim_start_matches("sha256:")!=digest(&bytes)) {
                    return Err("产物已偏离本次真实回执/版本核验哈希".into());
                }
                let result=evaluate(&contract,root,&bytes,&relative,database,run,&finding);
                if super::super::attachment_compute::authorized_delivery_root(database,run).as_ref().ok()!=Some(root) {
                    return Err("内容核验期间项目授权改变，未核验".into());
                }
                Ok(result)
            })().unwrap_or_else(RequirementVerdict::Unverified);
            let (state,reason)=match result {RequirementVerdict::Met=>("passed",None),RequirementVerdict::Unmet(reason)=>("failed",Some(reason)),
                RequirementVerdict::Unverified(reason)=>("unverified",Some(reason))};
            let source=spec["rule"].get("source").or_else(||spec["rule"]["sources"].as_array().and_then(|sources|sources.first()));
            let entry=json!({"id":requirement.id,"check":"content_contract","state":state,"reason":reason,
                "sourceText":requirement.source_text,"sourcePath":source.and_then(|source|source["path"].as_str()),
                "sourceHash":source.and_then(|source|source["sha256"].as_str())});
            finding["checks"][requirement.id.as_str()]=entry.clone();outcomes.push(entry);
            if state=="failed" {verdict.passed=false;finding["reasonCode"]=json!("content_contract_mismatch");finding["reason"]=json!(reason);}
        }
        let failed=outcomes.iter().any(|o|o["state"]=="failed");let unknown=outcomes.iter().any(|o|o["state"]=="unverified");
        finding["semanticStatus"]=json!(if failed{"unmet"}else if unknown{"unverified"}else{"met"});
        finding["verificationStatus"]=json!(if failed{"failed"}else if unknown{"unverified"}else{"verified"});
        finding["requirements"]=json!(outcomes);verdict.finding_json=finding.to_string();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference()->Source{Source{path:"policy.md".into(),sha256:"a".repeat(64),bytes:1,first_line:1,last_line:1}}
    #[test]
    fn finite_material_budget_and_subject_local_negation() {
        let body=(0..33).map(|i|format!("禁止提及“Feature {i}”。")).collect::<Vec<_>>().join("\n");
        let rules=material_rules(&reference(),&body);
        assert_eq!(rules.iter().filter(|r|matches!(r,Contract::ForbiddenMention{..})).count(),32);
        assert!(rules.iter().any(|r|matches!(r,Contract::Unbound{..})));
        assert!(availability_claim("Guide 已发布，Other 未上线。","Guide"));
        assert!(!availability_claim("Guide 尚未发布。","Guide"));
        assert!(material_rules(&reference(),"Do not claim 'Smart Guide' is released.").iter().any(|r|matches!(r,Contract::ForbiddenAvailability{..})));
        let line="这是产品上线后已经跑出来的数，不是目标值。";
        assert!(!qualifier_negated(line,line.find("上线后").unwrap()));
        assert!(readable(br#"{"title":"\u0048idden Preview"}"#,"brief.json").unwrap().contains("Hidden Preview"));
    }
    #[test]
    fn explicit_sheet_and_multiple_source_remain_ambiguous() {
        assert!(source_sheet_from_task("按字段统计 Sheet 'Alpha' 和 Sheet 'Beta'。").is_err());
        assert_eq!(source_sheet_from_task("以 Detailed Ledger 为事实明细，按 Segment Group 列统计。").unwrap(),Some("Detailed Ledger".into()));
        let requirement=source_statistics_demand("读取 FIRST.XLSX 和 SECOND.XLSX，按 Segment Group 列统计，生成 report.md。").unwrap();
        assert!(matches!(requirement.kind,RequirementKind::SourceStatsUnbound{..}));
    }
}
