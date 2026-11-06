use crate::{app_state::AppState, database::Database, model_service};
use fox_agent_backend::credential_save::{save_with_keys, KeyPlan};
use serde::{Deserialize, Serialize};
use tauri::{State, WebviewWindow};

const SETTING: &str = "search.service";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SearchProvider {
    #[default]
    Auto,
    Brave,
    Tavily,
    Exa,
    Searxng,
    DuckDuckGo,
}

impl SearchProvider {
    pub(crate) const PLUGINS: [(Self, &'static str, &'static str, &'static str); 4] = [
        (Self::Tavily, "search:tavily", "Tavily", "使用 Tavily API 搜索网页。在设置中的搜索服务配置 API Key。"),
        (Self::Brave, "search:brave", "Brave Search", "使用 Brave 的网页索引。在设置中的搜索服务配置 API Key。"),
        (Self::Exa, "search:exa", "Exa", "使用 Exa 搜索网页与内容摘要。在设置中的搜索服务配置 API Key。"),
        (Self::Searxng, "search:searxng", "SearXNG", "使用自托管的 SearXNG。在设置中的搜索服务填写实例地址。"),
    ];

    fn key_source(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Brave => Some(("fox-search:brave", "BRAVE_API_KEY")),
            Self::Tavily => Some(("fox-search:tavily", "TAVILY_API_KEY")),
            Self::Exa => Some(("fox-search:exa", "EXA_API_KEY")),
            _ => None,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SearchConfig {
    pub provider: SearchProvider,
    searxng_url: String,
}

impl SearchConfig {
    pub(crate) fn connection_value(&self, provider: SearchProvider) -> Result<String, String> {
        if let Some((id, env)) = provider.key_source() {
            return model_service::get_api_key(id)
                .or_else(|| std::env::var(env).ok().filter(|key| !key.trim().is_empty()))
                .ok_or_else(|| "请到设置中的搜索服务填写 API Key。".to_owned());
        }
        if provider == SearchProvider::Searxng {
            let url = if self.searxng_url.is_empty() {
                std::env::var("FOX_SEARXNG_URL").unwrap_or_default()
            } else {
                self.searxng_url.clone()
            };
            let parsed = url::Url::parse(&url).map_err(|_| "请输入完整的 SearXNG 实例地址。")?;
            if !matches!(parsed.scheme(), "https" | "http") || parsed.host().is_none()
                || !parsed.username().is_empty() || parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() {
                return Err("请输入 HTTP 或 HTTPS 实例地址，地址中不能包含密码、查询参数或片段。".into());
            }
            return Ok(url);
        }
        Ok(String::new())
    }
}

pub(crate) fn load(database: &Database) -> Result<SearchConfig, String> {
    database.app_setting(SETTING)?.map(|value| serde_json::from_str(&value).map_err(|e| e.to_string())).transpose().map(Option::unwrap_or_default)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchView {
    #[serde(flatten)]
    config: SearchConfig,
    configured_keys: Vec<SearchProvider>,
}

fn view(database: &Database) -> Result<SearchView, String> {
    let config = load(database)?;
    let configured_keys = [SearchProvider::Tavily, SearchProvider::Brave, SearchProvider::Exa]
        .into_iter().filter(|provider| config.connection_value(*provider).is_ok()).collect();
    Ok(SearchView { config, configured_keys })
}

#[tauri::command]
pub(crate) fn search_service_get(window: WebviewWindow, state: State<'_, AppState>) -> Result<SearchView, String> {
    crate::commands::require_trusted_local_window(&window)?;
    view(&state.database)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchUpdate {
    provider: SearchProvider,
    searxng_url: Option<String>,
    api_key: Option<String>,
}

pub(crate) fn activate_plugin(database: &Database, provider: SearchProvider, enabled: bool) -> Result<(), String> {
    if !enabled && load(database)?.provider != provider {
        return Ok(());
    }
    select(database, SearchUpdate {
        provider: if enabled { provider } else { SearchProvider::Auto }, api_key: None, searxng_url: None,
    })
}

fn select(database: &Database, update: SearchUpdate) -> Result<(), String> {
    let mut config = load(database)?;
    config.provider = update.provider;
    if let Some(url) = update.searxng_url {
        config.searxng_url = url.trim().to_owned();
    }
    let mut plan = KeyPlan::default();
    if let (Some((id, _)), Some(key)) = (config.provider.key_source(), update.api_key.as_deref().map(str::trim).filter(|key| !key.is_empty())) {
        plan.before_record.push((id.to_owned(), Some(key.to_owned())));
    } else {
        config.connection_value(config.provider)?;
    }
    let value = serde_json::to_string(&config).map_err(|error| error.to_string())?;
    save_with_keys(&mut model_service::KeyringStore, &plan, || database.set_app_setting(SETTING, &value))
        .map_err(|error| format!("搜索服务保存失败：{error:?}"))
}

#[tauri::command]
pub(crate) fn search_service_save(window: WebviewWindow, state: State<'_, AppState>, request: SearchUpdate) -> Result<SearchView, String> {
    crate::commands::require_trusted_local_window(&window)?;
    select(&state.database, request)?;
    view(&state.database)
}
