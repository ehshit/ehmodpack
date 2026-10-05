use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::loaders::LoaderKind;
use crate::manifest::PkgType;
use crate::modrinth::Sort;

#[derive(Debug, Parser)]
#[command(
    name = "eh's modpack",
    version,
    about = "it does modpacks because it says so",
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
    #[arg(long, global = true, default_value = ".")]
    pub dir: PathBuf,
    #[arg(long, global = true, env = "EHMODPACK_API")]
    pub api: Option<String>,
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum WorkflowKind {
    Source,
    #[value(name = "binaries")]
    Binaries,
}

impl WorkflowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Binaries => "binaries",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    Search {
        query: Option<String>,
        #[arg(long, short)]
        kind: Option<PkgType>,
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long, value_enum)]
        sort: Option<Sort>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, default_value_t = 1)]
        page: usize,
    },
    Browse {
        #[arg(long, short)]
        kind: Option<PkgType>,
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long, value_enum, default_value_t = Sort::Downloads)]
        sort: Sort,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Info {
        project: String,
    },
    #[command(disable_version_flag = true)]
    Add {
        project: String,
        #[arg(long, short)]
        version: Option<String>,
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        kind: Option<PkgType>,
    },
    Remove {
        project: String,
    },
    Find {
        query: String,
        #[arg(long, short = 'r')]
        remove: bool,
    },
    List {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
    },
    Lock {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long)]
        check: bool,
    },
    Build {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long)]
        locked: bool,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        #[arg(long)]
        all: bool,
    },
    New {
        name: Option<String>,
        #[arg(long)]
        folder: Option<String>,
        #[arg(long, short = 'm')]
        mc: Vec<String>,
        #[arg(long, short = 'l')]
        loader: Vec<LoaderKind>,
        #[arg(long)]
        loader_version: Option<String>,
        #[arg(long, value_enum)]
        workflow: Option<WorkflowKind>,
        #[arg(long)]
        git: bool,
        #[arg(long)]
        force: bool,
    },
    NewFromMrpack {
        mrpack: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        folder: Option<String>,
        #[arg(long)]
        include_configs: bool,
        #[arg(long, value_enum)]
        workflow: Option<WorkflowKind>,
        #[arg(long)]
        git: bool,
        #[arg(long)]
        force: bool,
    },
    AddVer {
        version: String,
        #[arg(long, short = 'l')]
        loader: Vec<LoaderKind>,
    },
    Update {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        update_changelogs: bool,
    },
    OneVer {
        version: String,
    },
    SyncPackActive {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long)]
        no_download: bool,
    },
    Order {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long)]
        off: bool,
        packs: Vec<String>,
    },
    FixLoader {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long, short)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
    },
    Verify {
        #[arg(long)]
        mc: Option<String>,
        #[arg(long, short)]
        loader: Option<LoaderKind>,
        #[arg(long, short)]
        all: bool,
        #[arg(long)]
        keep_going: bool,
    },
    Publish {
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        project_id: Option<String>,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        #[arg(long, short = 'm')]
        mc: Option<String>,
        #[arg(long, short = 'l')]
        loader: Option<LoaderKind>,
    },
    Validate,
    SetRelease {
        version: String,
    },
    UpdateSchema,
    Test {
        #[arg(long, short = 'v')]
        ver: Option<String>,
        #[arg(long, short = 'l')]
        loader: Option<LoaderKind>,
        #[arg(long)]
        all: bool,
        #[arg(long, value_enum, default_value_t = Launcher::Auto)]
        launcher: Launcher,
        #[arg(long)]
        launch: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        keep: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Launcher {
    Auto,
    Prism,
    Hmc,
    None,
}