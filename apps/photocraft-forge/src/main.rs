//! `forge` stdio entry point.
//!
//! Environment-independent: no arguments, no config files, no network. It speaks MCP over
//! stdin/stdout and discovers what it needs (the repo root, `git`/`cargo`, optional companions) at
//! runtime.

use std::process::ExitCode;

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("forge: failed to start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(photocraft_forge::serve_stdio()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("forge: {e}");
            ExitCode::FAILURE
        }
    }
}
