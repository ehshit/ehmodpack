use std::collections::BTreeSet;

use crate::lock::Lock;
use crate::manifest::{Manifest, PkgType, Support, Targetish};

pub const ALLOWED_DEPENDENCIES: [&str; 6] = [
    "minecraft",
    "forge",
    "neoforge",
    "neo-forge",
    "fabric-loader",
    "quilt-loader",
];

pub fn loader_key(kind: &str) -> &str {
    match kind {
        "fabric" => "fabric-loader",
        "quilt" => "quilt-loader",
        "neoforge" => "neoforge",
        "forge" => "forge",
        other => other,
    }
}

pub fn allowed_dependency(key: &str) -> bool {
    ALLOWED_DEPENDENCIES.contains(&key)
}

pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, text: String) {
        self.errors.push(text);
    }

    fn warn(&mut self, text: String) {
        self.warnings.push(text);
    }
}

pub fn check(
    lock: &Lock,
    manifest: &Manifest,
    sources: &[(String, std::path::PathBuf)],
) -> Report {
    let mut r = Report::new();

    if lock.name.trim().is_empty() {
        r.error("the pack has no name".to_string());
    }
    if lock.version.trim().is_empty() {
        r.error("the pack has no version, versionId would be empty".to_string());
    }
    if lock.targets.is_empty() {
        r.error("there are no targets to build for".to_string());
        return r;
    }

    for target in &lock.targets {
        let label = target.label();
        if target.minecraft.trim().is_empty() {
            r.error(format!("{label} has no minecraft version"));
        }
        if !allowed_dependency(&loader_key(&target.loader.kind)) {
            r.error(format!(
                "{label} uses loader {}, which is not one the modpack spec allows",
                target.loader.kind
            ));
        }
        if target.loader.version.trim().is_empty() {
            r.error(format!("{label} has no loader version"));
        }
        if target.packages.is_empty() {
            r.warn(format!("{label} has no packages at all"));
        }

        let mut paths: BTreeSet<&str> = BTreeSet::new();
        let mut actives: Vec<&str> = Vec::new();
        for pkg in &target.packages {
            let name = if pkg.project.is_empty() {
                pkg.filename.as_str()
            } else {
                pkg.project.as_str()
            };
            if pkg.downloads.is_empty() {
                r.error(format!("{label}: {name} has no download url"));
            } else if !pkg.downloads[0].starts_with("https://") {
                r.error(format!(
                    "{label}: {name} has a non https download url ({})",
                    pkg.downloads[0]
                ));
            }
            if pkg.hashes.sha512.is_empty() {
                r.error(format!("{label}: {name} has no sha512, a launcher cannot verify it"));
            }
            if pkg.hashes.sha1.is_empty() {
                r.warn(format!("{label}: {name} has no sha1"));
            }
            if pkg.file_size == 0 {
                r.warn(format!("{label}: {name} has a file size of 0"));
            }
            if pkg.path.is_empty() {
                r.error(format!("{label}: {name} has no path in the pack"));
            } else {
                if pkg.path.contains("..") || pkg.path.starts_with('/') {
                    r.error(format!("{label}: {} escapes the pack", pkg.path));
                }
                if !paths.insert(pkg.path.as_str()) {
                    r.error(format!("{label}: two packages share the path {}", pkg.path));
                }
            }
            if pkg.kind == PkgType::Mod && !pkg.path.starts_with("mods/") {
                r.warn(format!(
                    "{label}: {name} is a mod but lands in {}",
                    pkg.path
                ));
            }
            if pkg.position.is_some() {
                if pkg.env.client != Support::Required {
                    r.error(format!(
                        "{label}: {name} is active but is not marked client required, so a launcher will not install it and options.txt would point at nothing"
                    ));
                }
                if !pkg.path.starts_with("resourcepacks/") && !pkg.path.starts_with("shaderpacks/") {
                    r.error(format!(
                        "{label}: {name} is active but lands in {}, it needs to be a resourcepack or shader",
                        pkg.path
                    ));
                }
                actives.push(name);
            }
        }
        if actives.is_empty() && crate::build::pack_paths_on_disk(sources).is_empty() {
            r.warn(format!("{label} has no active resource pack"));
        }
    }

    let labels: Vec<String> = lock.targets.iter().map(|t| t.label()).collect();
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    for l in &labels {
        if !seen.insert(l) {
            r.error(format!("{l} is listed twice in the lockfile"));
        }
    }

    for pkg in &manifest.packages {
        if pkg.project.is_none() && pkg.id.is_none() {
            r.error("a package has neither project nor id".to_string());
        }
        if pkg.version.is_some() && !pkg.versions.is_empty() {
            r.warn(format!(
                "{} has both version and versions, version wins",
                pkg.key()
            ));
        }
        for key in pkg.versions.keys() {
            if key != "default" && key != "*" && !key.contains('+') && !key.starts_with('1') {
                r.warn(format!("{} has an odd versions key {key:?}", pkg.key()));
            }
        }
        for key in &pkg.skip {
            let mc = key.split('+').next().unwrap_or(key);
            if key != "*" && key != "default" && !mc.starts_with('1') {
                r.warn(format!("{} has an odd skip key {key:?}", pkg.key()));
            }
        }
    }

    if manifest.packages.is_empty() {
        r.warn("the pack has no packages at all".to_string());
    }

    r
}

pub fn check_sources(sources: &[(String, std::path::PathBuf)]) -> Report {
    let mut r = Report::new();
    let mut total = 0usize;
    for (name, root) in sources {
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![(root.clone(), root.clone())];
        while let Some((base, dir)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push((base.clone(), path));
                    continue;
                }
                total += 1;
                let rel = path
                    .strip_prefix(&base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel.contains("..") || rel.starts_with('/') || rel.contains(':') {
                    r.error(format!("{name}: {} would escape the pack", rel));
                }
                if std::fs::symlink_metadata(&path)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    r.error(format!("{name}: {rel} is a symlink, a pack cannot carry one"));
                }
            }
        }
    }
    r.warnings.push(format!("{total} override files"));
    r
}