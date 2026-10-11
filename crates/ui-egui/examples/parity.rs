//! Print Photoshop menu parity and optionally write or check the Markdown report.
//!
//! ```sh
//! cargo run -p photocraft-ui-egui --example parity -- [--write docs/parity-checklist.md] [--check docs/parity-checklist.md] [--json]
//! ```
//! Usually run through `cargo xtask parity [--check]`.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let p = photocraft_ui_egui::parity::compute();
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&p).expect("serialize parity"));
        return;
    }
    println!("Photoshop menu parity: {} / {} live ({:.1}%)", p.live, p.total, p.percent());
    for m in &p.menus {
        println!("  {:<8} {:>4} / {:<4}", m.menu, m.live, m.total);
    }
    if let Some(i) = args.iter().position(|a| a == "--write") {
        let path = args.get(i + 1).expect("--write needs a path");
        std::fs::write(path, p.to_markdown()).unwrap_or_else(|e| panic!("{path}: {e}"));
        println!("wrote {path}");
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--check") {
        let path = args.get(i + 1).expect("--check needs a path");
        let want = p.to_markdown();
        let have = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        if have == want {
            println!("{path} is up to date");
        } else {
            eprintln!("error: {path} is stale: run `cargo xtask parity` and commit the result");
            std::process::exit(1);
        }
    }
}
