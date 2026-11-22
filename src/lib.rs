pub mod account;
pub mod blockpage;
pub mod build;
pub mod cli;
pub mod commands;
pub mod http;
pub mod i18n;
pub mod importer;
pub mod java_install;
pub mod launcher;
pub mod loaders;
pub mod locale;
pub mod lock;
pub mod manifest;
pub mod mcmeta;
pub mod minecraft;
pub mod modmeta;
pub mod modrinth;
pub mod progress;
pub mod scaffold;
pub mod secret;
pub mod staging;
pub mod useragent;
pub mod validate;

pub use cli::{Cli, Cmd, Launcher, LocaleAction, WorkflowKind};

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    locale::prepare(&cli)?;
    commands::dispatch(cli).await
}