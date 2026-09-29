use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::loaders::{java_for, LoaderKind};

pub const MANIFEST_FILE: &str = "packages.json";
pub const SOFTWARES_FILE: &str = "softwares.json";
pub const LOCK_FILE: &str = "ehmodpack.lock.json";
pub const CHANGELOG_FILE: &str = "changelog.toml";
pub const DEFAULT_OVERRIDES: &str = "overrides";
pub const DEFAULT_CLIENT_OVERRIDES: &str = "client-overrides";
pub const DEFAULT_SERVER_OVERRIDES: &str = "server-overrides";
pub const SCHEMA_VERSION: u32 = 1;
pub const SCHEMA_BASE: &str = "https://docs.ehis.gay/ehmodpack/schema";
pub const PACKAGES_SCHEMA: &str = include_str!("../schema/packages.schema.json");
pub const SOFTWARES_SCHEMA: &str = include_str!("../schema/softwares.schema.json");
pub const LOCK_SCHEMA: &str = include_str!("../schema/lock.schema.json");
pub const SCHEMA_STALE: &str = "This schema version seems out of date, if you want to update this schema to the latest ver you can run ehmodpack update-schema";

pub fn packages_schema_url() -> String {
    format!("{SCHEMA_BASE}/packages.schema.json")
}

pub fn softwares_schema_url() -> String {
    format!("{SCHEMA_BASE}/softwares.schema.json")
}

fn yes() -> bool {
    true
}

fn zero_version() -> String {
    "0.0.0".to_string()
}

fn overrides_dir() -> String {
    DEFAULT_OVERRIDES.to_string()
}

fn client_overrides_dir() -> String {
    DEFAULT_CLIENT_OVERRIDES.to_string()
}

fn server_overrides_dir() -> String {
    DEFAULT_SERVER_OVERRIDES.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum PkgType {
    Mod,
    Resourcepack,
    Shader,
    Datapack,
}

impl PkgType {
    pub fn modrinth_type(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Resourcepack => "resourcepack",
            Self::Shader => "shader",
            Self::Datapack => "datapack",
        }
    }

    pub fn folder(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::Resourcepack => "resourcepacks",
            Self::Shader => "shaderpacks",
            Self::Datapack => "datapacks",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Support {
    Required,
    Optional,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Env {
    pub client: Support,
    pub server: Support,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            client: Support::Required,
            server: Support::Unsupported,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum RpPosition {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoaderSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub minecraft: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java: Option<String>,
    pub loader: LoaderSpec,
}

impl Target {
    pub fn new(minecraft: &str, kind: LoaderKind, loader_version: &str) -> Self {
        Self {
            minecraft: minecraft.to_string(),
            java: Some(java_for(minecraft).to_string()),
            loader: LoaderSpec {
                kind: kind.as_str().to_string(),
                version: loader_version.to_string(),
            },
        }
    }
}

pub trait Targetish {
    fn minecraft(&self) -> &str;
    fn loader_kind(&self) -> &str;
    fn label(&self) -> String {
        format!("{}/{}", self.minecraft(), self.loader_kind())
    }
}

impl Targetish for Target {
    fn minecraft(&self) -> &str {
        &self.minecraft
    }
    fn loader_kind(&self) -> &str {
        &self.loader.kind
    }
}

pub fn pick<'a, T: Targetish>(
    items: &'a [T],
    mc: Option<&str>,
    loader: Option<LoaderKind>,
) -> Result<&'a T> {
    if items.is_empty() {
        bail!("no targets, add one to {SOFTWARES_FILE}");
    }
    let want = format!(
        "{}/{}",
        mc.unwrap_or("*"),
        loader.map(LoaderKind::as_str).unwrap_or("*")
    );
    let hits: Vec<&T> = items
        .iter()
        .filter(|t| match mc {
            Some(want) => {
                let have = t.minecraft();
                let exact = have == want;
                let prefix = have.starts_with(want)
                    && want.matches('.').count() < have.matches('.').count();
                exact || prefix
            }
            None => true,
        })
        .filter(|t| match loader {
            Some(want) => t.loader_kind().eq_ignore_ascii_case(want.as_str()),
            None => true,
        })
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => bail!(
            "no target matches {want}, have {}",
            items.iter().map(Targetish::label).collect::<Vec<_>>().join(", ")
        ),
        _ => bail!(
            "{want} is ambiguous, pick one of {}",
            hits.iter()
                .map(|t| t.label())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    #[serde(rename = "type")]
    pub kind: PkgType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Env>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<RpPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<bool>,
}

impl Package {
    pub fn key(&self) -> String {
        self.project
            .clone()
            .or_else(|| self.id.clone())
            .unwrap_or_default()
    }

    pub fn is_active(&self) -> bool {
        self.active.unwrap_or(false)
    }

    pub fn env(&self) -> Env {
        if let Some(env) = self.env {
            return env;
        }
        let client = match (self.kind, self.optional.unwrap_or(false), self.is_active()) {
            (_, true, _) => Support::Optional,
            (PkgType::Mod, false, _) => Support::Required,
            (_, false, true) => Support::Required,
            (_, false, false) => Support::Optional,
        };
        Env {
            client,
            server: Support::Unsupported,
        }
    }

    pub fn version_keys(&self, minecraft: &str, loader: &str) -> Vec<String> {
        vec![
            format!("{minecraft}+{loader}"),
            minecraft.to_string(),
            "default".to_string(),
            "*".to_string(),
        ]
    }

    pub fn version_for(&self, minecraft: &str, loader: &str) -> Option<String> {
        if let Some(version) = &self.version {
            return Some(version.clone());
        }
        self.version_keys(minecraft, loader)
            .iter()
            .find_map(|key| self.versions.get(key).cloned())
    }

    pub fn is_skipped(&self, minecraft: &str, loader: &str) -> bool {
        if self.skip.is_empty() {
            return false;
        }
        let keys = [
            format!("{minecraft}+{loader}"),
            format!("{minecraft}+*"),
            minecraft.to_string(),
            "default".to_string(),
            "*".to_string(),
        ];
        self.skip
            .iter()
            .any(|raw| keys.iter().any(|key| key.eq_ignore_ascii_case(raw.trim())))
    }

    pub fn knows(&self, id: &str) -> bool {
        self.ids
            .as_deref()
            .is_some_and(|ids| ids.iter().any(|k| k.eq_ignore_ascii_case(id)))
    }

    pub fn remember(&mut self, id: &str) {
        if id.is_empty() || self.knows(id) {
            return;
        }
        let ids = self.ids.get_or_insert_with(Vec::new);
        ids.push(id.to_string());
    }

    pub fn is_locked(&self) -> bool {
        self.lock.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(rename = "schemaVersion", default)]
    pub schema_version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(
        rename = "modrinthProjectId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub modrinth_project_id: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default = "zero_version")]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<String>,
    #[serde(default = "yes")]
    pub client: bool,
    #[serde(default = "yes")]
    pub server: bool,
    #[serde(default = "overrides_dir")]
    pub overrides: String,
    #[serde(default = "client_overrides_dir")]
    pub client_overrides: String,
    #[serde(default = "server_overrides_dir")]
    pub server_overrides: String,
    #[serde(default)]
    pub packages: Vec<Package>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Softwares {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(rename = "schemaVersion", default)]
    pub schema_version: u32,
    #[serde(default)]
    pub targets: Vec<Target>,
}

impl Softwares {
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(SOFTWARES_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let parsed = serde_json::from_str(&text)
            .with_context(|| format!("could not parse {}", path.display()))?;
        Ok(Some(parsed))
    }

    pub fn write(&self, dir: &Path) -> Result<()> {
        write_file(
            &dir.join(SOFTWARES_FILE),
            &serde_json::to_string_pretty(self)?,
        )
    }

    pub fn stamp(&mut self) {
        self.schema = Some(softwares_schema_url());
        self.schema_version = SCHEMA_VERSION;
    }
}

pub fn validate_allowed(key: &str) -> bool {
    crate::validate::allowed_dependency(key)
}

pub fn validate_report(lock: &crate::lock::Lock, manifest: &Manifest) -> crate::validate::Report {
    crate::validate::check(lock, manifest, &[])
}

pub fn lock_schema_url() -> String {
    format!("{SCHEMA_BASE}/lock.schema.json")
}

pub fn write_local_schemas(dir: &Path) -> Result<Vec<PathBuf>> {
    let root = dir.join(".ehmodpack").join("schema");
    let mut written = Vec::new();
    for (name, body) in [
        ("packages.schema.json", PACKAGES_SCHEMA),
        ("softwares.schema.json", SOFTWARES_SCHEMA),
        ("lock.schema.json", LOCK_SCHEMA),
    ] {
        let path = root.join(name);
        write_file(&path, body)?;
        written.push(path);
    }
    Ok(written)
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        let value = jsonc_parser::parse_to_serde_value(text, &jsonc_parser::ParseOptions::default())
            .map_err(|e| anyhow::anyhow!("{MANIFEST_FILE}: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("{MANIFEST_FILE} is empty"))?;
        serde_json::from_value(value)
            .with_context(|| format!("could not parse {MANIFEST_FILE}"))
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest always serialises")
    }

    pub fn write(&self, dir: &Path) -> Result<()> {
        write_file(&dir.join(MANIFEST_FILE), &self.to_json())
    }

    pub fn stamp(&mut self) {
        self.schema = Some(packages_schema_url());
        self.schema_version = SCHEMA_VERSION;
    }

    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.to_json().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn packages_for(&self, target: &Target) -> Vec<(usize, String)> {
        self.packages
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    i,
                    p.version_for(&target.minecraft, &target.loader.kind)
                        .unwrap_or_else(|| "*".to_string()),
                )
            })
            .collect()
    }
}

pub fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let body = contents.trim_end();
    std::fs::write(path, format!("{body}\n"))
        .with_context(|| format!("could not write {}", path.display()))
}

pub fn find_manifest(start: &Path) -> Result<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(MANIFEST_FILE).exists() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => bail!(
                "no {MANIFEST_FILE} found in {} or any parent directory",
                start.display()
            ),
        }
    }
}