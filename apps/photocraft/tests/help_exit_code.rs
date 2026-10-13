//! `--help` and `-h` print the usage and exit 0 before any window opens; before that, either
//! argument was read as a file to open.

/// Run `photocraft` with the given arguments in an isolated config dir and return the exit
/// status, stdout and stderr. No display variables, so a regression that would open a window
/// fails fast instead.
fn run_photocraft(args: &[&str]) -> (std::process::ExitStatus, String, String) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("photocraft-help-exit-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_photocraft"))
        .args(args)
        .env("PHOTOCRAFT_CONFIG_DIR", &dir)
        .env_remove("PHOTOCRAFT_CONTROL_PORT")
        .env_remove("PHOTOCRAFT_CONTROL_TOKEN")
        .env_remove("PHOTOCRAFT_CONTROL_TOKEN_FILE")
        .env_remove("PHOTOCRAFT_AUTOMATION_READ_ROOT")
        .env_remove("PHOTOCRAFT_AUTOMATION_WRITE_ROOT")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .output()
        .expect("failed to run photocraft binary");
    std::fs::remove_dir_all(&dir).ok();
    (output.status, String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

/// The usage must list every flag the argument loop accepts, and point at the headless sibling
/// entry point and the protocol documentation.
#[test]
fn help_prints_the_flags_and_exits_zero_without_opening_a_window() {
    for flag in ["--help", "-h"] {
        let (status, stdout, stderr) = run_photocraft(&[flag]);
        assert_eq!(status.code(), Some(0), "{flag}: stdout: {stdout}\nstderr: {stderr}");
        assert!(stdout.contains("USAGE"), "{flag}: {stdout}");
        for listed in [
            "--control <port>",
            "--control-token <64-hex>",
            "--control-token-file <path>",
            "--automation-read-root <dir>",
            "--automation-write-root <dir>",
            "--safe-gpu",
            "--in-window-menus",
            "--version",
        ] {
            assert!(stdout.contains(listed), "{flag} must list {listed}: {stdout}");
        }
        assert!(stdout.contains("photocraft-cli"), "{flag} must point at the headless entry point: {stdout}");
        assert!(stdout.contains("docs/control-protocol.md"), "{flag} must point at the protocol docs: {stdout}");
    }
}
