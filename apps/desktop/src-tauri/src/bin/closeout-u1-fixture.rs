//! Package-2-D closeout: build the injected U1 parked starting point.
//!
//! A thin wrapper over [`fox_desktop_lib::build_u1_parked_fixture`], which does
//! all the work through production APIs. This binary exists only so the fixture
//! can be run against an already-open isolated database; it is gated behind the
//! `kernel-acceptance` feature and never ships in a normal production build.

fn main() {
    let data_dir = std::path::PathBuf::from(
        std::env::var_os("FOX_DATA_DIR")
            .expect("FOX_DATA_DIR must name the isolated acceptance directory"),
    );
    let project_root = std::path::PathBuf::from(
        std::env::var_os("FOX_ACCEPTANCE_PROJECT_ROOT")
            .expect("FOX_ACCEPTANCE_PROJECT_ROOT must name the isolated project root"),
    );
    let database_path = data_dir.join("fox.db");

    match fox_desktop_lib::build_u1_parked_fixture(database_path, project_root) {
        Ok((run_id, items)) => {
            println!("CLOSEOUT_FIXTURE_OK run_id={run_id} items={}", items.len());
            for item in &items {
                println!("CLOSEOUT_FIXTURE_ITEM {item}");
            }
        }
        Err(error) => {
            eprintln!("CLOSEOUT_FIXTURE_FAILED {error}");
            std::process::exit(1);
        }
    }
}
