pub mod build;
pub mod cli;
pub mod commands;
pub mod http;
pub mod importer;
pub mod launcher;
pub mod loaders;
pub mod lock;
pub mod manifest;
pub mod minecraft;
pub mod modmeta;
pub mod modrinth;
pub mod progress;
pub mod scaffold;
pub mod secret;
pub mod staging;
pub mod validate;

pub use cli::{Cli, Cmd, Launcher, WorkflowKind};

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    commands::dispatch(cli).await
}