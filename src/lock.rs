use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::{
    Env, LoaderSpec, PkgType, RpPosition, Targetish, LOCK_FILE,
};
use crate::modrinth::Hashes;

pub const LOCK_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    #[serde(rename = "type")]
    pub kind: PkgType,
    pub project: String,
    pub project_id: String,
    pub version_id: String,
    pub version_number: String,
    pub filename: String,
    pub path: String,
    pub hashes: Hashes,
    pub downloads: Vec<String>,
    pub file_size: u64,
    pub env: Env,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<RpPosition>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedExternal {
    pub name: String,
    pub active: bool,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<RpPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedTarget {
    pub minecraft: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java: Option<String>,
    pub loader: LoaderSpec,
    pub packages: Vec<LockedPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_packs: Vec<LockedExternal>,
}

impl LockedTarget {
    pub fn active_packs(&self) -> Vec<&LockedPackage> {
        self.packages
            .iter()
            .filter(|p| p.position.is_some())
            .collect()
    }

    pub fn active_entries(&self) -> Vec<(String, Option<RpPosition>)> {
        let mut out: Vec<(String, Option<RpPosition>)> = self
            .active_packs()
            .iter()
            .map(|p| {
                let bare = p.path.rsplit('/').next().unwrap_or(&p.path);
                (format!("file/{bare}"), p.position)
            })
            .collect();
        out.extend(self.external_packs.iter().filter(|e| e.active).map(|e| {
            let entry = if e.builtin {
                e.name.clone()
            } else {
                format!("file/{}", e.name)
            };
            (entry, e.position)
        }));
        out
    }
}

impl Targetish for LockedTarget {
    fn minecraft(&self) -> &str {
        &self.minecraft
    }

    fn loader_kind(&self) -> &str {
        &self.loader.kind
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lock {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(rename = "schemaVersion", default)]
    pub schema_version: u32,
    pub lock_version: u32,
    pub generated_at: String,
    pub manifest_hash: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub targets: Vec<LockedTarget>,
}

impl Lock {
    pub fn stamped(
        generated_at: String,
        manifest_hash: String,
        name: String,
        version: String,
        summary: String,
        targets: Vec<LockedTarget>,
    ) -> Self {
        Self {
            schema: Some(crate::manifest::lock_schema_url()),
            schema_version: crate::manifest::SCHEMA_VERSION,
            lock_version: LOCK_VERSION,
            generated_at,
            manifest_hash,
            name,
            version,
            summary,
            targets,
        }
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(LOCK_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let lock: Self = serde_json::from_str(&text)
            .with_context(|| format!("could not parse {}", path.display()))?;
        if lock.lock_version != LOCK_VERSION {
            bail!(
                "{} is lock format v{}, this build understands v{LOCK_VERSION}, run `ehmodpack lock` again",
                path.display(),
                lock.lock_version
            );
        }
        Ok(lock)
    }

    pub fn exists(dir: &Path) -> bool {
        dir.join(LOCK_FILE).exists()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("lockfile always serialises")
    }

    pub fn write(&self, dir: &Path) -> Result<()> {
        crate::manifest::write_file(&dir.join(LOCK_FILE), &self.to_json())
    }

    pub fn target(
        &self,
        mc: Option<&str>,
        loader: Option<crate::loaders::LoaderKind>,
    ) -> Result<&LockedTarget> {
        crate::manifest::pick(&self.targets, mc, loader)
    }

    pub fn labels(&self) -> Vec<String> {
        self.targets.iter().map(Targetish::label).collect()
    }

    pub fn multi(&self) -> bool {
        self.targets.len() > 1
    }
}