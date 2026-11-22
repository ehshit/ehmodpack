use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use inquire::Select;
use serde::{Deserialize, Serialize};

use crate::{Cmd, i18n};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct LocaleConfig {
    locale: Option<String>,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetUserDefaultLocaleName(lp_locale_name: *mut u16, cch_locale_name: i32) -> i32;
}

pub fn prepare(cli: &crate::Cli) -> Result<()> {
    let path = config_path()?;
    if let Some(config) = read_config(&path)? {
        if let Some(locale) = config.locale.as_deref() {
            i18n::set_locale(locale);
        }
        return Ok(());
    }

    if matches!(cli.cmd, Cmd::Locale { .. })
        || !std::io::stdin().is_terminal()
        || env::var_os("CI").is_some()
    {
        return Ok(());
    }

    onboard(&path)
}

pub fn list() {
    let current = i18n::current_locale();
    println!("{}", i18n::t("locale-list-header"));
    println!(
        "{:<28} {}",
        i18n::t("locale-list-name"),
        i18n::t("locale-list-code")
    );
    for id in i18n::bundled_locales() {
        let marker = if id == current {
            format!("  {}", i18n::t("locale-current-marker"))
        } else {
            String::new()
        };
        println!("{:<28} {}{}", display_name(id), id, marker);
    }
}

pub fn set(raw: &str) -> Result<()> {
    let Some(locale) = i18n::match_locale(raw) else {
        bail!("{}", i18n::t_args("locale-not-found", &[("locale", raw)]));
    };
    let path = config_path()?;
    i18n::set_locale(locale);
    write_config(&path, Some(locale))?;
    println!("{}", i18n::t("locale-saved"));
    Ok(())
}

fn onboard(path: &Path) -> Result<()> {
    let detected = detected_locale();
    let clean = detected.as_deref().map(clean_tag);

    if let Some(tag) = clean.as_deref()
        && let Some(matched) = i18n::match_locale(tag)
    {
        if matched.eq_ignore_ascii_case("en-US") {
            return Ok(());
        }

        let prompt = i18n::t_args(
            "locale-detected",
            &[("lang", display_name(matched).as_str())],
        );
        if inquire::Confirm::new(&prompt)
            .with_default(false)
            .prompt()
            .unwrap_or(false)
        {
            i18n::set_locale(matched);
            write_config(path, Some(matched))?;
            println!("{}", i18n::t("locale-saved"));
        } else {
            write_config(path, None)?;
            println!(
                "{}",
                i18n::t_args("locale-change-later", &[("locale", matched)])
            );
        }
        return Ok(());
    }

    let shown = clean.clone().unwrap_or_else(|| i18n::t("locale-unknown"));
    let prompt = i18n::t_args("locale-detected-missing", &[("lang", shown.as_str())]);
    if inquire::Confirm::new(&prompt)
        .with_default(false)
        .prompt()
        .unwrap_or(false)
    {
        i18n::set_locale("en-US");
        write_config(path, Some("en-US"))?;
        println!("{}", i18n::t("locale-saved"));
        return Ok(());
    }

    let locales = i18n::bundled_locales();
    let labels: Vec<String> = locales.iter().map(|id| pick_label(id)).collect();
    let chosen = Select::new(&i18n::t("locale-pick"), labels.clone())
        .with_page_size(12)
        .prompt()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let Some(index) = labels.iter().position(|label| label == &chosen) else {
        bail!("{}", i18n::t("locale-not-picked"));
    };
    let locale = locales[index];
    i18n::set_locale(locale);
    write_config(path, Some(locale))?;
    println!("{}", i18n::t("locale-saved"));
    Ok(())
}

fn config_path() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("EHMODPACK_CONFIG_DIR") {
        return Ok(PathBuf::from(dir).join("locale.json"));
    }
    let base = dirs::config_dir().context(i18n::t("locale-config-dir-missing"))?;
    Ok(base.join("ehmodpack").join("locale.json"))
}

fn read_config(path: &Path) -> Result<Option<LocaleConfig>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            let detail = e.to_string();
            return Err(e).with_context(|| {
                i18n::t_args("locale-config-read-failed", &[("error", detail.as_str())])
            });
        }
    };
    let config = serde_json::from_str(&raw).unwrap_or_default();
    Ok(Some(config))
}

fn write_config(path: &Path, locale: Option<&str>) -> Result<()> {
    let config = LocaleConfig {
        locale: locale.map(str::to_string),
    };
    let body =
        serde_json::to_string_pretty(&config).context(i18n::t("locale-config-encode-failed"))?;
    crate::manifest::write_file(path, &body).context(i18n::t("locale-config-write-failed-generic"))
}

fn detected_locale() -> Option<String> {
    if let Ok(raw) = env::var("EHMODPACK_LANG")
        && !raw.trim().is_empty()
    {
        return Some(raw);
    }
    os_locale()
}

#[cfg(windows)]
fn os_locale() -> Option<String> {
    const LOCALE_NAME_MAX_LENGTH: usize = 85;
    let mut buffer = [0u16; LOCALE_NAME_MAX_LENGTH];
    let length =
        unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), LOCALE_NAME_MAX_LENGTH as i32) };
    if length <= 1 {
        return None;
    }
    Some(String::from_utf16_lossy(
        &buffer[..(length as usize).saturating_sub(1)],
    ))
}

#[cfg(target_os = "macos")]
fn os_locale() -> Option<String> {
    use std::process::Command;

    let output = Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let line = line.trim().trim_end_matches(',').trim();
        let Some(tag) = line.strip_prefix('"') else {
            continue;
        };
        let Some((tag, _)) = tag.split_once('"') else {
            continue;
        };
        if !tag.is_empty() {
            return Some(tag.to_string());
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn os_locale() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(raw) = env::var(key)
            && !raw.trim().is_empty()
        {
            return Some(raw);
        }
    }
    let text = fs::read_to_string("/etc/locale.conf").ok()?;
    for line in text.lines() {
        let line = line.trim();
        let Some(value) = line.strip_prefix("LANG=") else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn os_locale() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(raw) = env::var(key)
            && !raw.trim().is_empty()
        {
            return Some(raw);
        }
    }
    None
}

fn clean_tag(raw: &str) -> String {
    let head = raw.trim().split(['.', '@']).next().unwrap_or(raw);
    head.replace('_', "-")
}

fn pick_label(id: &str) -> String {
    format!("{}  {}", display_name(id), id)
}

const DISPLAY_NAMES: &[(&str, &str)] = &[
    ("en-US", "English (United States)"),
    // ("es-419", "Spanish (Latin America)"),
    // ("es-ES", "Spanish"),
    // ("de-DE", "German"),
    // ("fr-FR", "French"),
    // ("pt-BR", "Portuguese (Brazil)"),
    // ("ja-JP", "Japanese"),
    // ("zh-CN", "Chinese (Simplified)"),
    // ("ru-RU", "Russian"),
    // ("cs-CZ", "Czech"),
    // ("da-DK", "Danish"),
    // ("de-CH", "German (Switzerland)"),
    // ("fi-FI", "Finnish"),
    // ("fil-PH", "Filipino"),
    // ("he-IL", "Hebrew"),
    // ("hu-HU", "Hungarian"),
    // ("id-ID", "Indonesian"),
    // ("it-IT", "Italian (Italy)"),
    // ("ko-KR", "Korean"),
    // ("ms-MY", "Malay"),
    // ("nl-NL", "Dutch"),
    // ("no-NO", "Norwegian Bokmal"),
    // ("pl-PL", "Polish"),
    // ("pt-PT", "Portuguese (Portugal)"),
    // ("ro-RO", "Romanian"),
    // ("sr-CS", "Serbian (Latin)"),
    // ("sv-SE", "Swedish"),
    // ("th-TH", "Thai"),
    // ("tr-TR", "Turkish"),
    // ("uk-UA", "Ukrainian"),
    // ("vi-VN", "Vietnamese"),
    // ("zh-TW", "Chinese (Traditional)"),
];

fn display_name(id: &str) -> String {
    let tag = clean_tag(id);
    DISPLAY_NAMES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(&tag))
        .map(|(_, name)| (*name).to_string())
        .unwrap_or(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_tag_normalizes_common_locale_shapes() {
        assert_eq!(clean_tag("es_ES.UTF-8@x"), "es-ES");
        assert_eq!(clean_tag("zh-Hans-CN"), "zh-Hans-CN");
    }

    #[test]
    fn display_names_come_from_the_small_modrinth_list() {
        assert_eq!(display_name("en-US"), "English (United States)");
        assert_eq!(display_name("pt_BR"), "pt-BR");
        assert_eq!(display_name("ru-RU"), "ru-RU");
        assert_eq!(display_name("cs-CZ"), "cs-CZ");
        assert_eq!(display_name("zz-ZZ"), "zz-ZZ");
    }

    #[test]
    fn pick_label_contains_display_name_and_literal_locale() {
        assert_eq!(pick_label("en-US"), "English (United States)  en-US");
    }
}