use crate::database::McpServerRecord;
use crate::mcp::{
    credential_headers, http_client, http_status_error, parse_endpoint, read_limited,
    safe_header_name,
};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use url::Url;

const MAX_TOOLS: usize = 200;
const MAX_DEFINITION_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
struct OpenApiParameter {
    name: String,
    location: String,
    required: bool,
    schema: Value,
}

#[derive(Clone)]
struct OpenApiOperation {
    tool_name: String,
    description: String,
    method: String,
    path: String,
    parameters: Vec<OpenApiParameter>,
    body_schema: Option<Value>,
    body_required: bool,
}

pub(crate) struct OpenApiConnector {
    base_url: Url,
    client: Client,
    credential_headers: HeaderMap,
    operations: Vec<OpenApiOperation>,
}

impl OpenApiConnector {
    pub(crate) fn start(server: &McpServerRecord) -> Result<Self, String> {
        let definition = server
            .definition
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "OpenAPI 扩展源缺少定义".to_owned())?;
        if definition.len() > MAX_DEFINITION_BYTES {
            return Err("OpenAPI 定义超过 2 MB 上限".to_owned());
        }
        let document = parse_document(definition)?;
        let base_url = match server
            .endpoint_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            Some(endpoint) => parse_endpoint(Some(endpoint), "OpenAPI")?,
            None => document
                .get("servers")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("url"))
                .and_then(Value::as_str)
                .map(|value| parse_endpoint(Some(value), "OpenAPI servers[0]"))
                .transpose()?
                .ok_or_else(|| "OpenAPI 未配置基础 URL，且定义中没有 servers[0].url".to_owned())?,
        };
        let operations = operations(&document)?;
        if operations.is_empty() {
            return Err("OpenAPI 定义没有可调用的 operation".to_owned());
        }
        if operations.len() > MAX_TOOLS {
            return Err(format!("OpenAPI operation 数量超过上限 {MAX_TOOLS}"));
        }
        Ok(Self {
            base_url,
            client: http_client()?,
            credential_headers: credential_headers(&server.id)?,
            operations,
        })
    }

    pub(crate) fn list_tools(&self) -> Result<Vec<Value>, String> {
        Ok(self
            .operations
            .iter()
            .map(|operation| {
                let mut properties = Map::new();
                let mut required = Vec::new();
                for parameter in &operation.parameters {
                    properties.insert(parameter.name.clone(), parameter.schema.clone());
                    if parameter.required {
                        required.push(Value::String(parameter.name.clone()));
                    }
                }
                if let Some(schema) = &operation.body_schema {
                    properties.insert("body".to_owned(), schema.clone());
                    if operation.body_required {
                        required.push(Value::String("body".to_owned()));
                    }
                }
                let mut input_schema = json!({ "type": "object", "properties": properties });
                if !required.is_empty() {
                    input_schema["required"] = Value::Array(required);
                }
                json!({
                    "name": operation.tool_name,
                    "description": operation.description,
                    "inputSchema": input_schema,
                })
            })
            .collect())
    }

    pub(crate) fn call_tool(&self, tool: &str, arguments: &Value) -> Result<Value, String> {
        let operation = self
            .operations
            .iter()
            .find(|operation| operation.tool_name == tool)
            .ok_or_else(|| format!("OpenAPI 未声明 operation {tool}"))?;
        let arguments = arguments
            .as_object()
            .ok_or_else(|| "OpenAPI 工具参数必须是对象".to_owned())?;
        let mut path = operation.path.clone();
        let mut query = Vec::new();
        let mut operation_headers = HeaderMap::new();
        for parameter in &operation.parameters {
            let value = arguments.get(&parameter.name);
            if parameter.required && value.is_none() {
                return Err(format!("OpenAPI 参数 {} 为必填项", parameter.name));
            }
            let Some(value) = value else {
                continue;
            };
            let value = scalar_argument(value, &parameter.name)?;
            match parameter.location.as_str() {
                "path" => {
                    path =
                        path.replace(&format!("{{{}}}", parameter.name), &percent_encode(&value));
                }
                "query" => query.push((parameter.name.clone(), value)),
                "header" => {
                    let name = safe_header_name(&parameter.name)?;
                    let value = HeaderValue::from_str(&value).map_err(|error| {
                        format!("OpenAPI header {} 无效: {error}", parameter.name)
                    })?;
                    operation_headers.insert(name, value);
                }
                other => return Err(format!("OpenAPI 暂不支持 {other} 参数")),
            }
        }
        if path.contains('{') || path.contains('}') {
            return Err("OpenAPI 路径仍包含未绑定的模板参数".to_owned());
        }
        let mut endpoint = join_url(&self.base_url, &path)?;
        {
            let mut pairs = endpoint.query_pairs_mut();
            for (name, value) in query {
                pairs.append_pair(&name, &value);
            }
        }
        let method = Method::from_bytes(operation.method.as_bytes())
            .map_err(|error| format!("OpenAPI HTTP 方法无效: {error}"))?;
        let mut request = self
            .client
            .request(method, endpoint)
            .headers(self.credential_headers.clone())
            .headers(operation_headers)
            .header(ACCEPT, "application/json, text/plain;q=0.9, */*;q=0.1");
        if let Some(body) = arguments.get("body") {
            request = request.json(body);
        } else if operation.body_required {
            return Err("OpenAPI 请求体 body 为必填项".to_owned());
        }
        let response = request.send().map_err(|error| error.to_string())?;
        if response.status().is_redirection() {
            return Err("OpenAPI 调用禁止自动重定向；请修正基础 URL".to_owned());
        }
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = read_limited(response)?;
        if !status.is_success() {
            return Err(http_status_error("OpenAPI 调用", status, &bytes));
        }
        let body = if bytes.is_empty() {
            Value::Null
        } else if content_type.to_ascii_lowercase().contains("json") {
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("OpenAPI 响应 JSON 无效: {error}"))?
        } else {
            Value::String(String::from_utf8_lossy(&bytes).into_owned())
        };
        Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()) }],
            "structuredContent": { "status": status.as_u16(), "contentType": content_type, "body": body },
        }))
    }
}

fn parse_document(definition: &str) -> Result<Value, String> {
    let document = match serde_json::from_str::<Value>(definition) {
        Ok(document) => document,
        Err(json_error) => serde_yaml::from_str::<Value>(definition).map_err(|yaml_error| {
            format!("OpenAPI JSON/YAML 解析失败: {json_error}; {yaml_error}")
        })?,
    };
    let version = document
        .get("openapi")
        .and_then(Value::as_str)
        .ok_or_else(|| "定义缺少 openapi 版本".to_owned())?;
    if !version.starts_with("3.") {
        return Err(format!("Fox 当前仅支持 OpenAPI 3.x，收到 {version}"));
    }
    if !document.get("paths").is_some_and(Value::is_object) {
        return Err("OpenAPI 定义缺少 paths 对象".to_owned());
    }
    Ok(document)
}

fn operations(document: &Value) -> Result<Vec<OpenApiOperation>, String> {
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| "OpenAPI paths 无效".to_owned())?;
    let mut operations = Vec::new();
    let mut tool_names = HashSet::new();
    for (path, raw_path_item) in paths {
        let path_item = resolve_reference(document, raw_path_item, 0, &mut HashSet::new())?;
        let Some(path_object) = path_item.as_object() else {
            continue;
        };
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            let Some(operation) = path_object.get(method).and_then(Value::as_object) else {
                continue;
            };
            if operation.get("deprecated").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let base_name = operation
                .get("operationId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(sanitize_tool_name)
                .unwrap_or_else(|| sanitize_tool_name(&format!("{}_{}", method, path)));
            let tool_name = unique_tool_name(base_name, &mut tool_names);
            let description = operation
                .get("description")
                .or_else(|| operation.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let parameters = collect_parameters(
                document,
                path_object.get("parameters"),
                operation.get("parameters"),
            )?;
            let (body_schema, body_required) =
                request_body(document, operation.get("requestBody"))?;
            operations.push(OpenApiOperation {
                tool_name,
                description,
                method: method.to_ascii_uppercase(),
                path: path.clone(),
                parameters,
                body_schema,
                body_required,
            });
        }
    }
    Ok(operations)
}

fn collect_parameters(
    document: &Value,
    path_parameters: Option<&Value>,
    operation_parameters: Option<&Value>,
) -> Result<Vec<OpenApiParameter>, String> {
    let mut parameters: Vec<OpenApiParameter> = Vec::new();
    let mut positions = HashMap::new();
    for raw in [path_parameters, operation_parameters]
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .flatten()
    {
        let parameter = resolve_reference(document, raw, 0, &mut HashSet::new())?;
        let object = parameter
            .as_object()
            .ok_or_else(|| "OpenAPI parameter 必须是对象".to_owned())?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "OpenAPI parameter 缺少 name".to_owned())?
            .to_owned();
        let location = object
            .get("in")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("OpenAPI parameter {name} 缺少 in"))?
            .to_owned();
        if !matches!(location.as_str(), "path" | "query" | "header") {
            continue;
        }
        let required = location == "path"
            || object
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let schema = object
            .get("schema")
            .map(|value| resolve_reference(document, value, 0, &mut HashSet::new()))
            .transpose()?
            .unwrap_or_else(|| json!({ "type": "string" }));
        let parameter = OpenApiParameter {
            name: name.clone(),
            location: location.clone(),
            required,
            schema,
        };
        let key = (name, location);
        if let Some(index) = positions.get(&key).copied() {
            parameters[index] = parameter;
        } else {
            positions.insert(key, parameters.len());
            parameters.push(parameter);
        }
    }
    Ok(parameters)
}

fn request_body(document: &Value, raw: Option<&Value>) -> Result<(Option<Value>, bool), String> {
    let Some(raw) = raw else {
        return Ok((None, false));
    };
    let body = resolve_reference(document, raw, 0, &mut HashSet::new())?;
    let object = body
        .as_object()
        .ok_or_else(|| "OpenAPI requestBody 必须是对象".to_owned())?;
    let required = object
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let content = object
        .get("content")
        .and_then(Value::as_object)
        .ok_or_else(|| "OpenAPI requestBody 缺少 content".to_owned())?;
    let media = content
        .get("application/json")
        .or_else(|| {
            content
                .iter()
                .find(|(name, _)| name.ends_with("+json"))
                .map(|(_, value)| value)
        })
        .ok_or_else(|| "OpenAPI 首版仅支持 JSON requestBody".to_owned())?;
    let schema = media
        .get("schema")
        .map(|value| resolve_reference(document, value, 0, &mut HashSet::new()))
        .transpose()?
        .unwrap_or_else(|| json!({ "type": "object" }));
    Ok((Some(schema), required))
}

fn resolve_reference(
    document: &Value,
    value: &Value,
    depth: usize,
    seen: &mut HashSet<String>,
) -> Result<Value, String> {
    if depth > 64 {
        return Err("OpenAPI $ref 展开深度超过上限".to_owned());
    }
    if let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| "OpenAPI 首版仅支持文档内 $ref".to_owned())?;
        if !seen.insert(reference.to_owned()) {
            return Ok(json!({
                "type": "object",
                "description": format!("recursive reference {reference}")
            }));
        }
        let target = document
            .pointer(pointer)
            .ok_or_else(|| format!("OpenAPI $ref 不存在: {reference}"))?;
        let resolved = resolve_reference(document, target, depth + 1, seen);
        seen.remove(reference);
        return resolved;
    }
    match value {
        Value::Object(object) => object
            .iter()
            .map(|(key, child)| {
                resolve_reference(document, child, depth + 1, seen)
                    .map(|child| (key.clone(), child))
            })
            .collect::<Result<Map<_, _>, _>>()
            .map(Value::Object),
        Value::Array(values) => values
            .iter()
            .map(|child| resolve_reference(document, child, depth + 1, seen))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        value => Ok(value.clone()),
    }
}

fn unique_tool_name(base: String, used: &mut HashSet<String>) -> String {
    let base = if base.is_empty() {
        "operation".to_owned()
    } else {
        base
    };
    if used.insert(base.clone()) {
        return base;
    }
    for suffix in 2..=MAX_TOOLS + 1 {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    base
}

fn sanitize_tool_name(value: &str) -> String {
    let mut name = String::with_capacity(value.len().min(64));
    let mut previous_separator = false;
    for character in value.chars() {
        if name.len() >= 64 {
            break;
        }
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            name.push(character);
            previous_separator = false;
        } else if !previous_separator && !name.is_empty() {
            name.push('_');
            previous_separator = true;
        }
    }
    name.trim_matches('_').to_owned()
}

fn scalar_argument(value: &Value, name: &str) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        _ => Err(format!("OpenAPI 参数 {name} 必须是标量")),
    }
}

fn join_url(base: &Url, path: &str) -> Result<Url, String> {
    Url::parse(&format!(
        "{}/{}",
        base.as_str().trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
    .map_err(|error| format!("OpenAPI 请求 URL 无效: {error}"))
}

fn percent_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn server_with_endpoint(definition: &str, endpoint: &str) -> McpServerRecord {
        McpServerRecord {
            id: "openapi-test".to_owned(),
            name: "OpenAPI Test".to_owned(),
            command: String::new(),
            args: Vec::new(),
            transport: "openapi".to_owned(),
            endpoint_url: Some(endpoint.to_owned()),
            definition: Some(definition.to_owned()),
            enabled: true,
            status: "unknown".to_owned(),
            credential_configured: false,
            last_error: None,
            last_checked_at: None,
            last_latency_ms: None,
            tool_count: None,
            consecutive_failures: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn server(definition: &str) -> McpServerRecord {
        server_with_endpoint(definition, "https://example.com/api")
    }

    #[test]
    fn creates_dynamic_tools_from_yaml_and_resolves_refs() {
        let definition = r#"
openapi: 3.0.3
info: { title: Demo, version: 1.0.0 }
paths:
  /pets/{petId}:
    get:
      operationId: getPet
      description: Read a pet
      parameters:
        - name: petId
          in: path
          required: true
          schema: { $ref: '#/components/schemas/PetId' }
        - name: verbose
          in: query
          schema: { type: boolean }
components:
  schemas:
    PetId: { type: string }
"#;
        let connector = OpenApiConnector::start(&server(definition)).expect("parse OpenAPI");
        let tools = connector.list_tools().expect("list tools");
        assert_eq!(tools[0]["name"], "getPet");
        assert_eq!(
            tools[0]["inputSchema"]["properties"]["petId"]["type"],
            "string"
        );
        assert_eq!(tools[0]["inputSchema"]["required"], json!(["petId"]));
    }

    #[test]
    fn rejects_external_references_and_openapi_two() {
        let external = r#"{
          "openapi":"3.1.0","info":{"title":"x","version":"1"},
          "paths":{"/x":{"post":{"operationId":"x","requestBody":{"$ref":"https://example.com/body.json"}}}}
        }"#;
        assert!(OpenApiConnector::start(&server(external))
            .err()
            .expect("external ref must fail")
            .contains("文档内 $ref"));
        let old = r#"{"openapi":"2.0","paths":{}}"#;
        assert!(OpenApiConnector::start(&server(old))
            .err()
            .expect("OpenAPI 2 must fail")
            .contains("仅支持 OpenAPI 3.x"));
    }

    #[test]
    fn calls_openapi_operation_with_encoded_path_and_query_parameters() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind OpenAPI test server");
        let endpoint = format!("http://{}/base", listener.local_addr().unwrap());
        let server_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept OpenAPI call");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let count = stream.read(&mut buffer).expect("read request");
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            let first_line = request.lines().next().unwrap_or_default().to_owned();
            let body = r#"{"id":"a/b","name":"Fox"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write response");
            first_line
        });
        let definition = r#"
openapi: 3.0.3
info: { title: Demo, version: 1.0.0 }
paths:
  /pets/{petId}:
    get:
      operationId: getPet
      parameters:
        - { name: petId, in: path, required: true, schema: { type: string } }
        - { name: verbose, in: query, schema: { type: boolean } }
"#;
        let connector = OpenApiConnector::start(&server_with_endpoint(definition, &endpoint))
            .expect("create connector");
        let result = connector
            .call_tool("getPet", &json!({ "petId": "a/b", "verbose": true }))
            .expect("call operation");
        assert_eq!(result["structuredContent"]["body"]["name"], "Fox");
        assert_eq!(
            server_thread.join().expect("join OpenAPI server"),
            "GET /base/pets/a%2Fb?verbose=true HTTP/1.1"
        );
    }
}
