//! Package-2-D closeout §5: dump the persisted context-budget reports for the
//! isolated acceptance database, so the real trigger of a compaction stop is
//! established from data. Read-only; feature-gated test facility.

fn main() {
    let database_path = std::path::PathBuf::from(
        std::env::var_os("FOX_DATA_DIR")
            .expect("FOX_DATA_DIR must name the isolated acceptance directory"),
    )
    .join("fox.db");
    let filter = std::env::var_os("FOX_DUMP_RUN_ID").map(|value| value.to_string_lossy().into_owned());

    match fox_desktop_lib::dump_context_budget_reports(database_path, filter.as_deref()) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Err(error) => {
            eprintln!("CLOSEOUT_DUMP_FAILED {error}");
            std::process::exit(1);
        }
    }
}
