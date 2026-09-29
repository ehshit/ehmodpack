use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use futures_util::StreamExt;
use sha1::{Digest, Sha1};

pub const PISTON_META: &str = "https://piston-meta.mojang.com";
pub const RESOURCE_PACK: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone, Deserialize)]
pub struct VersionSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default, rename = "releaseTime")]
    pub release_time: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest: Latest,
    pub versions: Vec<VersionSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionJson {
    pub id: String,
    #[serde(default, rename = "inheritsFrom")]
    pub inherits_from: Option<String>,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    #[serde(default, rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default, rename = "assetIndex")]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaVersion {
    #[serde(default)]
    pub component: String,
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    #[serde(default, rename = "totalSize")]
    pub total_size: u64,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Ruled {
        rules: Vec<Rule>,
        value: serde_json::Value,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub action: Option<String>,
    pub os: Option<Os>,
    #[serde(rename = "features", default)]
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Os {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub natives: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub extract: Option<Extract>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: Option<BTreeMap<String, Artifact>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Extract {
    pub exclude: Vec<String>,
}

impl Library {
    pub fn group_artifact(&self) -> String {
        let parts: Vec<&str> = self.name.split(':').collect();
        if parts.len() < 3 {
            return self.name.clone();
        }
        format!("{}:{}:{}", parts[0], parts[1], parts[2])
    }

    pub fn path(&self) -> Option<&str> {
        self.downloads
            .as_ref()?
            .artifact
            .as_ref()
            .map(|a| a.path.as_str())
    }

    pub fn allowed(&self, os_name: &str, arch: &str) -> bool {
        match &self.rules {
            None => true,
            Some(rules) => {
                let ctx = Ctx {
                    os_name: os_name.to_string(),
                    arch: arch.to_string(),
                    features: BTreeMap::new(),
                };
                rules_allow(rules, &ctx)
            }
        }
    }

    pub fn native_classifier(&self, os_name: &str) -> Option<&String> {
        let natives = self.natives.as_ref()?;
        let key = match os_name {
            "windows" => "windows",
            "osx" => "osx",
            _ => "linux",
        };
        natives.get(key).or_else(|| natives.get("linux"))
    }
}

pub fn current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "x86"
    }
}

pub async fn manifest(http: &crate::http::Http) -> Result<VersionManifest> {
    http.get_json(&format!("{PISTON_META}/mc/game/version_manifest_v2.json"))
        .await
}

pub async fn version_json(
    http: &crate::http::Http,
    manifest: &VersionManifest,
    id: &str,
) -> Result<VersionJson> {
    let want = manifest
        .versions
        .iter()
        .find(|v| v.id == id)
        .with_context(|| format!("minecraft {id} is not in the version manifest"))?;
    http.get_json(&want.url).await
}

pub fn library_file(dir: &Path, path: &str) -> PathBuf {
    let mut full = dir.join("libraries");
    for part in path.split('/') {
        full.push(part);
    }
    full
}

pub async fn download_library(
    http: &crate::http::Http,
    dir: &Path,
    artifact: &Artifact,
) -> Result<PathBuf> {
    let out = library_file(dir, &artifact.path);
    if out.is_file() {
        return Ok(out);
    }
    http.download(&artifact.url, &out, &format!("fetching {}", artifact.path))
        .await
        .with_context(|| format!("could not fetch {}", artifact.path))?;
    let body = std::fs::read(&out).with_context(|| format!("could not read {}", out.display()))?;
    let got = sha1_hex(&body);
    if !artifact.sha1.is_empty() && got != artifact.sha1 {
        bail!(
            "{} did not match its sha1, wanted {} got {got}",
            artifact.path,
            artifact.sha1
        );
    }
    Ok(out)
}

pub struct Natives {
    pub java: PathBuf,
    pub java_major: u32,
    pub libraries: usize,
    pub assets: usize,
    jvm: Vec<String>,
    main_class: String,
}

impl std::fmt::Debug for Natives {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Natives")
            .field("java", &self.java)
            .field("java_major", &self.java_major)
            .field("libraries", &self.libraries)
            .field("main_class", &self.main_class)
            .field("jvm", &self.jvm.len())
            .finish()
    }
}

pub fn split_jvm(jvm: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for arg in jvm {
        if let Some(rest) = arg.strip_prefix("-cp ") {
            out.push("-cp".to_string());
            out.push(rest.to_string());
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--class-path ") {
            out.push("--class-path".to_string());
            out.push(rest.to_string());
            continue;
        }
        out.push(arg.clone());
    }
    out
}

pub async fn install<F>(
    http: &crate::http::Http,
    base: &Path,
    target: &crate::lock::LockedTarget,
    mut log: F,
) -> Result<Natives>
where
    F: FnMut(&str),
{
    log("reading the version manifest");
    let manifest = manifest(http).await?;
    let vanilla = version_json(http, &manifest, &target.minecraft).await?;
    let installed = crate::launcher::install(http, base, target, &mut log).await?;
    let version = flatten(&vanilla, &installed.version, &mut log).await?;

    let ctx = Ctx::default();
    let os_name = ctx.os_name.clone();
    let arch = ctx.arch.clone();

    let native_dir = base.join("natives");
    fs::create_dir_all(&native_dir)?;
    let mut classpath: Vec<PathBuf> = Vec::new();
    let mut libraries = 0usize;

    for lib in &version.libraries {
        if !lib.allowed(&os_name, &arch) {
            continue;
        }
        let Some(artifact) = lib
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .cloned()
        else {
            if let (Some(classifier), Some(downloads)) = (lib.native_classifier(&os_name), &lib.downloads)
            {
                if let Some(native) = downloads
                    .classifiers
                    .as_ref()
                    .and_then(|c| c.get(classifier))
                {
                    let out = download_native(http, base, native, &native_dir, &lib.extract).await?;
                    log(&format!("{} native files", out));
                }
                continue;
            }
            let path = crate::launcher::maven_path(&lib.group_artifact());
            if path.is_empty() {
                continue;
            }
            let out = library_file(base, &path);
            if out.is_file() {
                classpath.push(out);
                libraries += 1;
            } else {
                log(&format!("missing {}, skipping", lib.name));
            }
            continue;
        };
        let out = download_library(http, base, &artifact).await?;
        classpath.push(out);
        libraries += 1;
    }

    let game_id = version
        .inherits_from
        .clone()
        .unwrap_or_else(|| version.id.clone());
    let version_jar = base
        .join("versions")
        .join(&game_id)
        .join(format!("{game_id}.jar"));
    if !version_jar.is_file() {
        if let Some(parent) = version_jar.parent() {
            fs::create_dir_all(parent)?;
        }
        let summary = manifest
            .versions
            .iter()
            .find(|v| v.id == game_id)
            .with_context(|| format!("minecraft {game_id} is not in the version manifest"))?;
        log(&format!("fetching the game jar for {game_id}"));
        let body = http.get_bytes(&summary.url).await?;
        let info: serde_json::Value = serde_json::from_slice(&body)?;
        let downloads = info
            .get("downloads")
            .and_then(|d| d.get("client"))
            .context("the version json had no client download")?;
        let url = downloads
            .get("url")
            .and_then(|u| u.as_str())
            .context("the client download had no url")?;
        let want = downloads.get("sha1").and_then(|s| s.as_str()).unwrap_or("");
        http.download(url, &version_jar, &format!("fetching {game_id}.jar"))
            .await?;
        if !want.is_empty() {
            let jar = fs::read(&version_jar)?;
            let got = sha1_hex(&jar);
            if got != want {
                bail!("the game jar {game_id} did not match its sha1");
            }
        }
    }
    if version_jar.is_file() {
        classpath.insert(0, version_jar);
        log(&format!("game jar {game_id} is on the classpath"));
    }

    let mut assets = 0usize;
    if let Some(index) = &version.asset_index {
        log("fetching the asset index");
        let wanted = asset_index(http, base, index).await?;
        let total = wanted.len();
        let missing: Vec<(String, PathBuf)> = wanted
            .values()
            .map(|hash| {
                (
                    format!("{RESOURCE_PACK}/{}/{hash}", &hash[..2]),
                    asset_path(base, hash),
                )
            })
            .filter(|(_, path)| !path.is_file())
            .collect();
        let already = total - missing.len();
        assets = total;
        if missing.is_empty() {
            log(&format!("all {total} assets are already here"));
        } else {
            log(&format!(
                "fetching {} of {total} assets, {already} are already here",
                missing.len()
            ));
            let mut bar = crate::progress::Bar::new("fetching assets");
            let chunk_size = 240usize;
            let mut got = 0u64;
            for chunk in missing.chunks(chunk_size) {
                let jobs = chunk.iter().map(|(url, dest)| {
                    let client = http.clone();
                    let url = url.clone();
                    let dest = dest.clone();
                    async move { client.fetch_to(&url, &dest).await }
                });
                let results: Vec<Result<u64>> = futures_util::stream::iter(jobs)
                    .buffer_unordered(24)
                    .collect()
                    .await;
                for r in results {
                    r?;
                    got += 1;
                }
                bar.tick(got, Some(missing.len() as u64));
            }
            bar.done(got);
        }
    }

    let required = version
        .java_version
        .as_ref()
        .map(|j| j.major_version)
        .unwrap_or(8);
    let (java, java_major) = java_of(&java_candidates(), required)?;

    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    vars.insert(
        "natives_directory".to_string(),
        native_dir.to_string_lossy().replace('\\', "/"),
    );
    vars.insert(
        "launcher_name".to_string(),
        "ehmodpack".to_string(),
    );
    vars.insert("launcher_version".to_string(), "1".to_string());
    vars.insert(
        "classpath".to_string(),
        classpath
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>()
            .join(if current_os() == "windows" { ";" } else { ":" }),
    );
    vars.insert(
        "library_directory".to_string(),
        base.join("libraries").to_string_lossy().replace('\\', "/"),
    );

    let mut jvm: Vec<String> = Vec::new();
    if let Some(arguments) = &version.arguments {
        for arg in evaluate(&arguments.jvm, &ctx) {
            jvm.push(template(&arg, &vars));
        }
    }
    if !jvm
        .iter()
        .any(|a| a == "-cp" || a.starts_with("-cp ") || a.starts_with("--class-path"))
    {
        jvm.push("-cp".to_string());
        jvm.push(vars["classpath"].clone());
    }
    jvm.push(format!(
        "-Dminecraft.launcher.brand={}",
        vars["launcher_name"]
    ));
    for extra in crate::launcher::extra_jvm_args() {
        if let Argument::Plain(text) = extra {
            jvm.push(text);
        }
    }
    jvm.push(version.main_class.clone());

    Ok(Natives {
        java,
        java_major,
        libraries,
        assets,
        jvm,
        main_class: version.main_class,
    })
}

async fn download_native(
    http: &crate::http::Http,
    base: &Path,
    artifact: &Artifact,
    native_dir: &Path,
    extract: &Option<Extract>,
) -> Result<usize> {
    let jar = download_library(http, base, artifact).await?;
    let exclude = extract
        .as_ref()
        .map(|e| e.exclude.clone())
        .unwrap_or_default();
    extract_zip(&jar, native_dir, &exclude)
}

pub fn extract_zip(jar: &Path, dest: &Path, exclude: &[String]) -> Result<usize> {
    let file = fs::File::open(jar)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut written = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        if exclude.iter().any(|e| name.starts_with(e.as_str())) {
            continue;
        }
        let target = dest.join(&name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data)?;
        fs::write(&target, data)?;
        written += 1;
    }
    Ok(written)
}

async fn flatten<F>(
    vanilla: &VersionJson,
    loader: &VersionJson,
    log: &mut F,
) -> Result<VersionJson>
where
    F: FnMut(&str),
{
    let mut out = vanilla.clone();
    out.id = loader.id.clone();
    out.inherits_from = loader.inherits_from.clone().or_else(|| {
        if loader.id == vanilla.id {
            None
        } else {
            Some(vanilla.id.clone())
        }
    });
    out.main_class = loader.main_class.clone();
    out.libraries.extend(loader.libraries.iter().cloned());
    if let Some(arguments) = &loader.arguments {
        let mut merged = out.arguments.unwrap_or_default();
        merged.jvm.extend(arguments.jvm.iter().cloned());
        merged.game.extend(arguments.game.iter().cloned());
        out.arguments = Some(merged);
    }
    log(&format!("{} libraries to fetch", out.libraries.len()));
    Ok(out)
}

pub fn spawn(natives: &Natives, dir: &Path) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new(&natives.java);
    for arg in split_jvm(&natives.jvm) {
        if arg.contains("${") {
            continue;
        }
        cmd.arg(arg);
    }
    let child = cmd
        .arg("--gameDir")
        .arg(dir)
        .arg("--assetsDir")
        .arg(dir.join(".minecraft-ehmodpack/assets"))
        .arg("--version")
        .arg(&natives.main_class)
        .arg("--accessToken")
        .arg("0")
        .arg("--userType")
        .arg("legacy")
        .arg("--username")
        .arg("Player")
        .arg("--uuid")
        .arg("00000000000000000000000000000000")
        .arg("--versionType")
        .arg("release")
        .current_dir(dir)
        .spawn()
        .with_context(|| format!("could not start {}", natives.java.display()))?;
    Ok(child)
}

pub struct Ctx {
    pub os_name: String,
    pub arch: String,
    pub features: BTreeMap<String, bool>,
}

impl Default for Ctx {
    fn default() -> Self {
        Self {
            os_name: current_os().to_string(),
            arch: current_arch().to_string(),
            features: BTreeMap::new(),
        }
    }
}

fn rule_passes(rule: &Rule, ctx: &Ctx) -> bool {
    if let Some(features) = &rule.features {
        for (name, want) in features {
            if ctx.features.get(name).copied().unwrap_or(false) != *want {
                return false;
            }
        }
    }
    match &rule.os {
        None => true,
        Some(os) => {
            let name_ok = match &os.name {
                None => true,
                Some(name) => {
                    name == "*" || name.eq_ignore_ascii_case(&ctx.os_name)
                }
            };
            let arch_ok = match &os.arch {
                Some(a) => a == &ctx.arch,
                None => true,
            };
            name_ok && arch_ok
        }
    }
}

fn rules_allow(rules: &[Rule], ctx: &Ctx) -> bool {
    let mut allowed = false;
    for rule in rules {
        if rule_passes(rule, ctx) {
            allowed = rule.action.as_deref() != Some("disallow");
        }
    }
    allowed
}

pub fn evaluate(args: &[Argument], ctx: &Ctx) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Argument::Plain(text) => out.push(text.clone()),
            Argument::Ruled { rules, value } => {
                if !rules_allow(rules, ctx) {
                    continue;
                }
                match value {
                    serde_json::Value::String(text) => out.push(text.clone()),
                    serde_json::Value::Array(items) => {
                        for item in items {
                            match item {
                                serde_json::Value::String(text) => out.push(text.clone()),
                                serde_json::Value::Object(_) => {
                                    if let Ok(nested) = serde_json::from_value::<Argument>(
                                        item.clone(),
                                    ) {
                                        out.extend(evaluate(&[nested], ctx));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

pub fn java_of(candidates: &[PathBuf], required_major: u32) -> Result<(PathBuf, u32)> {
    let mut seen = Vec::new();
    for dir in candidates {
        let exe = dir.join("bin").join(if current_os() == "windows" {
            "java.exe"
        } else {
            "java"
        });
        if exe.is_file() && !seen.contains(&exe) {
            seen.push(exe.clone());
            let output = std::process::Command::new(&exe)
                .arg("-version")
                .output()
                .with_context(|| format!("could not run {}", exe.display()))?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            if let Some(major) = parse_java_major(&text) {
                if major >= required_major {
                    return Ok((exe, major));
                }
            }
        }
    }
    bail!(
        "no java {required_major} or newer was found, ehmodpack looked in {}",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub fn parse_java_major(text: &str) -> Option<u32> {
    let quoted: Vec<&str> = text.split('"').collect();
    for chunk in quoted.iter().skip(1).step_by(2) {
        let mut parts = chunk.split(['.', '_', '-']);
        let first = parts.next()?;
        if let Ok(major) = first.parse::<u32>() {
            if major == 1 {
                return parts.next().and_then(|p| p.parse().ok());
            }
            return Some(major);
        }
    }
    for word in text.split_whitespace() {
        if let Ok(v) = word.trim_matches(|c: char| !c.is_ascii_digit() && c != '.').parse::<f64>() {
            if v > 1.0 {
                return Some(v as u32);
            }
        }
    }
    None
}

pub fn java_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(home) = std::env::var("JAVA_HOME") {
        out.push(PathBuf::from(home));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.file_name().is_some_and(|n| n == "bin") {
                if let Some(parent) = dir.parent() {
                    out.push(parent.to_path_buf());
                }
            }
        }
    }
    for base in [
        "C:/Program Files/Java",
        "C:/Program Files/Eclipse Adoptium",
        "C:/Program Files/Microsoft",
        "C:/Program Files/Zulu",
    ] {
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                out.push(entry.path());
            }
        }
    }
    out
}

pub fn sha1_hex(body: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(body);
    format!("{:x}", hasher.finalize())
}

pub fn asset_path(dir: &Path, hash: &str) -> PathBuf {
    let mut out = dir.join("assets").join("objects");
    out.push(&hash[..2]);
    out.push(hash);
    out
}

pub async fn asset_index(
    http: &crate::http::Http,
    dir: &Path,
    index: &AssetIndexRef,
) -> Result<BTreeMap<String, String>> {
    let out = dir.join("assets").join("indexes").join(format!("{}.json", index.id));
    if out.is_file() {
        let text = std::fs::read_to_string(&out)?;
        return Ok(parse_asset_index(&text));
    }
    let url = if index.url.is_empty() {
        let id = &index.id;
        format!("{RESOURCE_PACK}/{id}.json")
    } else {
        index.url.clone()
    };
    let body = http.get_bytes(&url).await?;
    if !index.sha1.is_empty() && sha1_hex(&body) != index.sha1 {
        let id = &index.id;
        bail!("the asset index {id} did not match its sha1");
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, &body)?;
    Ok(parse_asset_index(&String::from_utf8_lossy(&body)))
}

pub fn parse_asset_index(text: &str) -> BTreeMap<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
    let mut out = BTreeMap::new();
    if let Some(objects) = value.get("objects").and_then(|o| o.as_object()) {
        for (name, entry) in objects {
            if let Some(hash) = entry.get("hash").and_then(|h| h.as_str()) {
                out.insert(name.clone(), hash.to_string());
            }
        }
    }
    out
}

pub fn template(value: &str, vars: &BTreeMap<String, String>) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '}') {
                let key: String = chars[i + 2..i + 1 + end].iter().collect();
                if let Some(found) = vars.get(&key) {
                    out.push_str(found);
                    i += end + 2;
                    continue;
                }
            }
        }
        if chars[i] == '{' {
            if let Some(end) = chars[i..].iter().position(|c| *c == '}') {
                let key: String = chars[i + 1..i + end].iter().collect();
                if let Some(found) = vars.get(&key) {
                    out.push_str(found);
                    i += end + 1;
                    continue;
                }
                if key.contains(':') {
                    i += end + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> BTreeMap<String, String> {
        let mut v = BTreeMap::new();
        v.insert("natives_directory".to_string(), "C:/n".to_string());
        v.insert("launcher_name".to_string(), "ehmodpack".to_string());
        v.insert("classpath".to_string(), "a.jar;b.jar".to_string());
        v.insert("missing".to_string(), String::new());
        v
    }

    #[test]
    fn substitutes_plain_placeholders() {
        assert_eq!(
            template("-Djava.library.path=${natives_directory}", &vars()),
            "-Djava.library.path=C:/n"
        );
        assert_eq!(template("-cp {classpath}", &vars()), "-cp a.jar;b.jar");
    }

    #[test]
    fn leaves_unknown_placeholders_alone() {
        assert_eq!(template("{unknown_thing}", &vars()), "{unknown_thing}");
    }

    #[test]
    fn a_placeholder_with_no_value_becomes_empty() {
        assert_eq!(template("[{missing}]", &vars()), "[]");
    }

    #[test]
    fn a_rule_with_a_colon_is_dropped_not_substituted() {
        assert_eq!(template("{rules:a}", &vars()), "");
    }

    #[test]
    fn an_unknown_dollar_placeholder_is_left_alone() {
        assert_eq!(template("${nope}", &vars()), "${nope}");
    }

    #[test]
    fn a_four_part_coordinate_keeps_the_version_not_the_classifier() {
        let lib = Library {
            name: "org.lwjgl:lwjgl:3.3.3:natives-windows".to_string(),
            downloads: None,
            rules: None,
            natives: None,
            extract: None,
        };
        assert_eq!(lib.group_artifact(), "org.lwjgl:lwjgl:3.3.3");
    }

    #[test]
    fn library_names_split_into_path() {
        let lib = Library {
            name: "org.lwjgl:lwjgl:3.3.3:natives-windows".to_string(),
            downloads: None,
            rules: None,
            natives: None,
            extract: None,
        };
        assert_eq!(lib.group_artifact(), "org.lwjgl:lwjgl:3.3.3");
    }

    #[test]
    fn library_rules_gate_by_os() {
        let lib = Library {
            name: "x:y:1".to_string(),
            downloads: None,
            rules: Some(vec![Rule {
                action: None,
                os: Some(Os {
                    name: Some("windows".to_string()),
                    version: None,
                    arch: None,
                }),
                features: None,
            }]),
            natives: None,
            extract: None,
        };
        assert!(lib.allowed("windows", "x86_64"));
        assert!(!lib.allowed("linux", "x86_64"));
    }

    #[test]
    fn a_library_with_no_rules_is_always_allowed() {
        let lib = Library {
            name: "x:y:1".to_string(),
            downloads: None,
            rules: None,
            natives: None,
            extract: None,
        };
        assert!(lib.allowed("linux", "x86_64"));
    }

    fn real_1_21_11_args() -> Vec<Argument> {
        serde_json::from_str::<Arguments>(
            r#"{
              "jvm": [
                {"rules": [{"action": "allow", "os": {"arch": "x86"}}], "value": "-Xss1M"},
                "-Djava.library.path=${natives_directory}",
                {"rules": [{"os": {"name": "windows"}}], "value": "-Dos.name=Windows 10"},
                {
                  "rules": [
                    {"action": "allow"},
                    {"action": "disallow", "os": {"name": "osx"}}
                  ],
                  "value": "-XnoX11"
                }
              ],
              "game": ["--demo", "--username", "${auth_player_name}"]
            }"#,
        )
        .unwrap()
        .jvm
    }

    #[test]
    fn a_rule_whose_os_has_no_name_still_parses() {
        let args = real_1_21_11_args();
        assert_eq!(args.len(), 4);
    }

    #[test]
    fn an_arch_rule_of_x86_does_not_fire_on_x86_64() {
        let mut ctx = Ctx::default();
        ctx.arch = "x86_64".to_string();
        let out = evaluate(&real_1_21_11_args(), &ctx);
        assert!(!out.contains(&"-Xss1M".to_string()), "{out:?}");
    }

    #[test]
    fn an_arch_rule_of_x86_does_fire_on_32_bit() {
        let mut ctx = Ctx::default();
        ctx.arch = "x86".to_string();
        let out = evaluate(&real_1_21_11_args(), &ctx);
        assert!(out.contains(&"-Xss1M".to_string()), "{out:?}");
    }

    #[test]
    fn a_windows_rule_only_fires_on_windows() {
        let mut linux = Ctx::default();
        linux.os_name = "linux".to_string();
        assert!(!evaluate(&real_1_21_11_args(), &linux)
            .contains(&"-Dos.name=Windows 10".to_string()));
        let mut windows = Ctx::default();
        windows.os_name = "windows".to_string();
        assert!(evaluate(&real_1_21_11_args(), &windows)
            .contains(&"-Dos.name=Windows 10".to_string()));
    }

    #[test]
    fn a_disallow_rule_removes_the_argument() {
        let mut mac = Ctx::default();
        mac.os_name = "osx".to_string();
        let out = evaluate(&real_1_21_11_args(), &mac);
        assert!(!out.contains(&"-XnoX11".to_string()), "{out:?}");
        let mut linux = Ctx::default();
        linux.os_name = "linux".to_string();
        let out = evaluate(&real_1_21_11_args(), &linux);
        assert!(out.contains(&"-XnoX11".to_string()), "{out:?}");
    }

    #[test]
    fn a_ruled_value_that_is_an_array_expands_to_several_arguments() {
        let args: Vec<Argument> = serde_json::from_str(
            r#"[{"rules":[{"action":"allow","features":{"has_custom_resolution":true}}],"value":["--width","${resolution_width}"]}]"#,
        )
        .unwrap();
        let mut ctx = Ctx::default();
        ctx.features.insert("has_custom_resolution".to_string(), true);
        let out = evaluate(&args, &ctx);
        assert_eq!(out, vec!["--width".to_string(), "${resolution_width}".to_string()]);
    }

    #[test]
    fn mojang_camel_case_keys_parse() {
        let text = r#"{
          "id": "1.21.11",
          "type": "release",
          "mainClass": "net.minecraft.client.main.Main",
          "javaVersion": { "component": "java-runtime-gamma", "majorVersion": 21 },
          "assetIndex": { "id": "19", "sha1": "aa", "size": 1, "totalSize": 2 },
          "assets": "19",
          "libraries": [
            {
              "name": "org.lwjgl:lwjgl:3.3.3:natives-windows",
              "downloads": { "artifact": { "path": "a/b.jar", "sha1": "cc", "size": 3, "url": "http://x" } },
              "extract": { "exclude": ["META-INF/"] },
              "natives": { "windows": "natives-windows" },
              "rules": [ { "action": "allow", "os": { "name": "windows", "version": "^10\\." } } ]
            }
          ],
          "arguments": { "game": ["--demo"], "jvm": [] }
        }"#;
        let v: VersionJson = serde_json::from_str(text).expect("should parse");
        assert_eq!(v.main_class, "net.minecraft.client.main.Main");
        assert_eq!(v.java_version.unwrap().major_version, 21);
        assert_eq!(v.asset_index.unwrap().total_size, 2);
        assert_eq!(v.libraries.len(), 1);
        assert_eq!(v.libraries[0].path(), Some("a/b.jar"));
        assert_eq!(
            v.libraries[0].native_classifier("windows"),
            Some(&"natives-windows".to_string())
        );
        assert!(v.libraries[0].allowed("windows", "x86_64"));
        assert!(!v.libraries[0].allowed("linux", "x86_64"));
    }

    #[test]
    fn a_fabric_style_version_json_inherits_and_names_knot() {
        let text = r#"{
          "id": "1.21.11-fabric-0.19.5",
          "inheritsFrom": "1.21.11",
          "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
          "libraries": [ { "name": "net.fabricmc:fabric-loader:0.19.5" } ],
          "arguments": { "jvm": [ "-DFabricMcEmu= net.minecraft.client.main.Main" ], "game": [] }
        }"#;
        let v: VersionJson = serde_json::from_str(text).expect("should parse");
        assert_eq!(v.inherits_from.as_deref(), Some("1.21.11"));
        assert!(v.main_class.ends_with("KnotClient"));
        assert!(!v
            .arguments
            .unwrap()
            .jvm
            .iter()
            .collect::<Vec<_>>()
            .is_empty());
    }

    #[test]
    fn the_version_manifest_is_camel_case_too() {
        let text = r#"{
          "latest": { "release": "1.21.11", "snapshot": "26w" },
          "versions": [
            { "id": "1.21.11", "type": "release", "url": "http://x/1.21.11.json", "releaseTime": "2026-01-01T00:00:00Z" }
          ]
        }"#;
        let m: VersionManifest = serde_json::from_str(text).expect("should parse");
        assert_eq!(m.latest.release, "1.21.11");
        assert_eq!(m.versions[0].release_time, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn asset_index_is_flattened_to_name_hash() {
        let text = r#"{"objects":{"minecraft/sounds/x.ogg":{"hash":"abc123","size":1}}}"#;
        let flat = parse_asset_index(text);
        assert_eq!(flat.get("minecraft/sounds/x.ogg").map(String::as_str), Some("abc123"));
    }

    #[test]
    fn a_junk_asset_index_is_empty_rather_than_a_panic() {
        assert!(parse_asset_index("not json").is_empty());
    }

    #[test]
    fn a_fabric_profile_keeps_pointing_at_the_vanilla_game() {
        let vanilla: VersionJson = serde_json::from_str(
            r#"{"id":"1.21.11","mainClass":"net.minecraft.client.main.Main","libraries":[]}"#,
        )
        .unwrap();
        let loader: VersionJson = serde_json::from_str(
            r#"{"id":"1.21.11-fabric-0.19.5","inheritsFrom":"1.21.11","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient","libraries":[{"name":"net.fabricmc:fabric-loader:0.19.5"}]}"#,
        )
        .unwrap();
        let mut log = |_: &str| {};
        let merged = futures_block_on(flatten(&vanilla, &loader, &mut log)).unwrap();
        assert_eq!(merged.id, "1.21.11-fabric-0.19.5");
        assert_eq!(merged.inherits_from.as_deref(), Some("1.21.11"));
        assert_eq!(merged.libraries.len(), 1);
    }

    fn futures_block_on<F: std::future::Future>(mut f: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {}
        fn clone(p: *const ()) -> RawWaker {
            RawWaker::new(p, &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut f = unsafe { std::pin::Pin::new_unchecked(&mut f) };
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn a_space_inside_a_path_is_never_split() {
        let jvm = vec![
            "-Djava.library.path=C:/Users/x/Just Plots-mc-1/natives".to_string(),
            "-cp".to_string(),
            "a.jar;b.jar".to_string(),
            "net.minecraft.client.main.Main".to_string(),
        ];
        assert_eq!(
            split_jvm(&jvm),
            vec![
                "-Djava.library.path=C:/Users/x/Just Plots-mc-1/natives".to_string(),
                "-cp".to_string(),
                "a.jar;b.jar".to_string(),
                "net.minecraft.client.main.Main".to_string(),
            ]
        );
    }

    #[test]
    fn only_a_known_class_path_prefix_is_split() {
        let jvm = vec![
            "-cp C:/one two/three.jar".to_string(),
            "--class-path x.jar".to_string(),
            "-Xmx2G".to_string(),
        ];
        assert_eq!(
            split_jvm(&jvm),
            vec![
                "-cp".to_string(),
                "C:/one two/three.jar".to_string(),
                "--class-path".to_string(),
                "x.jar".to_string(),
                "-Xmx2G".to_string(),
            ]
        );
    }
    #[test]
    fn library_file_lands_under_libraries() {
        let got = library_file(Path::new("/tmp"), "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar");
        assert!(got.ends_with("libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar"));
    }
}