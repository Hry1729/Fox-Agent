//! Dedicated, bounded connections for Kernel effects. Never invalidate/kill a
//! pooled connection that another Run may still own.
use super::*;
use crate::kernel::CancellationToken;
use std::time::Instant;

pub(crate) fn execute_owned(server: &McpServerRecord, call: Option<(&str,&Value)>, token: &CancellationToken,
    budget: Duration) -> Result<Value,String> {
    token.check()?;
    if !server.enabled || server.id == crate::office::SERVER_ID { return Err("MCP connection requires a dedicated authorized adapter".into()); }
    let deadline = Instant::now() + budget;
    let mut stdio = None;
    let mut http = None;
    let mut openapi = None;
    match server.transport.as_str() {
        "stdio" => stdio = Some(OwnedStdio::start(server)?),
        "streamable_http" => {
            let mut session = HttpMcpSession::start(server)?;
            session.execution_deadline = Some(deadline);
            session.cancellation = Some(token.clone());
            session.ensure_initialized()?;
            http = Some(session);
        }
        "openapi" => openapi = Some(crate::mcp_openapi::OpenApiConnector::start_bounded(server, deadline, token.clone())?),
        _ => return Err("unsupported frozen MCP transport".into()),
    }
    let tools = if let Some(session) = &mut stdio {
        let initialized = session.request("initialize", json!({"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},
            "clientInfo":{"name":"Fox Kernel","version":env!("CARGO_PKG_VERSION")}}), token, deadline)?;
        if !initialized["protocolVersion"].is_string() { return Err("invalid MCP initialization".into()); }
        session.write(json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}), token, deadline)?;
        tools_from_result(session.request("tools/list", json!({}), token, deadline)?)?
    } else if let Some(session) = &mut http { tools_from_result(session.request("tools/list", json!({}))?)? }
    else { openapi.as_ref().unwrap().list_tools()? };
    token.check()?;
    let tools = normalize_tools(tools)?;
    let Some((name,arguments)) = call else { return Ok(json!({"tools":tools})); };
    let definition = tools.iter().find(|tool| tool["name"] == name).ok_or("MCP tool is not declared by the frozen connection")?;
    let validator = jsonschema::validator_for(&definition["inputSchema"]).map_err(|_| "invalid MCP input schema")?;
    if !validator.is_valid(arguments) { return Err("MCP arguments do not match its declared schema".into()); }
    let result = if let Some(session) = &mut stdio { session.request("tools/call", json!({"name":name,"arguments":arguments}), token, deadline)? }
    else if let Some(session) = &mut http { session.request("tools/call", json!({"name":name,"arguments":arguments}))? }
    else { openapi.as_ref().unwrap().call_tool(name,arguments)? };
    token.check()?;
    if Instant::now() >= deadline { return Err("MCP execution budget exceeded".into()); }
    Ok(result)
}

struct OwnedStdio {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    receiver: mpsc::Receiver<Result<String,String>>,
    reader: Option<thread::JoinHandle<()>>,
    writer: Option<thread::JoinHandle<()>>,
    next_id: u64,
}

impl OwnedStdio {
    fn start(server: &McpServerRecord) -> Result<Self,String> {
        let mut command = Command::new(&server.command);
        command.args(&server.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(environment) = load_environment(&server.id) { command.envs(environment); }
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| "cannot start owned MCP server")?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("missing owned MCP stdout")?;
        let (sender,receiver) = mpsc::sync_channel(32);
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let count = (&mut reader).take((MAX_RESPONSE_BYTES+1) as u64).read_until(b'\n', &mut bytes);
                let value = match count {
                    Ok(0) => break,
                    Ok(_) if bytes.len() > MAX_RESPONSE_BYTES => Err("owned MCP response exceeds limit".into()),
                    Ok(_) => String::from_utf8(bytes).map_err(|_| "owned MCP response is not UTF-8".into()),
                    Err(_) => Err("owned MCP output failed".into()),
                };
                let stop = value.is_err();
                if sender.try_send(value).is_err() || stop { break; }
            }
        });
        Ok(Self { child,stdin,receiver,reader:Some(reader),writer:None,next_id:1 })
    }

    fn write(&mut self, value: Value, token: &CancellationToken, deadline: Instant) -> Result<(),String> {
        token.check()?;
        let mut bytes = serde_json::to_vec(&value).map_err(|_| "invalid owned MCP request")?;
        if bytes.len() > 1_048_576 { return Err("owned MCP request exceeds limit".into()); }
        bytes.push(b'\n');
        let mut stdin = self.stdin.take().ok_or("owned MCP pipe is unavailable")?;
        let (sender,receiver) = mpsc::sync_channel(1);
        self.writer = Some(thread::spawn(move || {
            let result = stdin.write_all(&bytes).and_then(|_| stdin.flush()).map_err(|_| "owned MCP write failed".to_owned());
            let _ = sender.send((stdin,result));
        }));
        loop {
            token.check()?;
            if Instant::now() >= deadline { return Err("owned MCP write budget exceeded".into()); }
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok((stdin,result)) => { self.stdin = Some(stdin); self.writer.take().unwrap().join().map_err(|_| "MCP writer failed")?; return result; }
                Err(mpsc::RecvTimeoutError::Timeout) => {},
                Err(_) => return Err("owned MCP writer disconnected".into()),
            }
        }
    }

    fn request(&mut self, method: &str, params: Value, token: &CancellationToken, deadline: Instant) -> Result<Value,String> {
        let id = self.next_id; self.next_id += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),token,deadline)?;
        loop {
            token.check()?;
            if Instant::now() >= deadline { return Err("owned MCP request budget exceeded".into()); }
            match self.receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(line)) => {
                    let value = serde_json::from_str(&line).map_err(|_| "invalid owned MCP response")?;
                    if let Some(result) = extract_rpc_response(&value, &json!(id))? { return Ok(result); }
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout) => {},
                Err(_) => return Err("owned MCP output disconnected".into()),
            }
        }
    }
}

impl Drop for OwnedStdio {
    fn drop(&mut self) {
        crate::tool_host::terminate_process_tree(&mut self.child);
        self.stdin.take();
        if let Some(writer) = self.writer.take() { let _ = writer.join(); }
        if let Some(reader) = self.reader.take() { let _ = reader.join(); }
    }
}
