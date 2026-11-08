//! `openpets-rs`                  start the desktop pet (menu bar app)
//! `openpets-rs mcp`              run the stdio MCP server agents launch
//! `openpets-rs hook`             handle one Claude Code hook event (stdin)
//! `openpets-rs install-hooks`    add the hook to Claude Code settings
//! `openpets-rs uninstall-hooks`  remove it again

#[cfg(target_os = "macos")]
mod app;
mod claude_settings;
#[cfg(target_os = "macos")]
mod dock;
#[cfg(target_os = "macos")]
mod emote;
mod hook;
mod icon;
mod ipc;
mod mcp;
mod motion;
mod paths;
mod pet;
mod reaction;
mod timer;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match args.as_slice() {
        [] => run_app(),
        ["mcp"] => mcp::run().map_err(|error| format!("MCP server stopped: {error}")),
        ["hook", ..] => {
            // Claude Code may pass extra arguments in the future; a hook must
            // never fail because of them.
            hook::run();
            Ok(())
        }
        ["install-hooks"] => claude_settings::install(),
        ["uninstall-hooks"] => claude_settings::uninstall(),
        [.., "--version" | "-V"] => {
            println!("openpets-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [.., "--help" | "-h"] | ["help"] => {
            print_usage();
            Ok(())
        }
        _ => Err(format!(
            "unknown command `{}`; try `openpets-rs --help`",
            args.join(" ")
        )),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("openpets-rs: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
fn run_app() -> Result<(), String> {
    app::run()
}

#[cfg(not(target_os = "macos"))]
fn run_app() -> Result<(), String> {
    Err("the desktop pet only runs on macOS".into())
}

fn print_usage() {
    println!("openpets-rs {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Usage:");
    println!("  openpets-rs                  start the desktop pet");
    println!("  openpets-rs mcp              run the MCP server (stdio) for coding agents");
    println!("  openpets-rs hook             handle one Claude Code hook event from stdin");
    println!("  openpets-rs install-hooks    add the hook to Claude Code settings");
    println!("  openpets-rs uninstall-hooks  remove the hook from Claude Code settings");
    println!("  openpets-rs --version        print the version");
}
