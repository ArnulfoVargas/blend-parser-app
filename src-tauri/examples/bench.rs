//! Backend cost of each UI command on a real model.
//!
//!     cargo run --release -p blend-parser-app --features bench --example bench -- <model> [runs]
//!
//! Prints a Markdown table: the operation, then the summary every command returns.

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args.next().expect("usage: bench <model> [runs]");
    let runs: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(5);
    let rows = blend_parser_app_lib::bench::run(std::path::Path::new(&model), runs);
    println!("| Operation | Op (ms) | Summary (ms) | Total (ms) |");
    println!("|---|---:|---:|---:|");
    for r in rows {
        println!("| {} | {:.1} | {:.1} | {:.1} |", r.op, r.op_ms, r.summary_ms, r.op_ms + r.summary_ms);
    }
}
