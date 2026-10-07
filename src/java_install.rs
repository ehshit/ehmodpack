use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

pub const VENDORS: &[(&str, &str)] = &[
    ("eclipse-adoptium", "Eclipse Adoptium (Temurin)"),
    ("azul-zulu", "Azul Zulu"),
    ("amazon-corretto", "Amazon Corretto"),
];

pub fn default_vendor() -> &'static str {
    "eclipse-adoptium"
}

pub fn normalise_vendor(input: &str) -> Result<&'static str> {
    let v = input.trim().to_ascii_lowercase();
    let id = match v.as_str() {
        "eclipse-adoptium" | "adoptium" | "temurin" | "eclipse" => "eclipse-adoptium",
        "azul-zulu" | "zulu" | "azul" => "azul-zulu",
        "amazon-corretto" | "corretto" | "amazon" => "amazon-corretto",
        other => bail!(
            "unknown java vendor {other}, have: {}",
            VENDORS
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    Ok(id)
}

fn arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("aarch64"),
        other => bail!("this cpu architecture ({other}) has no java download here"),
    }
}

fn archive_ext() -> &'static str {
    if std::env::consts::OS == "windows" {
        "zip"
    } else {
        "tar.gz"
    }
}

async fn url_for(http: &crate::http::Http, vendor: &str, major: u32) -> Result<String> {
    let arch = arch()?;
    match vendor {
        "azul-zulu" => {
            let os = match std::env::consts::OS {
                "windows" => "windows",
                "macos" => "macos",
                _ => "linux_glibc",
            };
            let url = format!(
                "https://api.azul.com/metadata/v1/zulu/packages/?java_version={major}&os={os}&arch={arch}&archive_type={}&java_package_type=jdk&javafx_bundled=false&release_status=ga&availability_types=ca&page=1&page_size=100",
                archive_ext()
            );
            let body = http
                .get_bytes(&url)
                .await
                .context("could not ask azul for java")?;
            let list: serde_json::Value =
                serde_json::from_slice(&body).context("azul answered something unreadable")?;
            let empty: Vec<serde_json::Value> = Vec::new();
            let arr = list.as_array().unwrap_or(&empty);
            let pick = arr
                .iter()
                .find(|e| e.get("latest").and_then(|v| v.as_bool()) == Some(true))
                .or_else(|| arr.first());
            let Some(entry) = pick else {
                bail!("azul has no java {major} for this system");
            };
            let Some(dl) = entry.get("download_url").and_then(|v| v.as_str()) else {
                bail!("azul did not say where to download");
            };
            Ok(dl.to_string())
        }
        "amazon-corretto" => {
            let os = match std::env::consts::OS {
                "windows" => "windows",
                "macos" => "macos",
                _ => "linux",
            };
            Ok(format!(
                "https://corretto.aws/downloads/latest/amazon-corretto-{major}-{arch}-{os}-jdk.{}",
                archive_ext()
            ))
        }
        _ => {
            let os = match std::env::consts::OS {
                "windows" => "windows",
                "macos" => "mac",
                _ => "linux",
            };
            Ok(format!(
                "https://api.adoptium.net/v3/binary/latest/{major}/ga/{os}/{arch}/jdk/hotspot/normal/eclipse"
            ))
        }
    }
}

pub fn install_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ehmodpack")
        .join("java")
}

pub fn find_home_under(dir: &Path) -> Option<PathBuf> {
    if crate::minecraft::java_exe_in(dir).is_some() {
        return Some(dir.to_path_buf());
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if crate::minecraft::java_exe_in(&path).is_some() {
            return Some(path);
        }
    }
    None
}

fn extract(archive: &Path, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into)?;
    if archive.to_string_lossy().ends_with(".zip") {
        let file = std::fs::File::open(archive)?;
        let mut zip = zip::ZipArchive::new(file).context("the java archive is not a readable zip")?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(rel) = entry.enclosed_name() else {
                continue;
            };
            let out = into.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut file)?;
        }
    } else {
        let file = std::fs::File::open(archive)?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        tar.unpack(into)
            .context("the java archive is not a readable tar.gz")?;
    }
    Ok(())
}

pub async fn install<F>(
    http: &crate::http::Http,
    vendor: &str,
    major: u32,
    mut log: F,
) -> Result<PathBuf>
where
    F: FnMut(&str),
{
    let root = install_root();
    let dest = root.join(format!("{vendor}-{major}"));
    if let Some(home) = find_home_under(&dest)
        && let Some((_, found)) = crate::minecraft::java_at_home(&home)
        && found >= major
    {
        log(&format!("java {major} is already installed at {}", home.display()));
        return Ok(home);
    }
    let url = url_for(http, vendor, major).await?;
    log(&format!("java {major} from {vendor}"));
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    std::fs::create_dir_all(&dest)?;
    let archive = root.join(format!("{vendor}-{major}.{}", archive_ext()));
    http.download(&url, &archive, "fetching java").await?;
    extract(&archive, &dest)?;
    let _ = std::fs::remove_file(&archive);
    let Some(home) = find_home_under(&dest) else {
        bail!(
            "the java download did not contain a usable java at {}",
            dest.display()
        );
    };
    if crate::minecraft::java_at_home(&home).is_none() {
        bail!("the java at {} does not run", home.display());
    }
    log(&format!("installed to {}", home.display()));
    Ok(home)
}

fn append_line(file: &Path, line: &str) -> Result<()> {
    let mut text = std::fs::read_to_string(file).unwrap_or_default();
    if text.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, text)
        .with_context(|| format!("could not write {}", file.display()))
}

pub fn set_java_home(home: &Path, add_path: bool) -> Result<bool> {
    let home_str = home.to_string_lossy().into_owned();
    let bin = home.join("bin");
    if std::env::consts::OS == "windows" {
        let out = Command::new("setx")
            .args(["JAVA_HOME", &home_str])
            .output()
            .context("could not run setx")?;
        if !out.status.success() {
            bail!(
                "setx refused to set JAVA_HOME: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    } else {
        let Some(user) = dirs::home_dir() else {
            bail!("no home directory, cannot set JAVA_HOME");
        };
        let mut files: Vec<PathBuf> = vec![user.join(".profile")];
        for name in [".bashrc", ".zshrc"] {
            let f = user.join(name);
            if f.exists() {
                files.push(f);
            }
        }
        for f in &files {
            append_line(f, &format!("export JAVA_HOME=\"{home_str}\""))?;
        }
    }
    if add_path {
        let report = onpath::add(&bin, "ehmodpack-java")
            .context("could not add java to your PATH")?;
        println!("{report}");
    }
    Ok(add_path)
}