use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

fn pe_machine(path: &Path) -> Result<u16, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)
        .map_err(|error| error.to_string())?;
    if &dos[..2] != b"MZ" {
        return Err(format!("{} has no DOS header", path.display()));
    }
    let pe_offset = u32::from_le_bytes(dos[0x3c..0x40].try_into().unwrap()) as u64;
    file.seek(SeekFrom::Start(pe_offset))
        .map_err(|error| error.to_string())?;
    let mut header = [0u8; 6];
    file.read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    if &header[..4] != b"PE\0\0" {
        return Err(format!("{} has no PE signature", path.display()));
    }
    Ok(u16::from_le_bytes([header[4], header[5]]))
}

fn stage_windows_zvec_test_dll() -> PathBuf {
    let target = env::var("TARGET").expect("Cargo TARGET");

    // This metadata comes from the direct, feature-matched zvec-rust-sys native
    // dependency. It names the directory actually selected for native linking.
    let source_dir = env::var("DEP_ZVEC_C_API_LIB_DIR")
        .expect("zvec-rust-sys did not expose its resolved lib_dir metadata");
    let source_dir = Path::new(&source_dir)
        .canonicalize()
        .expect("canonical Zvec library directory");
    let source = source_dir.join("zvec_c_api.dll");
    let import_lib = source_dir.join("zvec_c_api.lib");
    assert!(
        source.is_file(),
        "missing real Zvec DLL: {}",
        source.display()
    );
    assert!(
        import_lib.is_file(),
        "missing matching Zvec import library: {}",
        import_lib.display()
    );
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", import_lib.display());
    println!("cargo:rerun-if-env-changed=ZVEC_LIB_DIR");

    let expected_machine = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => 0x8664,
        Ok("aarch64") => 0xaa64,
        Ok("x86") => 0x014c,
        other => panic!("unsupported Windows Zvec target architecture: {other:?}"),
    };
    let actual_machine = pe_machine(&source).expect("read Zvec DLL PE header");
    assert_eq!(
        actual_machine, expected_machine,
        "Zvec DLL PE machine does not match target {target}"
    );

    // OUT_DIR is <profile>/build/fox-desktop-<hash>/out, including custom
    // CARGO_TARGET_DIR and explicit target triples. Test executables are in deps.
    let out_dir = env::var_os("OUT_DIR").expect("Cargo OUT_DIR");
    let build_dir = Path::new(&out_dir)
        .parent()
        .and_then(Path::parent)
        .expect("Fox build-script directory");
    assert_eq!(
        build_dir.file_name().and_then(|name| name.to_str()),
        Some("build")
    );
    let deps_dir = build_dir
        .parent()
        .expect("Cargo profile directory")
        .join("deps");
    fs::create_dir_all(&deps_dir).expect("create Cargo deps directory");
    let destination = deps_dir.join("zvec_c_api.dll");
    let original = fs::read(&source).expect("read real Zvec DLL");
    if destination.exists() {
        assert!(
            fs::read(&destination).expect("read existing app-local Zvec DLL") == original,
            "{} differs from the selected build DLL {}; refusing to use or overwrite it",
            destination.display(),
            source.display()
        );
        return source;
    }
    let temporary = deps_dir.join(format!(".zvec_c_api.{}.tmp", std::process::id()));
    fs::copy(&source, &temporary).expect("stage real Zvec DLL beside test executables");
    assert!(
        fs::read(&temporary).expect("verify staged Zvec DLL") == original,
        "staged Zvec DLL differs from the selected build DLL"
    );
    // A same-directory hard link publishes only if the destination is absent.
    // Unlike Windows rename, it cannot replace another build's different DLL.
    if let Err(error) = fs::hard_link(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        if destination.exists()
            && fs::read(&destination)
                .ok()
                .is_some_and(|bytes| bytes == original)
        {
            return source;
        }
        panic!("cannot publish {}: {error}", destination.display());
    }
    fs::remove_file(&temporary).expect("remove only our verified temporary DLL");
    assert!(
        fs::read(&destination).expect("verify app-local Zvec DLL") == original,
        "app-local Zvec DLL differs from the selected build DLL"
    );
    source
}

fn map_windows_zvec_bundle_resource(source: &Path) {
    const ZVEC_RESOURCE_GLOB: &str =
        "target/*/build/zvec-rust-sys-*/out/zvec-prebuilt/zvec_c_api.dll";

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest dir");
    let config_path = Path::new(&manifest_dir).join("tauri.conf.json");
    let config: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&config_path).expect("read Tauri configuration"),
    )
    .expect("parse Tauri configuration");
    let Some(default_target) = config
        .pointer("/bundle/resources")
        .and_then(serde_json::Value::as_object)
        .and_then(|resources| resources.get(ZVEC_RESOURCE_GLOB))
        .and_then(serde_json::Value::as_str)
    else {
        // The configured resource was removed or changed; there is no default
        // glob to replace, so leave the caller's config alone.
        return;
    };

    let mut overrides = env::var("TAURI_CONFIG")
        .ok()
        .map(|body| serde_json::from_str::<serde_json::Value>(&body).expect("parse TAURI_CONFIG"))
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(root) = overrides.as_object_mut() else {
        // A non-object TAURI_CONFIG replaces the whole config shape. Respect
        // that explicit caller override rather than silently rewriting it.
        return;
    };
    let Some(bundle) = root
        .entry("bundle")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    else {
        // A caller-supplied non-object bundle is also a complete override.
        return;
    };
    let resources = bundle
        .entry("resources")
        .or_insert_with(|| serde_json::json!({}));
    let Some(resources) = resources.as_object_mut() else {
        // Lists and other non-map resource overrides are caller-owned complete
        // replacements. Preserve them without changing their resource semantics.
        return;
    };

    let target = match resources.get(ZVEC_RESOURCE_GLOB) {
        Some(serde_json::Value::Null) => return,
        Some(serde_json::Value::String(target)) => target.clone(),
        Some(_) => return,
        None => default_target.to_owned(),
    };

    let source = source.to_string_lossy();
    let source = source
        .strip_prefix(r"\\?\UNC\")
        .map(|unc| format!(r"\\{unc}"))
        .or_else(|| source.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| source.into_owned())
        .replace('\\', "/");

    // Remove only the stale default glob, then map the verified native DLL at
    // its actual resolved path to the configured destination. A caller's
    // explicit string target is carried over; explicit null disables the DLL.
    resources.insert(ZVEC_RESOURCE_GLOB.into(), serde_json::Value::Null);
    resources
        .entry(source)
        .or_insert_with(|| serde_json::Value::String(target));
    env::set_var("TAURI_CONFIG", serde_json::to_string(&overrides).unwrap());
}

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Tauri's resource reaches its binary targets but not Cargo's library
        // unit-test executable. A real AppHandle test pulls in
        // TaskDialogIndirect, unavailable in unactivated system comctl32 v5.
        // Cargo's `rustc-link-arg-tests` omits lib unit tests, so use the same
        // linker manifest flags Tauri uses for its own tests. This additive
        // manifest declares only the Common Controls v6 dependency already
        // present in Tauri's product resource; it requests no privileges.
        let manifest = Path::new(&env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest dir"))
            .join("resources/tests/common-controls-v6.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    let zvec_source = if env::var_os("CARGO_FEATURE_ZVEC").is_some()
        && env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
    {
        Some(stage_windows_zvec_test_dll())
    } else {
        None
    };
    // The zvec native DLL is only produced when the `zvec` feature is enabled
    // (zvec-rust-sys downloads/builds it into OUT_DIR). On Windows/MSVC, replace
    // the default resource glob with the verified DLL path selected by Cargo;
    // this avoids relying on a target-directory glob that may not match a shared
    // or custom CARGO_TARGET_DIR. The DLL must never be faked.
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        if let Some(source) = zvec_source.as_deref() {
            map_windows_zvec_bundle_resource(source);
        }
    }
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

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // The linker now embeds the same v6 manifest for both the desktop
        // binary and lib unit tests. Leave Tauri's icon and version resource
        // intact, but omit only its duplicate RT_MANIFEST (id 1).
        tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new_without_app_manifest(),
        )).expect("build Tauri resources without a duplicate linker manifest");
    } else {
        tauri_build::build();
    }
}
