use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
pub const QUILT_META: &str = "https://meta.quiltmc.org/v3";
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
    Quilt,
    #[value(name = "neoforge")]
    NeoForge,
}

impl LoaderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::NeoForge => "neoforge",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Fabric, Self::Quilt, Self::NeoForge]
    }

    pub fn parse(text: &str) -> Option<Self> {
        let want = text.trim().to_ascii_lowercase();
        Self::all()
            .iter()
            .copied()
            .find(|k| k.as_str() == want)
    }

    fn base(self) -> &'static str {
        match self {
            Self::Fabric => FABRIC_META,
            Self::Quilt => QUILT_META,
            Self::NeoForge => NEOFORGE_MAVEN,
        }
    }

    fn env(self) -> &'static str {
        match self {
            Self::Fabric => "EHMODPACK_FABRIC_META",
            Self::Quilt => "EHMODPACK_QUILT_META",
            Self::NeoForge => "EHMODPACK_NEOFORGE_MAVEN",
        }
    }
}

impl std::fmt::Display for LoaderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderInfo {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub stable: bool,
    #[serde(default)]
    pub build: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetaEntry {
    #[serde(default)]
    pub loader: Option<LoaderInfo>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub stable: Option<bool>,
    #[serde(default)]
    pub build: Option<u32>,
}

impl MetaEntry {
    pub fn info(&self) -> Option<LoaderInfo> {
        if let Some(loader) = &self.loader {
            return Some(loader.clone());
        }
        self.version.clone().map(|version| LoaderInfo {
            version,
            stable: self.stable.unwrap_or(false),
            build: self.build.unwrap_or(0),
        })
    }
}

pub fn newest(infos: &[LoaderInfo]) -> Option<String> {
    let mut best: Option<&LoaderInfo> = None;
    let mut best_key: Option<semver::Version> = None;
    for info in infos.iter().filter(|i| i.stable) {
        let Some(key) = semver::Version::parse(&info.version).ok() else {
            continue;
        };
        if best_key.as_ref().is_none_or(|b| key > *b) {
            best_key = Some(key);
            best = Some(info);
        }
    }
    best.or_else(|| infos.first())
        .map(|i| i.version.clone())
}

pub async fn resolve(kind: LoaderKind, mc: &str) -> Result<String> {
    let base = std::env::var(kind.env()).unwrap_or_else(|_| kind.base().to_string());
    match kind {
        LoaderKind::NeoForge => from_maven(&base, mc).await,
        _ => from_meta(&base, mc).await,
    }
}

pub async fn from_meta(base: &str, mc: &str) -> Result<String> {
    let url = format!("{}/versions/loader/{}", base.trim_end_matches('/'), mc);
    let entries: Vec<MetaEntry> = get_json(&url).await?;
    let infos: Vec<LoaderInfo> = entries.iter().filter_map(MetaEntry::info).collect();
    newest(&infos).ok_or_else(|| anyhow::anyhow!("no loader version for mc {mc} at {url}"))
}

pub async fn from_maven(base: &str, mc: &str) -> Result<String> {
    let url = format!("{}/maven-metadata.xml", base.trim_end_matches('/'));
    let text = get_text(&url).await?;
    let prefix = format!("{}.", mc.strip_prefix("1.").unwrap_or(mc));
    let found: Vec<String> = text
        .split("<version>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</version>").next())
        .map(|v| v.trim().to_string())
        .filter(|v| v.starts_with(&prefix))
        .collect();
    if found.is_empty() {
        bail!("neoforge has no release for mc {mc}, looked for {prefix}*");
    }
    let infos: Vec<LoaderInfo> = found
        .iter()
        .map(|v| LoaderInfo {
            version: v.clone(),
            stable: true,
            build: 0,
        })
        .collect();
    newest(&infos).ok_or_else(|| anyhow::anyhow!("could not pick a neoforge version for {mc}"))
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    client()?
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?
        .error_for_status()
        .with_context(|| format!("GET {url} returned an error"))?
        .json()
        .await
        .with_context(|| format!("could not decode the response from {url}"))
}

async fn get_text(url: &str) -> Result<String> {
    client()?
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?
        .error_for_status()
        .with_context(|| format!("GET {url} returned an error"))?
        .text()
        .await
        .with_context(|| format!("could not read the response from {url}"))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(crate::modrinth::USER_AGENT)
        .build()
        .context("could not build the http client")?)
}

pub fn java_for(mc: &str) -> &'static str {
    let mut parts = mc.split('.');
    let _major = parts.next();
    let minor = parts.next().and_then(parse_u32).unwrap_or(0);
    let patch = parts.next().and_then(parse_u32).unwrap_or(0);
    if minor >= 21 {
        return "21";
    }
    if minor == 20 && patch >= 5 {
        return "21";
    }
    if minor >= 17 {
        return "17";
    }
    "8"
}

fn parse_u32(text: &str) -> Option<u32> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(version: &str, stable: bool) -> LoaderInfo {
        LoaderInfo {
            version: version.to_string(),
            stable,
            build: 0,
        }
    }

    #[test]
    fn picks_the_highest_stable_not_the_highest_build() {
        let entries = vec![
            LoaderInfo { build: 5, ..info("0.19.5", true) },
            LoaderInfo { build: 4, ..info("0.19.4", false) },
            LoaderInfo { build: 0, ..info("0.19.0", false) },
            LoaderInfo { build: 6, ..info("0.18.6", false) },
        ];
        assert_eq!(newest(&entries).as_deref(), Some("0.19.5"));
    }

    #[test]
    fn ignores_unstable_when_a_stable_one_exists() {
        let entries = vec![info("0.20.0", false), info("0.19.5", true)];
        assert_eq!(newest(&entries).as_deref(), Some("0.19.5"));
    }

    #[test]
    fn falls_back_to_the_first_when_nothing_is_stable() {
        let entries = vec![info("0.20.0", false), info("0.19.5", false)];
        assert_eq!(newest(&entries).as_deref(), Some("0.20.0"));
    }

    #[test]
    fn nothing_stable_and_nothing_at_all() {
        assert_eq!(newest(&[]), None);
    }

    #[test]
    fn skips_versions_it_cannot_parse() {
        let entries = vec![info("not-a-version", true), info("0.19.5", true)];
        assert_eq!(newest(&entries).as_deref(), Some("0.19.5"));
    }

    #[test]
    fn reads_the_nested_fabric_shape() {
        let entry: MetaEntry = serde_json::from_str(
            r#"{"loader":{"separator":".","build":5,"maven":"x","version":"0.19.5","stable":true},"intermediary":{"version":"1.21.11"}}"#,
        )
        .unwrap();
        let parsed = entry.info().unwrap();
        assert_eq!(parsed.version, "0.19.5");
        assert!(parsed.stable);
    }

    #[test]
    fn reads_the_flat_shape() {
        let entry: MetaEntry =
            serde_json::from_str(r#"{"version":"0.20.0","stable":true}"#).unwrap();
        let parsed = entry.info().unwrap();
        assert_eq!(parsed.version, "0.20.0");
        assert!(parsed.stable);
    }

    #[test]
    fn neoforge_drops_the_leading_one() {
        assert_eq!(
            parse_maven_versions(
                "<metadata><versioning><versions><version>21.11.4</version><version>21.11.3</version><version>21.10.0</version><version>20.4.0</version></versions></versioning></metadata>",
                "1.21.11"
            ),
            vec!["21.11.4".to_string(), "21.11.3".to_string()]
        );
    }

    #[test]
    fn java_follows_the_minecraft_version() {
        assert_eq!(java_for("1.21.11"), "21");
        assert_eq!(java_for("1.20.6"), "21");
        assert_eq!(java_for("1.20.4"), "17");
        assert_eq!(java_for("1.19.4"), "17");
        assert_eq!(java_for("1.16.5"), "8");
    }

    fn parse_maven_versions(xml: &str, mc: &str) -> Vec<String> {
    let prefix = format!("{}.", mc.strip_prefix("1.").unwrap_or(mc));
        xml.split("<version>")
            .skip(1)
            .filter_map(|chunk| chunk.split("</version>").next())
            .map(|v| v.trim().to_string())
            .filter(|v| v.starts_with(&prefix))
            .collect()
    }
}