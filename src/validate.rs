use std::collections::BTreeSet;

use crate::i18n;
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

pub fn check(lock: &Lock, manifest: &Manifest, sources: &[(String, std::path::PathBuf)]) -> Report {
    let mut r = Report::new();

    if lock.name.trim().is_empty() {
        r.error(i18n::t("validate-pack-no-name"));
    }
    if lock.version.trim().is_empty() {
        r.error(i18n::t("validate-pack-no-version"));
    }
    if lock.targets.is_empty() {
        r.error(i18n::t("validate-no-targets"));
        return r;
    }

    for target in &lock.targets {
        let label = target.label();
        if target.minecraft.trim().is_empty() {
            r.error(i18n::t_args(
                "validate-target-no-minecraft",
                &[("label", &label)],
            ));
        }
        if !allowed_dependency(&loader_key(&target.loader.kind)) {
            r.error(i18n::t_args(
                "validate-loader-not-allowed",
                &[("label", &label), ("loader", &target.loader.kind)],
            ));
        }
        if target.loader.version.trim().is_empty() {
            r.error(i18n::t_args(
                "validate-target-no-loader-version",
                &[("label", &label)],
            ));
        }
        if target.packages.is_empty() {
            r.warn(i18n::t_args(
                "validate-target-no-packages",
                &[("label", &label)],
            ));
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
                r.error(i18n::t_args(
                    "validate-no-download-url",
                    &[("label", &label), ("name", name)],
                ));
            } else if !pkg.downloads[0].starts_with("https://") {
                r.error(i18n::t_args(
                    "validate-non-https-url",
                    &[
                        ("label", &label),
                        ("name", name),
                        ("url", &pkg.downloads[0]),
                    ],
                ));
            }
            if pkg.hashes.sha512.is_empty() {
                r.error(i18n::t_args(
                    "validate-no-sha512",
                    &[("label", &label), ("name", name)],
                ));
            }
            if pkg.hashes.sha1.is_empty() {
                r.warn(i18n::t_args(
                    "validate-no-sha1",
                    &[("label", &label), ("name", name)],
                ));
            }
            if pkg.file_size == 0 {
                r.warn(i18n::t_args(
                    "validate-zero-size",
                    &[("label", &label), ("name", name)],
                ));
            }
            if pkg.path.is_empty() {
                r.error(i18n::t_args(
                    "validate-no-path",
                    &[("label", &label), ("name", name)],
                ));
            } else {
                if pkg.path.contains("..") || pkg.path.starts_with('/') {
                    r.error(i18n::t_args(
                        "validate-escapes-pack",
                        &[("label", &label), ("path", &pkg.path)],
                    ));
                }
                if !paths.insert(pkg.path.as_str()) {
                    r.error(i18n::t_args(
                        "validate-duplicate-path",
                        &[("label", &label), ("path", &pkg.path)],
                    ));
                }
            }
            if pkg.kind == PkgType::Mod && !pkg.path.starts_with("mods/") {
                r.warn(i18n::t_args(
                    "validate-mod-wrong-folder",
                    &[("label", &label), ("name", name), ("path", &pkg.path)],
                ));
            }
            if pkg.position.is_some() {
                if pkg.env.client != Support::Required {
                    r.error(i18n::t_args(
                        "validate-active-not-client-required",
                        &[("label", &label), ("name", name)],
                    ));
                }
                if !pkg.path.starts_with("resourcepacks/") && !pkg.path.starts_with("shaderpacks/")
                {
                    r.error(i18n::t_args(
                        "validate-active-wrong-folder",
                        &[("label", &label), ("name", name), ("path", &pkg.path)],
                    ));
                }
                actives.push(name);
            }
        }
        if actives.is_empty() && crate::build::pack_paths_on_disk(sources).is_empty() {
            r.warn(i18n::t_args(
                "validate-no-active-resource-pack",
                &[("label", &label)],
            ));
        }
    }

    let labels: Vec<String> = lock.targets.iter().map(|t| t.label()).collect();
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    for l in &labels {
        if !seen.insert(l) {
            r.error(i18n::t_args("validate-duplicate-target", &[("label", l)]));
        }
    }

    for pkg in &manifest.packages {
        if pkg.project.is_none() && pkg.id.is_none() {
            r.error(i18n::t("validate-package-no-project-or-id"));
        }
        if pkg.version.is_some() && !pkg.versions.is_empty() {
            r.warn(i18n::t_args(
                "validate-version-and-versions",
                &[("key", &pkg.key())],
            ));
        }
        for key in pkg.versions.keys() {
            if key != "default" && key != "*" && !key.contains('+') && !key.starts_with('1') {
                r.warn(i18n::t_args(
                    "validate-odd-versions-key",
                    &[("key", &pkg.key()), ("value", key)],
                ));
            }
        }
        for key in &pkg.skip {
            let mc = key.split('+').next().unwrap_or(key);
            if key != "*" && key != "default" && !mc.starts_with('1') {
                r.warn(i18n::t_args(
                    "validate-odd-skip-key",
                    &[("key", &pkg.key()), ("value", key)],
                ));
            }
        }
        if pkg.position.is_some() && !pkg.positions.is_empty() {
            r.warn(i18n::t_args(
                "validate-position-and-positions",
                &[("key", &pkg.key())],
            ));
        }
        for key in pkg.positions.keys() {
            let mc = key.split('+').next().unwrap_or(key);
            if key != "default" && key != "*" && !mc.starts_with('1') {
                r.warn(i18n::t_args(
                    "validate-odd-positions-key",
                    &[("key", &pkg.key()), ("value", key)],
                ));
            }
        }
    }

    for pack in &manifest.external_packs {
        if pack.name.trim().is_empty() {
            r.error(i18n::t("validate-external-no-name"));
        }
        if pack.is_builtin() && pack.name.contains('/') {
            r.error(i18n::t_args(
                "validate-external-builtin-path",
                &[("name", &pack.name)],
            ));
        }
        if pack.position.is_some() && !pack.positions.is_empty() {
            r.warn(i18n::t_args(
                "validate-position-and-positions",
                &[("key", &pack.name)],
            ));
        }
    }

    if manifest.packages.is_empty() {
        r.warn(i18n::t("validate-pack-no-packages"));
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
                    r.error(i18n::t_args(
                        "validate-source-escapes-pack",
                        &[("name", name), ("path", &rel)],
                    ));
                }
                if std::fs::symlink_metadata(&path)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    r.error(i18n::t_args(
                        "validate-source-symlink",
                        &[("name", name), ("path", &rel)],
                    ));
                }
            }
        }
    }
    r.warnings.push(i18n::t_args(
        "validate-override-files",
        &[("count", &total.to_string())],
    ));
    r
}