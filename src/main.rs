use std::io::IsTerminal;

use clap::Parser;
use owo_colors::OwoColorize;

use ehmodpack::{run, Cli};

fn main() {
    let cli = Cli::parse();
    let colored = !cli.no_color
        && std::env::var_os("NO_COLOR").is_none()
        && std::io::stderr().is_terminal();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let line = format!("Error: {e:#}");
            if colored {
                eprintln!("{}", line.red().bold());
            } else {
                eprintln!("{line}");
            }
            std::process::exit(1);
        }
    };
    if let Err(e) = runtime.block_on(run(cli)) {
        let full = format!("{e:#}");
        let mut lines = full.lines();
        if let Some(head) = lines.next() {
            let line = format!("Error: {head}");
            if colored {
                eprintln!("{}", line.red().bold());
            } else {
                eprintln!("{line}");
            }
        }
        eprintln!();
        for line in lines {
            if colored {
                eprintln!("  {}", line.red());
            } else {
                eprintln!("  {line}");
            }
        }
        std::process::exit(1);
    }
}