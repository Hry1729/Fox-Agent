use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

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

fn stage_windows_zvec_test_dll() {
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
        return;
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
            return;
        }
        panic!("cannot publish {}: {error}", destination.display());
    }
    fs::remove_file(&temporary).expect("remove only our verified temporary DLL");
    assert!(
        fs::read(&destination).expect("verify app-local Zvec DLL") == original,
        "app-local Zvec DLL differs from the selected build DLL"
    );
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
    if env::var_os("CARGO_FEATURE_ZVEC").is_some()
        && env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
    {
        stage_windows_zvec_test_dll();
    }
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
