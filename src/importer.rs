use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::manifest::{Env, PkgType, Support};

pub const LICENSE_WARNING: &str = "While we have this for building a project out of a modrinth modpack please make sure that you got permissions to use this modpack according to their project licenses and Modrinth's content rules!";

#[derive(Debug, Clone)]
pub struct IndexFile {
    pub path: String,
    pub project_id: String,
    pub version_id: String,
    pub filename: String,
    pub hashes: crate::modrinth::Hashes,
    pub downloads: Vec<String>,
    pub file_size: u64,
    pub env: Env,
}

impl IndexFile {
    pub fn kind(&self) -> PkgType {
        let folder = self.path.split('/').next().unwrap_or_default();
        match folder {
            "resourcepacks" => PkgType::Resourcepack,
            "shaderpacks" => PkgType::Shader,
            "datapacks" => PkgType::Datapack,
            _ => PkgType::Mod,
        }
    }
}

pub fn read_index(path: &Path) -> Result<Value> {
    let file = fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a readable zip", path.display()))?;
    let mut text = String::new();
    archive
        .by_name("modrinth.index.json")
        .context("this archive has no modrinth.index.json, so it is not an mrpack")?
        .read_to_string(&mut text)
        .context("could not read modrinth.index.json")?;
    serde_json::from_str(&text).context("could not parse modrinth.index.json")
}

pub fn name_of(index: &Value) -> String {
    index
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("imported pack")
        .to_string()
}

pub fn summary_of(index: &Value) -> String {
    index
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub fn version_of(index: &Value) -> String {
    index
        .get("versionId")
        .and_then(Value::as_str)
        .unwrap_or("0.0.0")
        .to_string()
}

pub fn minecraft_of(index: &Value) -> Result<String> {
    index
        .get("dependencies")
        .and_then(|d| d.get("minecraft"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .context("this modpack does not say which minecraft version it targets")
}

pub fn loaders_of(index: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(deps) = index.get("dependencies").and_then(Value::as_object) else {
        return out;
    };
    for (key, value) in deps {
        let Some(kind) = key.strip_suffix("-loader") else {
            continue;
        };
        if let Some(version) = value.as_str() {
            out.push((kind.to_string(), version.to_string()));
        }
    }
    out
}

pub fn java_of(index: &Value) -> Option<String> {
    index
        .get("dependencies")
        .and_then(|d| d.get("java"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn support(value: Option<&Value>) -> Support {
    match value.and_then(Value::as_str) {
        Some("required") => Support::Required,
        Some("optional") => Support::Optional,
        _ => Support::Unsupported,
    }
}

pub fn files_of(index: &Value) -> Result<Vec<IndexFile>> {
    let list = index
        .get("files")
        .and_then(Value::as_array)
        .context("this modpack has no files array")?;
    let mut out = Vec::new();
    for entry in list {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .context("a file entry has no path")?;
        let downloads: Vec<String> = entry
            .get("downloads")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let (project_id, version_id) = first_download(&downloads)
            .or_else(|| {
                entry
                    .get("projectId")
                    .and_then(Value::as_str)
                    .zip(entry.get("versionId").and_then(Value::as_str))
                    .map(|(p, v)| (p.to_string(), v.to_string()))
            })
            .map(|(p, v)| (p.to_string(), v.to_string()))
            .with_context(|| {
                format!(
                    "{path} does not come from modrinth, so ehmodpack cannot track it. Add it to the overrides folder instead."
                )
            })?;
        let hashes = entry.get("hashes").cloned().unwrap_or(Value::Null);
        out.push(IndexFile {
            path: path.to_string(),
            filename: path.rsplit('/').next().unwrap_or(path).to_string(),
            project_id,
            version_id,
            hashes: crate::modrinth::Hashes {
                sha1: hashes
                    .get("sha1")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                sha512: hashes
                    .get("sha512")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            downloads,
            file_size: entry.get("fileSize").and_then(Value::as_u64).unwrap_or(0),
            env: Env {
                client: support(entry.pointer("/env/client")),
                server: support(entry.pointer("/env/server")),
            },
        });
    }
    Ok(out)
}

fn first_download(downloads: &[String]) -> Option<(String, String)> {
    let url = downloads.iter().find(|u| u.contains("/data/"))?;
    let after = url.split("/data/").nth(1)?;
    let mut parts = after.split('/');
    let project = parts.next()?.to_string();
    let mut version = parts.next()?.to_string();
    if version == "versions" {
        version = parts.next()?.to_string();
    }
    Some((project, version))
}

pub fn extract_overrides(
    mrpack: &Path,
    dest: &Path,
    include_configs: bool,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<usize> {
    let file = fs::File::open(mrpack).with_context(|| format!("could not open {}", mrpack.display()))?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut written = 0usize;
    for prefix in ["overrides", "client-overrides", "server-overrides"] {
        let root = dest.join(prefix);
        out.push((prefix.to_string(), root.clone()));
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let Some(name) = entry
                .name()
                .strip_prefix(&format!("{prefix}/"))
                .map(str::to_string)
            else {
                continue;
            };
            if name.is_empty() || name.ends_with('/') {
                continue;
            }
            if !include_configs && name.starts_with("config/") {
                continue;
            }
            let target = root.join(&name);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            fs::write(&target, data)
                .with_context(|| format!("could not write {}", target.display()))?;
            written += 1;
        }
    }
    Ok(written)
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

pub fn ensure_empty_dir(dir: &Path, force: bool) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let empty = fs::read_dir(dir)
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