//! Prints the mid-level IR of a program (`reference/native-plan.md`,
//! section 3), for working on the native compiler.
//!
//! ```text
//! cargo run --example ir -- FILE.tin
//! ```
//!
//! `!` after a variable marks its last use; `drop` lists what a block
//! releases on entry.

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: cargo run --example ir -- FILE.tin");
        return ExitCode::from(2);
    };
    let prog = match tinsel::runner::load(std::path::Path::new(&path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}: {}", e.err.msg);
            return ExitCode::FAILURE;
        }
    };
    match tinsel::ir::lower::lower(&prog) {
        Ok(m) => {
            print!("{m}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {}", e.msg);
            ExitCode::FAILURE
        }
    }
}
