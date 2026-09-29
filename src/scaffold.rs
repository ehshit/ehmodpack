use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::cli::WorkflowKind;
use crate::loaders::{self, LoaderKind};
use crate::manifest::{
    self, Manifest, Package, PkgType, Softwares, Target, Targetish,
};
use crate::modrinth::Modrinth;

const BUILD_SOURCE_WORKFLOW: &str = include_str!("../assets/build-source.yml");
const BUILD_BINARIES_WORKFLOW: &str = include_str!("../assets/build-binaries.yml");
const REFRESH_SOURCE_WORKFLOW: &str = include_str!("../assets/refresh-source.yml");
const REFRESH_BINARIES_WORKFLOW: &str = include_str!("../assets/refresh-binaries.yml");
const OPTIONS_TXT: &str = include_str!("../assets/options.txt");
const GITIGNORE: &str = include_str!("../assets/gitignore");
const README: &str = include_str!("../assets/README.md");

#[derive(Debug, Default, Clone)]
pub struct NewPlan {
    pub name: Option<String>,
    pub folder: Option<String>,
    pub minecraft: Vec<String>,
    pub loaders: Vec<LoaderKind>,
    pub loader_version: Option<String>,
    pub workflow: Option<WorkflowKind>,
    pub git: Option<bool>,
}

pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("pack");
    }
    out
}

fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn tup(v: &str) -> (u64, u64, u64) {
        let mut parts = v
            .split('.')
            .filter(|p| !p.is_empty())
            .map(|p| p.parse().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    }
    tup(a).cmp(&tup(b))
}

pub fn starter_slugs(kind: LoaderKind) -> &'static [&'static str] {
    match kind {
        LoaderKind::Fabric => &["fabric-api", "fabric-language-kotlin"],
        LoaderKind::Quilt => &["quilted-fabric-api"],
        LoaderKind::NeoForge => &[],
    }
}

pub fn target_rows(targets: &[Target]) -> String {
    targets
        .iter()
        .map(|t| {
            format!(
                "| {} | {} | {} | {} |",
                t.minecraft,
                t.loader.kind,
                t.loader.version,
                t.java.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn today() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

pub fn write_project_files(
    dir: &Path,
    pack: &Manifest,
    softwares: &Softwares,
    name: &str,
    workflow: WorkflowKind,
) -> Result<()> {
    let rows = target_rows(&softwares.targets);
    let fill = |text: &str| -> String {
        text.replace("__NAME__", name)
            .replace("__SLUG__", &slugify(name))
            .replace("__TARGET_ROWS__", &rows)
    };
    pack.write(dir)?;
    softwares.write(dir)?;
    manifest::write_file(&dir.join("client-overrides/options.txt"), OPTIONS_TXT)?;
    manifest::write_file(&dir.join(".gitignore"), GITIGNORE)?;
    manifest::write_file(
        &dir.join(crate::manifest::CHANGELOG_FILE),
        &format!(
            "[[{}]]\nchangelogtext = \"\"\"\n- Initial release\n\"\"\"\n",
            today()
        ),
    )?;
    manifest::write_file(&dir.join("README.md"), &fill(README))?;
    let (build, refresh) = match workflow {
        WorkflowKind::Source => (BUILD_SOURCE_WORKFLOW, REFRESH_SOURCE_WORKFLOW),
        WorkflowKind::Binaries => (BUILD_BINARIES_WORKFLOW, REFRESH_BINARIES_WORKFLOW),
    };
    manifest::write_file(&dir.join(".github/workflows/build.yml"), &fill(build))?;
    manifest::write_file(&dir.join(".github/workflows/refresh.yml"), &fill(refresh))?;
    manifest::write_local_schemas(dir)?;
    Ok(())
}

pub fn ensure_empty_dir(dir: &Path, force: bool) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let empty = std::fs::read_dir(dir)
        .with_context(|| format!("could not read {}", dir.display()))?
        .next()
        .is_none();
    if !empty && !force {
        bail!(
            "{} already exists and is not empty, pass --force to write into it anyway",
            dir.display()
        );
    }
    Ok(())
}

pub async fn new(
    plan: NewPlan,
    parent: &Path,
    force: bool,
    api: &str,
) -> Result<(PathBuf, bool, Vec<Target>)> {
    let name = plan
        .name
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the modpack needs a name"))?;
    if plan.minecraft.is_empty() {
        bail!("the modpack needs at least one minecraft version");
    }
    if plan.loaders.is_empty() {
        bail!("the modpack needs at least one loader");
    }
    let git = plan.git.unwrap_or(false);

    let folder = plan
        .folder
        .clone()
        .map(|f| slugify(&f))
        .unwrap_or_else(|| slugify(&name));
    let dir = parent.join(folder);
    ensure_empty_dir(&dir, force)?;

    let client = Modrinth::new(api)?;
    let mut targets = Vec::new();
    for minecraft in &plan.minecraft {
        for kind in &plan.loaders {
            let version = match (&plan.loader_version, plan.loaders.len()) {
                (Some(v), 1) => v.clone(),
                _ => loaders::resolve(*kind, minecraft).await.with_context(|| {
                    format!("could not resolve the {kind} loader for {minecraft}")
                })?,
            };
            targets.push(Target::new(minecraft, *kind, &version));
        }
    }

    let single = targets.len() == 1;
    let mut order: Vec<String> = Vec::new();
    let mut kinds: BTreeMap<String, PkgType> = BTreeMap::new();
    let mut starters: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    for target in &targets {
        let kind = LoaderKind::parse(&target.loader.kind).unwrap_or(LoaderKind::Fabric);
        for slug in starter_slugs(kind) {
            let loaders = vec![target.loader.kind.clone()];
            let found = client
                .resolve(slug, Some(&target.minecraft), &loaders)
                .await
                .with_context(|| format!("could not resolve {slug} for {}", target.label()))?;
            let key = version_key(target, single);
            kinds.insert(
                found.project.slug.clone(),
                kind_of(&found.project.project_type),
            );
            starters
                .entry(found.project.slug.clone())
                .or_default()
                .insert(key, found.version.version_number.clone());
            if !order.contains(&found.project.slug) {
                order.push(found.project.slug.clone());
            }
        }
    }

    let packages: Vec<Package> = order
        .iter()
        .map(|slug| {
            let versions = starters.get(slug).cloned().unwrap_or_default();
            Package {
                kind: kinds.get(slug).copied().unwrap_or(PkgType::Mod),
                project: Some(slug.clone()),
                id: None,
                version: if single {
                    versions.get("default").cloned()
                } else {
                    None
                },
                versions: if single { BTreeMap::new() } else { versions },
                skip: Vec::new(),
                env: None,
                optional: None,
                active: None,
                position: None,
                ids: None,
                lock: None,
            }
        })
        .collect();

    let mut pack = Manifest {
        schema: None,
        schema_version: 0,
        name: name.clone(),
        modrinth_project_id: None,
        summary: "brrr".to_string(),
        version: "0.1.0".to_string(),
        top: plan
            .minecraft
            .iter()
            .max_by(|a, b| version_cmp(a, b))
            .cloned(),
        client: true,
        server: false,
        overrides: manifest::DEFAULT_OVERRIDES.to_string(),
        client_overrides: manifest::DEFAULT_CLIENT_OVERRIDES.to_string(),
        server_overrides: manifest::DEFAULT_SERVER_OVERRIDES.to_string(),
        packages,
    };
    let mut softwares = Softwares {
        schema: None,
        schema_version: 0,
        targets,
    };
    pack.stamp();
    softwares.stamp();

    write_project_files(&dir, &pack, &softwares, &name, plan.workflow.unwrap_or(WorkflowKind::Source))?;

    let committed = if git { init_repo(&dir)? } else { false };
    Ok((dir, committed, softwares.targets))
}

fn version_key(target: &Target, single: bool) -> String {
    if single {
        "default".to_string()
    } else {
        format!("{}+{}", target.minecraft, target.loader.kind)
    }
}

pub fn kind_of(project_type: &str) -> PkgType {
    match project_type {
        "resourcepack" => PkgType::Resourcepack,
        "shader" => PkgType::Shader,
        "datapack" => PkgType::Datapack,
        _ => PkgType::Mod,
    }
}

pub fn init_repo(dir: &Path) -> Result<bool> {
    let quiet = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("could not run git init")?
        .success();
    if !quiet {
        bail!("git init failed in {}", dir.display());
    }
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("could not run git add")?;
    Ok(Command::new("git")
        .args(["commit", "-q", "-m", "initial"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false))
}