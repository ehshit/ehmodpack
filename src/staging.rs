use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha512};

use crate::build::{inject_active, packs_on_disk};
use crate::lock::LockedTarget;
use crate::modrinth::Modrinth;

pub struct Staged {
    pub dir: PathBuf,
    pub downloaded: usize,
    pub bytes: u64,
    pub shas: usize,
}

pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("ehmodpack-test")
}

pub fn dir_name(pack: &str, target: &LockedTarget) -> String {
    let clean: String = pack
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_matches('-').to_string();
    let base = if clean.is_empty() { "pack" } else { &clean };
    format!(
        "{}-mc-{}-{}",
        base,
        target.minecraft.replace('.', "-"),
        target.loader.kind
    )
}

pub fn fresh_dir(pack: &str, target: &LockedTarget) -> Result<PathBuf> {
    let dir = temp_root().join(dir_name(pack, target));
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("could not clear {}", dir.display()))?;
    }
    fs::create_dir_all(&dir)
        .with_context(|| format!("could not create {}", dir.display()))?;
    Ok(dir)
}

pub fn wipe(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir)
            .with_context(|| format!("could not delete {}", dir.display()))?;
    }
    Ok(())
}

pub fn copy_overrides(sources: &[(String, PathBuf)], dest: &Path) -> Result<usize> {
    let mut copied = 0usize;
    for (prefix, root) in sources {
        if prefix == "server-overrides" {
            continue;
        }
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir)
                .with_context(|| format!("could not read {}", dir.display()))?
            {
                let path = entry?.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let target = dest.join(&rel);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&path, &target).with_context(|| {
                    format!("could not copy {} to {}", path.display(), target.display())
                })?;
                copied += 1;
            }
        }
    }
    Ok(copied)
}

pub async fn download<F>(
    client: &Modrinth,
    target: &LockedTarget,
    dest: &Path,
    mut log: F,
) -> Result<Staged>
where
    F: FnMut(&str),
{
    let mut downloaded = 0usize;
    let mut bytes = 0u64;
    let mut shas = 0usize;
    for pkg in &target.packages {
        let out = dest.join(&pkg.path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let url = pkg
            .downloads
            .first()
            .with_context(|| format!("{} has no download url", pkg.project))?;
        let written = client
            .download(url, &out, &format!("fetching {}", pkg.path))
            .await
            .with_context(|| format!("could not download {}", pkg.project))?;
        let body = std::fs::read(&out).with_context(|| format!("could not read {}", out.display()))?;
        bytes += written;
        if !pkg.hashes.sha512.is_empty() {
            check_sha512(&pkg.project, &body, &pkg.hashes.sha512)?;
            shas += 1;
        }
        if !pkg.hashes.sha1.is_empty() {
            let got = crate::build::sha1_of_bytes(&body).unwrap_or_default();
            if got != pkg.hashes.sha1 {
                anyhow::bail!("{} did not match its sha1", pkg.project);
            }
            shas += 1;
        }
        log(&format!("fetched {}", pkg.project));
        downloaded += 1;
    }
    let options = dest.join("options.txt");
    if options.exists() {
        let active: Vec<String> = target
            .active_packs()
            .iter()
            .map(|p| {
                p.path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&p.path)
                    .to_string()
            })
            .collect();
        if !active.is_empty() {
            let text = fs::read_to_string(&options)
                .with_context(|| format!("could not read {}", options.display()))?;
            let local = packs_on_disk(&[("staged".to_string(), dest.to_path_buf())]);
            fs::write(&options, inject_active(&text, &active, &local))?;
            log(&format!("switched on {} resource packs", active.len()));
        }
    }
    Ok(Staged {
        dir: dest.to_path_buf(),
        downloaded,
        bytes,
        shas,
    })
}

pub fn scan_conflicts(
    dir: &Path,
    target: &crate::modmeta::Target,
) -> crate::modmeta::Report {
    let mut metas: Vec<crate::modmeta::ModMeta> = Vec::new();
    for folder in ["mods", "resourcepacks", "shaderpacks"] {
        let Ok(entries) = fs::read_dir(dir.join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|e| e != "jar") {
                continue;
            }
            if let Ok(Some(meta)) = crate::modmeta::read(&path) {
                if !meta.id.is_empty() {
                    metas.push(meta);
                }
            }
        }
    }
    crate::modmeta::check(&metas, target)
}

pub fn verify(_project: &str, body: &[u8], sha1: &str, sha512: &str) -> Result<()> {
    if !sha512.is_empty() {
        let mut hasher = Sha512::new();
        hasher.update(body);
        let got = format!("{:x}", hasher.finalize());
        if got != sha512 {
            anyhow::bail!("sha512 does not match. got {got}, wanted {sha512}");
        }
    }
    if !sha1.is_empty() {
        let got = crate::build::sha1_of_bytes(body).unwrap_or_default();
        if got != sha1 {
            anyhow::bail!("sha1 does not match. got {got}, wanted {sha1}");
        }
    }
    Ok(())
}

pub fn check_sha512(project: &str, body: &[u8], want: &str) -> Result<()> {
    if want.is_empty() {
        return Ok(());
    }
    let mut hasher = Sha512::new();
    hasher.update(body);
    let got = format!("{:x}", hasher.finalize());
    if got != want {
        anyhow::bail!(
            "{project} did not match its sha512, the download is corrupt. got {got}, wanted {want}"
        );
    }
    Ok(())
}

pub fn wait_for_enter(prompt: &str) {
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}