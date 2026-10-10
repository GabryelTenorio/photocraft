//! Parse Cargo/clippy output into structured **diagnostics** and **test counts**.
//!
//! Research §8 lists "parsed diagnostics plus bounded raw logs" and "matched/executed/filtered test
//! counts" on a job manifest; §9b item 5 asks for "Cargo/clippy JSON mapped back to files, with
//! zero-test-match detection". This handles both cargo's `--message-format=json` stream and its human
//! output, so a job's evidence is structured regardless of how the gate runs cargo.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_DIAGNOSTICS: usize = 200;

/// Test counts aggregated across a job's test binaries.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TestCounts {
    /// Tests executed (`running N tests`).
    #[serde(default)]
    pub running: u64,
    #[serde(default)]
    pub passed: u64,
    #[serde(default)]
    pub failed: u64,
    #[serde(default)]
    pub ignored: u64,
    #[serde(default)]
    pub measured: u64,
    /// Tests filtered out (not matched by a filter) — the zero-test-match signal.
    #[serde(default)]
    pub filtered: u64,
}

/// A compiler/clippy diagnostic mapped back to a file location where possible.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
}

/// The parsed result of a job's output.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub entries: Vec<Diagnostic>,
    pub tests: TestCounts,
}

fn push(out: &mut Diagnostics, diagnostic: Diagnostic) {
    if out.entries.len() < MAX_DIAGNOSTICS {
        out.entries.push(diagnostic);
    }
}

/// Parse a job's (already bounded) log.
pub fn parse(log: &str) -> Diagnostics {
    let mut out = Diagnostics::default();
    let mut pending: Option<(String, Option<String>, String)> = None;
    for line in log.lines() {
        if let Some(d) = json_diagnostic(line) {
            push(&mut out, d);
            continue;
        }
        if let Some(counts) = test_result(line) {
            add_counts(&mut out.tests, &counts);
            continue;
        }
        if let Some(n) = running_count(line) {
            out.tests.running = out.tests.running.saturating_add(n);
            continue;
        }
        if let Some((file, line_no, column)) = span(line) {
            if let Some((level, code, message)) = pending.take() {
                push(&mut out, Diagnostic { level, code, message, file: Some(file), line: Some(line_no), column: Some(column) });
            }
            continue;
        }
        if let Some(header) = header(line) {
            pending = Some(header);
        }
    }
    out
}

fn json_diagnostic(line: &str) -> Option<Diagnostic> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;
    let level = message.get("level")?.as_str()?.to_string();
    let text = message.get("message")?.as_str()?.to_string();
    let code = message.get("code").and_then(|c| c.get("code")).and_then(Value::as_str).map(str::to_string);
    let span = message
        .get("spans")
        .and_then(Value::as_array)
        .and_then(|spans| spans.iter().find(|s| s.get("is_primary").and_then(Value::as_bool).unwrap_or(false)).or_else(|| spans.first()));
    let file = span.and_then(|s| s.get("file_name")).and_then(Value::as_str).map(str::to_string);
    let line = span.and_then(|s| s.get("line_start")).and_then(Value::as_u64);
    let column = span.and_then(|s| s.get("column_start")).and_then(Value::as_u64);
    Some(Diagnostic { level, code, message: text, file, line, column })
}

fn header(line: &str) -> Option<(String, Option<String>, String)> {
    let trimmed = line.trim_start();
    let level = if trimmed.starts_with("error") {
        "error"
    } else if trimmed.starts_with("warning") {
        "warning"
    } else {
        return None;
    };
    let rest = trimmed.get(level.len()..)?;
    if let Some(bracket) = rest.strip_prefix('[') {
        let end = bracket.find(']')?;
        let code = bracket.get(..end)?.to_string();
        let message = bracket.get(end + 1..)?.trim_start_matches(':').trim().to_string();
        return Some((level.to_string(), Some(code), message));
    }
    rest.strip_prefix(':').map(|message| (level.to_string(), None, message.trim().to_string()))
}

fn span(line: &str) -> Option<(String, u64, u64)> {
    let rest = line.trim_start().strip_prefix("-->")?.trim();
    parse_location(rest)
}

fn parse_location(s: &str) -> Option<(String, u64, u64)> {
    let mut parts = s.rsplitn(3, ':');
    let column = parts.next()?.trim().parse().ok()?;
    let line = parts.next()?.trim().parse().ok()?;
    let file = parts.next()?.trim().to_string();
    if file.is_empty() {
        return None;
    }
    Some((file, line, column))
}

fn number_before(s: &str, keyword: &str) -> Option<u64> {
    let i = s.find(keyword)?;
    s.get(..i)?.trim_end().rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn test_result(line: &str) -> Option<TestCounts> {
    if !line.trim_start().starts_with("test result:") {
        return None;
    }
    Some(TestCounts {
        running: 0,
        passed: number_before(line, " passed").unwrap_or(0),
        failed: number_before(line, " failed").unwrap_or(0),
        ignored: number_before(line, " ignored").unwrap_or(0),
        measured: number_before(line, " measured").unwrap_or(0),
        filtered: number_before(line, " filtered out").unwrap_or(0),
    })
}

fn add_counts(total: &mut TestCounts, add: &TestCounts) {
    total.passed = total.passed.saturating_add(add.passed);
    total.failed = total.failed.saturating_add(add.failed);
    total.ignored = total.ignored.saturating_add(add.ignored);
    total.measured = total.measured.saturating_add(add.measured);
    total.filtered = total.filtered.saturating_add(add.filtered);
}

fn running_count(line: &str) -> Option<u64> {
    let rest = line.trim().strip_prefix("running ")?;
    rest.strip_suffix(" tests")?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_human_diagnostics_with_location() {
        let log = "error[E0432]: unresolved import `foo`\n  --> crates/doc/src/lib.rs:10:5\nwarning: unused variable: `x`\n  --> crates/io/src/lib.rs:3:9\n";
        let d = parse(log);
        assert_eq!(d.entries.len(), 2);
        assert_eq!(d.entries[0].level, "error");
        assert_eq!(d.entries[0].code.as_deref(), Some("E0432"));
        assert_eq!(d.entries[0].file.as_deref(), Some("crates/doc/src/lib.rs"));
        assert_eq!(d.entries[0].line, Some(10));
        assert_eq!(d.entries[1].level, "warning");
        assert_eq!(d.entries[1].file.as_deref(), Some("crates/io/src/lib.rs"));
    }

    #[test]
    fn parses_cargo_json_diagnostics() {
        let line = r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"file_name":"crates/x/src/lib.rs","line_start":42,"column_start":7,"is_primary":true}]}}"#;
        let d = parse(line);
        assert_eq!(d.entries.len(), 1);
        assert_eq!(d.entries[0].code.as_deref(), Some("E0308"));
        assert_eq!(d.entries[0].file.as_deref(), Some("crates/x/src/lib.rs"));
        assert_eq!(d.entries[0].line, Some(42));
    }

    #[test]
    fn sums_test_counts_across_binaries() {
        let log = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\nrunning 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out\n";
        let d = parse(log);
        assert_eq!(d.tests.running, 3);
        assert_eq!(d.tests.passed, 3);
        assert_eq!(d.tests.filtered, 5);
    }

    #[test]
    fn filtered_run_has_no_passes() {
        let d = parse("running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n");
        assert_eq!(d.tests.passed, 0);
        assert_eq!(d.tests.running, 0);
    }
}
