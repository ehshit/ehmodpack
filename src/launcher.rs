use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::http::Http;
use crate::lock::LockedTarget;
use crate::minecraft::{self, Argument, Arguments, Library, VersionJson};

pub const FABRIC_MAVEN: &str = "https://maven.fabricmc.net";
pub const DEFAULT_SPONGE_MIXIN: &str = "0.17.4+mixin.0.8.7";
pub const DEFAULT_MIXINEXTRAS: &str = "0.5.5";
pub const DEFAULT_ASM: &str = "9.10.1";

pub struct Installed {
    pub version_id: String,
    pub version: VersionJson,
    pub raw: serde_json::Value,
}

pub async fn install<F>(
    http: &Http,
    base: &Path,
    target: &LockedTarget,
    mut log: F,
) -> Result<Installed>
where
    F: FnMut(&str),
{
    match target.loader.kind.as_str() {
        "fabric" => fabric(http, base, target, &mut log).await,
        other => bail!(
            "It only wires fabric up so far, {other} is not there yet sorry"
        ),
    }
}

async fn fabric<F>(
    http: &Http,
    base: &Path,
    target: &LockedTarget,
    log: &mut F,
) -> Result<Installed>
where
    F: FnMut(&str),
{
    let mc = &target.minecraft;
    let loader_version = &target.loader.version;
    let meta: serde_json::Value = http
        .get_json(&format!(
            "https://meta.fabricmc.net/v2/versions/loader/{mc}/{loader_version}"
        ))
        .await
        .with_context(|| format!("could not reach fabric meta for {mc} {loader_version}"))?;

    let loader_maven = meta
        .pointer("/loader/maven")
        .and_then(|v| v.as_str())
        .with_context(|| "fabric meta did not give a loader jar")?
        .to_string();
    let intermediary_maven = meta
        .pointer("/intermediary/maven")
        .and_then(|v| v.as_str())
        .with_context(|| "fabric meta did not give an intermediary jar")?
        .to_string();

    for maven in [
        loader_maven.as_str(),
        intermediary_maven.as_str(),
        sponge_mixin_maven().as_str(),
        mixinextras_maven().as_str(),
    ] {
        fetch_maven(http, base, maven, log).await?;
    }
    for artifact in asm_mavens() {
        fetch_maven(http, base, &artifact, log).await?;
    }

    let version_id = format!("{mc}-fabric-{loader_version}");
    let libraries: Vec<serde_json::Value> = [
        loader_maven.as_str(),
        intermediary_maven.as_str(),
        sponge_mixin_maven().as_str(),
        mixinextras_maven().as_str(),
    ]
    .into_iter()
    .map(|name| json!({ "name": name }))
    .chain(asm_mavens().into_iter().map(|name| json!({ "name": name })))
    .collect();

    let json = json!({
        "id": version_id,
        "inheritsFrom": mc,
        "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "libraries": libraries,
        "arguments": {
            "jvm": [
                "-DFabricMcEmu= net.minecraft.client.main.Main",
                "-Dnet.minecraft.launcher.brand=ehmodpack",
                "-Dnet.minecraft.launcher.version=1",
                "-Dfabric.development=false",
                "-Dfabric.side=client",
                {
                    "rules": [{ "os": { "name": "osx" } }],
                    "value": "-XstartOnFirstThread"
                },
                {
                    "rules": [{ "os": { "name": "windows" } }],
                    "value": "-Dos.name=Windows 10 -Dos.version=10.0"
                }
            ],
            "game": []
        }
    });
    let version: VersionJson = serde_json::from_value(json.clone())?;

    let out = base
        .join("versions")
        .join(&version_id)
        .join(format!("{version_id}.json"));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, serde_json::to_string_pretty(&json)?)
        .with_context(|| format!("could not write {}", out.display()))?;

    Ok(Installed {
        version_id,
        version,
        raw: json,
    })
}

fn sponge_mixin_maven() -> String {
    format!("net.fabricmc:sponge-mixin:{DEFAULT_SPONGE_MIXIN}")
}

fn mixinextras_maven() -> String {
    format!("io.github.llamalad7:mixinextras-fabric:{DEFAULT_MIXINEXTRAS}")
}

fn asm_mavens() -> Vec<String> {
    ["asm", "asm-analysis", "asm-commons", "asm-tree", "asm-util"]
        .iter()
        .map(|name| format!("org.ow2.asm:{name}:{DEFAULT_ASM}"))
        .collect()
}

pub fn maven_path(maven: &str) -> String {
    let parts: Vec<&str> = maven.split(':').collect();
    if parts.len() < 3 {
        return String::new();
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[parts.len() - 1]);
    format!(
        "{group}/{artifact}/{version}/{artifact}-{version}.jar",
        group = group.replace('.', "/")
    )
}

pub fn maven_url(maven: &str) -> String {
    format!("{FABRIC_MAVEN}/{path}", path = maven_path(maven))
}

async fn fetch_maven<F>(
    http: &Http,
    base: &Path,
    maven: &str,
    log: &mut F,
) -> Result<PathBuf>
where
    F: FnMut(&str),
{
    let path = maven_path(maven);
    if path.is_empty() {
        bail!("{maven} is not a maven coordinate");
    }
    let out = minecraft::library_file(base, &path);
    if out.is_file() {
        log(&format!("already have {maven}"));
        return Ok(out);
    }
    let url = maven_url(maven);
    http.download(&url, &out, &format!("fetching {maven}"))
        .await
        .with_context(|| format!("GET {url}"))?;
    log(&format!("got {maven}"));
    Ok(out)
}

pub fn loader_libraries(version: &VersionJson) -> Vec<Library> {
    version.libraries.clone()
}

pub fn extra_jvm_args() -> Vec<Argument> {
    vec![Argument::Plain("-Xmx2G".to_string())]
}

pub fn empty_arguments() -> Arguments {
    Arguments::default()
}