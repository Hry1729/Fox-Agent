use std::env;
use std::fs;

fn main() {
    // The zvec native DLL is only produced when the `zvec` feature is enabled
    // (zvec-rust-sys downloads/builds it into OUT_DIR). tauri.conf.json lists it
    // as a bundle resource glob; on a clean checkout built `--no-default-features`
    // (no zvec, no DLL) tauri_build would fail at the build-script stage with
    // "glob pattern ... zvec_c_api.dll didn't match any files". Drop that
    // resource entry when zvec is disabled. The DLL must never be faked: when the
    // feature IS on, tauri.conf.json's own glob is used unchanged (the override
    // is not applied), so a real zvec build still bundles the real DLL.
    if env::var_os("CARGO_FEATURE_ZVEC").is_none() {
        if let Ok(conf_path) = env::var("CARGO_MANIFEST_DIR") {
            let path = std::path::Path::new(&conf_path).join("tauri.conf.json");
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                    let key = "target/*/build/zvec-rust-sys-*/out/zvec-prebuilt/zvec_c_api.dll";
                    let has_key = value
                        .pointer("/bundle/resources")
                        .and_then(|r| r.as_object())
                        .map(|r| r.contains_key(key))
                        .unwrap_or(false);
                    if has_key {
                        // json_patch merge treats an explicit null as "remove this
                        // key", so the merged config drops only the zvec glob;
                        // every other bundle resource is untouched.
                        // Preserve caller-supplied dev URL/window overrides;
                        // isolated desktop acceptance uses those as well.
                        let mut overrides = env::var("TAURI_CONFIG")
                            .ok()
                            .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                            .unwrap_or_else(|| serde_json::json!({}));
                        if let Some(root) = overrides.as_object_mut() {
                            let bundle = root
                                .entry("bundle")
                                .or_insert_with(|| serde_json::json!({}));
                            if let Some(bundle) = bundle.as_object_mut() {
                                let resources = bundle
                                    .entry("resources")
                                    .or_insert_with(|| serde_json::json!({}));
                                if let Some(resources) = resources.as_object_mut() {
                                    resources.insert(key.into(), serde_json::Value::Null);
                                } else if let Some(resources) = resources.as_array_mut() {
                                    resources.retain(|resource| resource.as_str() != Some(key));
                                }
                            }
                        }
                        env::set_var("TAURI_CONFIG", serde_json::to_string(&overrides).unwrap());
                    }
                }
            }
        }
    }

    tauri_build::build()
}
