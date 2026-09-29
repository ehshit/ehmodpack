use clap::Parser;
use owo_colors::OwoColorize;

use ehmodpack::{run, Cli};

fn main() {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("{}", format!("Error: {e:#}").red().bold());
            std::process::exit(1);
        }
    };
    if let Err(e) = runtime.block_on(run(cli)) {
        let full = format!("{e:#}");
        let mut lines = full.lines();
        if let Some(head) = lines.next() {
            eprintln!("{}", format!("Error: {head}").red().bold());
        }
        eprintln!();
        for line in lines {
            eprintln!("  {}", line.red());
        }
        std::process::exit(1);
    }
}