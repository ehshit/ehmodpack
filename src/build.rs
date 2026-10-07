use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use crate::lock::{Lock, LockedTarget};
use crate::manifest::RpPosition;

pub const INTERNAL_PACK: &str = "fabric-resource-pack-v0";

pub fn pack_format_for(minecraft: &str) -> Option<u32> {
    Some(match minecraft {
        "1.19" | "1.19.1" | "1.19.2" => 9,
        "1.19.3" => 12,
        "1.19.4" => 13,
        "1.20" | "1.20.1" => 15,
        "1.20.2" => 18,
        "1.20.3" | "1.20.4" => 22,
        "1.20.5" | "1.20.6" => 32,
        "1.21" | "1.21.1" => 34,
        "1.21.2" | "1.21.3" => 42,
        "1.21.4" => 46,
        "1.21.5" => 55,
        "1.21.6" => 63,
        "1.21.7" | "1.21.8" => 64,
        "1.21.9" | "1.21.10" => 69,
        "1.21.11" => 75,
        "26.1" | "26.1.1" | "26.1.2" => 84,
        "26.2" => 88,
        "26.3" => 97,
        _ => return None,
    })
}

fn is_resource_pack(rel: &str) -> bool {
    rel.contains("resourcepacks/")
}

fn patch_mcmeta(bytes: &[u8], format: u32) -> Option<Vec<u8>> {
    let mut doc: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let pack = doc.get_mut("pack")?.as_object_mut()?;
    pack.insert("pack_format".to_string(), json!(format));
    if let Some(min) = pack.get("min_format").and_then(|v| v.as_u64()) {
        if min > format as u64 {
            pack.insert("min_format".to_string(), json!(format));
        }
    }
    if let Some(max) = pack.get("max_format").and_then(|v| v.as_u64()) {
        if max < format as u64 {
            pack.insert("max_format".to_string(), json!(format));
        }
    }
    serde_json::to_vec_pretty(&doc).ok()
}

fn patch_zip_mcmeta(bytes: &[u8], format: u32) -> Option<Vec<u8>> {
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let names: Vec<String> = (0..src.len())
        .filter_map(|i| src.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.ends_with("pack.mcmeta"))
        .collect();
    if names.is_empty() {
        return None;
    }
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for i in 0..src.len() {
        let mut file = src.by_index(i).ok()?;
        let name = file.name().to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data).ok()?;
        if names.contains(&name) {
            if let Some(fixed) = patch_mcmeta(&data, format) {
                data = fixed;
            }
        }
        out.start_file(name, opts).ok()?;
        out.write_all(&data).ok()?;
    }
    out.finish().ok().map(|c| c.into_inner())
}

pub fn mrpack_index(lock: &Lock, target: &LockedTarget) -> serde_json::Value {
    let files: Vec<serde_json::Value> = target
        .packages
        .iter()
        .map(|p| {
            json!({
                "path": &p.path,
                "hashes": {
                    "sha1": &p.hashes.sha1,
                    "sha512": &p.hashes.sha512,
                },
                "env": {
                    "client": p.env.client,
                    "server": p.env.server,
                },
                "downloads": &p.downloads,
                "fileSize": p.file_size,
            })
        })
        .collect();

    let mut dependencies = serde_json::Map::new();
    dependencies.insert("minecraft".to_string(), json!(target.minecraft));
    dependencies.insert(
        crate::validate::loader_key(&target.loader.kind).to_string(),
        json!(target.loader.version),
    );

    json!({
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": &lock.version,
        "name": &lock.name,
        "summary": &lock.summary,
        "files": files,
        "dependencies": serde_json::Value::Object(dependencies),
    })
}

pub fn output_name(lock: &Lock, target: &LockedTarget) -> String {
    let base = sanitise(&lock.name);
    if lock.multi() {
        format!(
            "{base}-mc-{}-{}",
            sanitise(&target.minecraft),
            target.loader.kind
        )
    } else {
        base
    }
}

pub fn build(
    lock: &Lock,
    target: &LockedTarget,
    out_dir: &Path,
    sources: &[(String, PathBuf)],
) -> Result<PathBuf> {
    let active: Vec<(String, Option<RpPosition>)> = target.active_entries();
    fs::create_dir_all(out_dir)
        .with_context(|| format!("could not create {}", out_dir.display()))?;
    let out_path = out_dir.join(format!("{}.mrpack", output_name(lock, target)));
    let file = fs::File::create(&out_path)
        .with_context(|| format!("could not create {}", out_path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let index = serde_json::to_string_pretty(&mrpack_index(lock, target))?;
    zip.start_file("modrinth.index.json", options)?;
    zip.write_all(format!("{}\n", index.trim_end()).as_bytes())?;

    let local = packs_on_disk(sources);
    for (name, root) in sources {
        let prefix = if name == "client-overrides" {
            "overrides"
        } else {
            name.as_str()
        };
        let mut found = Vec::new();
        collect(root, root, &mut found)?;
        found.sort();
        for (rel, abs) in found {
            let mut bytes =
                fs::read(&abs).with_context(|| format!("could not read {}", abs.display()))?;
            if rel == "options.txt" {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                bytes = inject_ordered(&text, &active, &local).into_bytes();
            } else if let Some(format) = pack_format_for(&target.minecraft) {
                if rel.ends_with("pack.mcmeta") && is_resource_pack(&rel) {
                    if let Some(fixed) = patch_mcmeta(&bytes, format) {
                        bytes = fixed;
                    }
                } else if rel.ends_with(".zip") && is_resource_pack(&rel) {
                    if let Some(fixed) = patch_zip_mcmeta(&bytes, format) {
                        bytes = fixed;
                    }
                }
            }
            zip.start_file(format!("{prefix}/{rel}"), options)?;
            zip.write_all(&bytes)?;
        }
    }

    zip.finish().context("could not finalise the archive")?;
    Ok(out_path)
}

pub fn inject_active(options: &str, active_files: &[String], local: &[String]) -> String {
    let active: Vec<(String, Option<RpPosition>)> = active_files
        .iter()
        .map(|f| (format!("file/{}", f.rsplit('/').next().unwrap_or(f)), None))
        .collect();
    inject_ordered(options, &active, local)
}

pub fn inject_ordered(
    options: &str,
    active: &[(String, Option<RpPosition>)],
    local: &[String],
) -> String {
    let existing = read_list(options, "resourcePacks")
        .unwrap_or_else(|| vec!["vanilla".to_string()]);
    let mut ranked: Vec<(String, u32)> = active
        .iter()
        .map(|(entry, position)| {
            (entry.clone(), position.unwrap_or_default().rank())
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let wanted: Vec<String> = ranked.iter().map(|(entry, _)| entry.clone()).collect();
    let bare_wanted: Vec<String> = ranked
        .iter()
        .map(|(entry, _)| entry.trim_start_matches("file/").to_string())
        .collect();

    let mut packs: Vec<String> = vec!["vanilla".to_string()];
    for entry in existing {
        if entry == "vanilla" || wanted.contains(&entry) {
            continue;
        }
        let bare = entry.trim_start_matches("file/");
        if bare_wanted.iter().any(|w| w == bare) {
            continue;
        }
        if !local.iter().any(|l| l == bare) {
            continue;
        }
        if !packs.contains(&entry) {
            packs.push(entry);
        }
    }    for entry in wanted {
        if !packs.contains(&entry) {
            packs.push(entry);
        }
    }

    let mut out = write_list(options, "resourcePacks", &packs);
    if text_has_key(&out, "incompatibleResourcePacks") {
        out = replace_line(&out, "incompatibleResourcePacks", "incompatibleResourcePacks:[]");
    } else {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("incompatibleResourcePacks:[]\n");
    }
    out
}

pub fn inject_resource_pack(options: &str, active_files: &[String]) -> String {
    inject_active(options, active_files, &[])
}

pub fn sha1_of_bytes(body: &[u8]) -> Option<String> {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(body);
    Some(hex(&hasher.finalize()))
}

pub fn sha1_of(path: &std::path::Path) -> Option<String> {
    let body = fs::read(path).ok()?;
    sha1_of_bytes(&body)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub fn pack_paths_on_disk(sources: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for (_, root) in sources {
        if !root.is_dir() {
            continue;
        }
        for folder in ["resourcepacks", "shaderpacks"] {
            let dir = root.join(folder);
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_zip = name.to_ascii_lowercase().ends_with(".zip");
                if !is_zip && !path.is_dir() {
                    continue;
                }
                if !out.iter().any(|(n, _)| *n == name) {
                    out.push((name, path));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn packs_on_disk(sources: &[(String, PathBuf)]) -> Vec<String> {
    pack_paths_on_disk(sources)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn read_list(text: &str, key: &str) -> Option<Vec<String>> {
    let line = text.lines().find(|l| is_key_line(l, key))?;
    let start = line.find('[')?;
    let end = line.rfind(']')?;
    serde_json::from_str(&format!("[{}]", &line[start + 1..end])).ok()
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("could not read {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

fn sanitise(name: &str) -> String {
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

fn is_key_line(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.starts_with(':'))
}

fn text_has_key(text: &str, key: &str) -> bool {
    text.lines().any(|l| is_key_line(l, key))
}

fn write_list(text: &str, key: &str, items: &[String]) -> String {
    let joined = items
        .iter()
        .map(|i| format!("\"{i}\""))
        .collect::<Vec<_>>()
        .join(",");
    let line = format!("{key}:[{joined}]");
    if text_has_key(text, key) {
        replace_line(text, key, &line)
    } else {
        let mut out = text.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
        out
    }
}

fn replace_line(text: &str, key: &str, line: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for l in text.split_inclusive('\n') {
        if is_key_line(l, key) {
            out.push_str(line);
            out.push('\n');
        } else {
            out.push_str(l);
        }
    }
    out
}

pub const STAMP_FILE: &str = ".built.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct BuildStamp {
    inputs: String,
    outputs: BTreeMap<String, String>,
}

pub fn inputs_digest(lock: &Lock, sources: &[(String, PathBuf)]) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(b"\n");
    if let Ok(bytes) = serde_json::to_vec(lock) {
        hasher.update(&bytes);
    }
    hasher.update(b"\n");
    let mut entries: Vec<(String, String)> = Vec::new();
    for (name, root) in sources {
        let mut found = Vec::new();
        if collect(root, root, &mut found).is_err() {
            continue;
        }
        for (rel, abs) in found {
            entries.push((format!("{name}/{rel}"), sha1_of(&abs).unwrap_or_default()));
        }
    }
    entries.sort();
    for (key, sum) in entries {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        hasher.update(sum.as_bytes());
        hasher.update(b"\n");
    }
    hex(&hasher.finalize())
}

pub fn write_stamp(
    out_dir: &Path,
    lock: &Lock,
    sources: &[(String, PathBuf)],
    built: &[PathBuf],
) -> Result<()> {
    let mut outputs = BTreeMap::new();
    for path in built {
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned())
            && let Some(sum) = sha1_of(path)
        {
            outputs.insert(name, sum);
        }
    }
    let stamp = BuildStamp {
        inputs: inputs_digest(lock, sources),
        outputs,
    };
    fs::create_dir_all(out_dir)?;
    let path = out_dir.join(STAMP_FILE);
    fs::write(&path, serde_json::to_vec_pretty(&stamp)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

pub fn stamp_fresh(out_dir: &Path, lock: &Lock, sources: &[(String, PathBuf)]) -> bool {
    let text = match fs::read_to_string(out_dir.join(STAMP_FILE)) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let stamp: BuildStamp = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(_) => return false,
    };
    if stamp.inputs != inputs_digest(lock, sources) {
        return false;
    }
    for target in &lock.targets {
        let name = format!("{}.mrpack", output_name(lock, target));
        let Some(want) = stamp.outputs.get(&name) else {
            return false;
        };
        let Some(got) = sha1_of(&out_dir.join(&name)) else {
            return false;
        };
        if &got != want {
            return false;
        }
    }
    true
}