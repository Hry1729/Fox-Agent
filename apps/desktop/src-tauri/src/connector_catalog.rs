use crate::database::{
    PluginActivation, PluginCardView, PluginInstallStatus, PluginKind, PluginOrigin,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn setup(resources: &Path) {
    let _ = RESOURCE_DIR.set(resources.join("connectors"));
}

pub fn command(server: &crate::database::McpServerRecord) -> Result<std::process::Command, String> {
    if server.command == "{fox-sse-bridge}" {
        return crate::connector_oauth::helper();
    }
    #[cfg(windows)]
    if server.id == "fox-catalog-filesystem" && server.command == "mcp-server-filesystem" {
        return Ok(std::process::Command::new("node"));
    }
    Ok(std::process::Command::new(&server.command))
}

pub fn command_args(server: &crate::database::McpServerRecord) -> Result<Vec<String>, String> {
    let mut args = server.args.clone();
    #[cfg(windows)]
    if server.id == "fox-catalog-filesystem" && server.command == "mcp-server-filesystem" {
        // npm installs a .cmd shim on Windows. Pass user-selected paths directly to Node.
        let paths = std::env::var_os("PATH").unwrap_or_default();
        let entry = std::env::split_paths(&paths)
            .map(|dir| dir.join("node_modules/@modelcontextprotocol/server-filesystem/dist/index.js"))
            .find(|path| path.is_file())
            .ok_or("未找到文件系统连接器，请安装 npm install -g @modelcontextprotocol/server-filesystem")?;
        args.insert(0, entry.to_string_lossy().into_owned());
    }
    if let Some(slot) = args
        .iter_mut()
        .find(|arg| arg.as_str() == "{fox-connector-adapter}")
    {
        let script = RESOURCE_DIR
            .get()
            .map(|dir| dir.join("stdio.py"))
            .filter(|p| p.is_file());
        #[cfg(debug_assertions)]
        let script = script.or_else(|| {
            Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/connectors/stdio.py"))
        });
        *slot = script
            .ok_or("安装包缺少连接器适配器")?
            .to_string_lossy()
            .into_owned();
    }
    Ok(args)
}

#[derive(Deserialize)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub setup: Setup,
}

#[derive(Deserialize)]
pub struct Setup {
    pub url: Option<String>,
    #[serde(default)]
    pub oauth: bool,
    pub registration: Option<String>,
}

pub static CONNECTORS: LazyLock<Vec<Connector>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../src/features/plugins/connector-catalog.json"
    ))
    .expect("bundled connector catalog must be valid")
});

pub fn preset(server_id: &str) -> Option<&'static Connector> {
    let id = server_id.strip_prefix("fox-catalog-")?;
    CONNECTORS.iter().find(|entry| entry.id == id)
}

pub fn cards() -> impl Iterator<Item = PluginCardView> {
    CONNECTORS.iter().map(|item| PluginCardView {
        id: format!("mcp:fox-catalog-{}", item.id),
        name: item.name.clone(),
        description: item.description.clone(),
        category: item.category.clone(),
        kind: PluginKind::Mcp,
        origin: PluginOrigin::Builtin,
        icon: Some("cable".to_owned()),
        version: None,
        install_status: PluginInstallStatus::NotInstalled,
        activation: PluginActivation::Global { enabled: false },
        runtime_status: None,
        permissions: Vec::new(),
        compatible: true,
        incompatibility_reason: None,
        last_error: None,
        installed_at: None,
        updated_at: None,
    })
}
