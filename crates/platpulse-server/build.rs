//! Build-time facts for the test suite.
//!
//! The shared test-database helper selects the Storage Backend from
//! `PLATPULSE_TEST_STORAGE` at run time, but libtest has no skip state: a test
//! that only makes sense against the SQLite file must be `ignore`d, not
//! passed, when the suite runs on PostgreSQL (ADR 0013). The attribute cannot
//! read the environment, so the build script turns the variable into the
//! `storage_postgres` cfg and `#[cfg_attr(storage_postgres, ignore = "…")]`
//! carries the reason into the test list.

fn main() {
    println!("cargo:rerun-if-env-changed=PLATPULSE_TEST_STORAGE");
    println!("cargo:rustc-check-cfg=cfg(storage_postgres)");
    if std::env::var("PLATPULSE_TEST_STORAGE").is_ok_and(|value| value.trim() == "postgres") {
        println!("cargo:rustc-cfg=storage_postgres");
    }
}
