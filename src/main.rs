use std::io::IsTerminal;

use clap::Parser;
use owo_colors::OwoColorize;

use ehmodpack::i18n;
use ehmodpack::{Cli, run};

fn main() {
    let cli = Cli::parse();
    let colored =
        !cli.no_color && std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let line = format!("{} {e:#}", i18n::t("error-prefix"));
            if colored {
                eprintln!("{}", line.red().bold());
            } else {
                eprintln!("{line}");
            }
            std::process::exit(1);
        }
    };
    if let Err(e) = runtime.block_on(run(cli)) {
        let raw = format!("{e:#}");
        let (full, prettified) = match ehmodpack::account::pretty_error(&raw) {
            Some(pretty) => (pretty, true),
            None => (raw, false),
        };
        let mut lines = full.lines();
        if let Some(head) = lines.next() {
            let line = format!("{} {head}", i18n::t("error-prefix"));
            if colored {
                eprintln!("{}", line.red().bold());
            } else {
                eprintln!("{line}");
            }
        }
        if !prettified {
            eprintln!();
        }
        for line in lines {
            if colored {
                if prettified {
                    eprintln!("{}", line.red());
                } else {
                    eprintln!("  {}", line.red());
                }
            } else if prettified {
                eprintln!("{line}");
            } else {
                eprintln!("  {line}");
            }
        }
        std::process::exit(1);
    }
}