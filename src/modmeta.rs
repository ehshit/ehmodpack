use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, Deserialize)]
pub struct Depends {
    #[serde(default, alias = "modid", alias = "modId")]
    pub id: String,
    #[serde(default, alias = "versions", alias = "versionRange")]
    pub range: String,
    #[serde(default, alias = "mandatory")]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(untagged)]
enum DependsRaw {
    #[default]
    Nothing,
    Map(BTreeMap<String, String>),
    List(Vec<Depends>),
}

impl DependsRaw {
    fn into_vec(self) -> Vec<Depends> {
        match self {
            Self::Nothing => Vec::new(),
            Self::List(list) => list,
            Self::Map(map) => map
                .into_iter()
                .map(|(id, range)| Depends {
                    id,
                    range,
                    required: true,
                })
                .collect(),
        }
    }
}

fn dep_list<'de, D>(d: D) -> std::result::Result<Vec<Depends>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(DependsRaw::deserialize(d)?.into_vec())
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModMeta {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "dep_list")]
    pub depends: Vec<Depends>,
    #[serde(default, deserialize_with = "dep_list")]
    pub breaks: Vec<Depends>,
}

#[derive(Debug, Clone)]
pub struct Conflict {
    pub offender: String,
    pub other: String,
    pub wanted: String,
    pub found: String,
    pub kind: &'static str,
}

pub fn read_bytes(body: &[u8]) -> Result<Option<ModMeta>> {
    let cursor = std::io::Cursor::new(body);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(_) => return Ok(None),
    };
    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
        .collect();
    for wanted in [
        "fabric.mod.json",
        "quilt.mod.json",
        "META-INF/neoforge.mods.toml",
        "META-INF/mods.toml",
    ]
    .iter()
    {
        let hit = names
            .iter()
            .find(|n| n.as_str() == *wanted)
            .cloned()
            .or_else(|| {
                if *wanted == "fabric.mod.json" {
                    names
                        .iter()
                        .find(|n| n.to_ascii_lowercase().ends_with("fabric.mod.json"))
                        .cloned()
                } else {
                    names.iter().find(|n| n.ends_with(*wanted)).cloned()
                }
            });
        let Some(hit) = hit else { continue };
        let mut text = String::new();
        let Ok(mut entry) = archive.by_name(&hit) else {
            continue;
        };
        if entry.read_to_string(&mut text).is_err() {
            continue;
        }
        if let Some(meta) = parse(wanted, &text) {
            return Ok(Some(meta));
        }
    }
    Ok(None)
}

pub fn read(jar: &Path) -> Result<Option<ModMeta>> {
    let file = std::fs::File::open(jar)
        .with_context(|| format!("could not open {}", jar.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a readable jar", jar.display()))?;
    for name in [
        "fabric.mod.json",
        "quilt.mod.json",
        "META-INF/neoforge.mods.toml",
        "META-INF/mods.toml",
    ] {
        let mut text = String::new();
        let ok = match archive.by_name(name) {
            Ok(mut entry) => entry.read_to_string(&mut text).is_ok(),
            Err(_) => false,
        };
        if ok {
            if let Some(meta) = parse(name, &text) {
                return Ok(Some(meta));
            }
        }
    }
    Ok(None)
}

fn parse(kind: &str, text: &str) -> Option<ModMeta> {
    if kind.ends_with(".toml") {
        return parse_toml(text);
    }
    serde_json::from_str(text).ok()
}

fn parse_toml(text: &str) -> Option<ModMeta> {
    let mut id = String::new();
    let mut version = String::new();
    let mut name = String::new();
    let mut deps: Vec<String> = Vec::new();
    let mut in_deps = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_deps = line.contains("dependencies");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match (in_deps, key.trim()) {
            (false, "modId") => id = value,
            (false, "version") => version = value,
            (false, "displayName") => name = value,
            (true, "modId") => deps.push(value),
            _ => {}
        }
    }
    if id.is_empty() {
        return None;
    }
    Some(ModMeta {
        id,
        version,
        name,
        depends: deps
            .into_iter()
            .map(|d| Depends {
                id: d,
                range: String::new(),
                required: true,
            })
            .collect(),
        breaks: Vec::new(),
    })
}

pub fn fits_minecraft(range: &str, found: &str) -> bool {
    let range = range.trim();
    if range.is_empty() || range == "*" || range.contains("${") {
        return true;
    }
    if range.starts_with('~') {
        let want = version_tuple(&clean(range.trim_start_matches('~')));
        let have = version_tuple(&clean(found));
        let minor = want.len().min(2);
        return have.len() >= minor && have[..minor] == want[..minor];
    }
    satisfied(range, found)
}

pub fn satisfied(range: &str, found: &str) -> bool {
    let range = range.trim();
    if range.is_empty() || range == "*" {
        return true;
    }
    if range.contains("${") {
        return true;
    }
    let found = clean(found);
    if found.is_empty() {
        return false;
    }
    let bracketed = range.starts_with('[') || range.starts_with('(');
    let lower_inc = range.starts_with('[');
    let upper_inc = range.ends_with(']');
    let body = range
        .trim_start_matches(['[', '('])
        .trim_end_matches([']', ')']);
    let parts: Vec<&str> = body
        .split(&[',', ' '][..])
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();

    if bracketed {
        if let Some(idx) = body.find(',') {
            let left = body[..idx].trim();
            let right = body[idx + 1..].trim();
            let lower_ok = if left.is_empty() {
                true
            } else {
                edge_ok(&found, left, lower_inc, true)
            };
            let upper_ok = if right.is_empty() {
                true
            } else {
                edge_ok(&found, right, upper_inc, false)
            };
            return lower_ok && upper_ok;
        }
        if parts.len() == 1 {
            let want = parts[0];
            return edge_ok(&found, want, true, true) && edge_ok(&found, want, true, false);
        }
        return true;
    }

    parts
        .iter()
        .all(|c| {
            if let Some(rest) = c.strip_prefix('~') {
                return tilde_ok(&found, rest.trim());
            }
            let (op, edge) = split_op(c);
            bound_ok(&found, op, edge)
        })
}

fn tilde_ok(found: &str, edge: &str) -> bool {
    let have = version_tuple(found);
    let want = version_tuple(edge);
    if want.is_empty() {
        return true;
    }
    if have < want {
        return false;
    }
    let mut upper = want.clone();
    if let Some(last) = upper.last_mut() {
        *last += 1;
    }
    have < upper
}

fn edge_ok(found: &str, clause: &str, inclusive: bool, is_lower: bool) -> bool {
    let (op, edge) = split_op(clause);
    if edge.is_empty() {
        return true;
    }
    if let Some(want) = wildcard_prefix(edge) {
        let have = version_tuple(found);
        let eq = have.len() >= want.len() && have[..want.len()] == want[..];
        return if op == "!=" { !eq } else { eq };
    }
    let ord = version_tuple(found).cmp(&version_tuple(edge));
    match op {
        ">=" => ord.is_ge(),
        ">" => ord.is_gt(),
        "<=" => ord.is_le(),
        "<" => ord.is_lt(),
        "!=" => ord.is_ne(),
        _ => {
            if is_lower {
                if inclusive { ord.is_ge() } else { ord.is_gt() }
            } else if inclusive {
                ord.is_le()
            } else {
                ord.is_lt()
            }
        }
    }
}

fn split_op(clause: &str) -> (&str, &str) {
    for op in ["<=", ">=", "==", "!=", "<", ">", "="] {
        if let Some(rest) = clause.strip_prefix(op) {
            return (op, rest.trim().trim_matches(['[', ']']));
        }
    }
    ("=", clause.trim_matches(['[', ']']))
}

fn bound_ok(found: &str, op: &str, edge: &str) -> bool {
    if edge.is_empty() {
        return true;
    }
    if let Some(want) = wildcard_prefix(edge) {
        let have = version_tuple(found);
        let eq = have.len() >= want.len() && have[..want.len()] == want[..];
        return if op == "!=" { !eq } else { eq };
    }
    let ord = version_tuple(found).cmp(&version_tuple(edge));
    match op {
        ">=" => ord.is_ge(),
        ">" => ord.is_gt(),
        "<=" => ord.is_le(),
        "<" => ord.is_lt(),
        "=" | "==" => ord.is_eq(),
        "!=" => ord.is_ne(),
        _ => true,
    }
}

fn version_tuple(text: &str) -> Vec<u64> {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let core: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    core.split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

fn wildcard_prefix(edge: &str) -> Option<Vec<u64>> {
    let last = edge.rsplit('.').next()?.trim().trim_end_matches(['-', '+']);
    if !matches!(last, "x" | "X" | "*" | "+") {
        return None;
    }
    let cut = edge.len().saturating_sub(last.len());
    let head = edge[..cut].trim_end_matches(['.', '-', '+']);
    Some(version_tuple(head))
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .collect::<String>()
        .to_ascii_lowercase()
}

pub struct Target {
    pub minecraft: String,
    pub java: String,
    pub loader: String,
}

pub const LOADER_APIS: [&str; 6] = [
    "fabric",
    "forge",
    "neoforge",
    "quilt",
    "quilt_loader",
    "loader",
];

pub fn mc_minor(version: &str) -> (u64, u64) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor)
}

pub fn shares_mc_minor(a: &str, b: &str) -> bool {
    mc_minor(a) == mc_minor(b)
}

pub fn game_mc_ok(target: &str, listed: &[String]) -> bool {
    if listed.is_empty() {
        return true;
    }
    if listed.iter().any(|g| g == target) {
        return true;
    }
    listed.iter().any(|g| shares_mc_minor(target, g))
}

pub fn is_platform(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    LOADER_APIS.contains(&id.as_str())
        || id == "minecraft"
        || id == "java"
        || id == "fabricloader"
        || id == "kotlin"
        || id == "org_jetbrains_annotations"
}

pub fn provided_by_fabric_api(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id == "fabric-api" || (id.starts_with("fabric-") && id != "fabric-legacy")
}

pub struct Report {
    pub errors: Vec<Conflict>,
    pub warnings: Vec<Conflict>,
}

pub fn check(metas: &[ModMeta], target: &Target) -> Report {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let has_fabric_api = metas.iter().any(|m| m.id.eq_ignore_ascii_case("fabric-api"));

    for meta in metas {
        if meta.id.is_empty() {
            continue;
        }
        for want in meta.depends.iter().filter(|d| d.required) {
            let id = want.id.to_ascii_lowercase();
            let label = if want.range.is_empty() {
                "any version".to_string()
            } else {
                want.range.clone()
            };

            if id == "minecraft" {
                if !fits_minecraft(&want.range, &target.minecraft) {
                    errors.push(Conflict {
                        offender: meta.id.clone(),
                        other: "minecraft".to_string(),
                        wanted: label,
                        found: target.minecraft.clone(),
                        kind: "needs",
                    });
                }
                continue;
            }
            if id == "java" {
                if !satisfied(&want.range, &target.java) {
                    errors.push(Conflict {
                        offender: meta.id.clone(),
                        other: "java".to_string(),
                        wanted: label,
                        found: target.java.clone(),
                        kind: "needs",
                    });
                }
                continue;
            }
            if id == "fabricloader" {
                if !satisfied(&want.range, &target.loader) {
                    errors.push(Conflict {
                        offender: meta.id.clone(),
                        other: "fabricloader".to_string(),
                        wanted: label,
                        found: target.loader.clone(),
                        kind: "needs",
                    });
                }
                continue;
            }
            if is_platform(&id) {
                continue;
            }
            if provided_by_fabric_api(&id) {
                if !has_fabric_api {
                    errors.push(Conflict {
                        offender: meta.id.clone(),
                        other: want.id.clone(),
                        wanted: label,
                        found: "fabric-api is not in the pack".to_string(),
                        kind: "needs",
                    });
                }
                continue;
            }

            let Some(other) = metas.iter().find(|m| m.id.eq_ignore_ascii_case(&id)) else {
                errors.push(Conflict {
                    offender: meta.id.clone(),
                    other: want.id.clone(),
                    wanted: label,
                    found: "missing".to_string(),
                    kind: "needs",
                });
                continue;
            };
            if !satisfied(&want.range, &other.version) {
                errors.push(Conflict {
                    offender: meta.id.clone(),
                    other: want.id.clone(),
                    wanted: label,
                    found: other.version.clone(),
                    kind: "needs",
                });
            }
        }

        for hate in &meta.breaks {
            if is_platform(&hate.id) || provided_by_fabric_api(&hate.id) {
                continue;
            }
            if let Some(other) = metas.iter().find(|m| m.id.eq_ignore_ascii_case(&hate.id)) {
                warnings.push(Conflict {
                    offender: meta.id.clone(),
                    other: hate.id.clone(),
                    wanted: hate.range.clone(),
                    found: other.version.clone(),
                    kind: "breaks",
                });
            }
        }
    }

    errors.dedup_by(|a, b| a.offender == b.offender && a.other == b.other);
    warnings.dedup_by(|a, b| a.offender == b.offender && a.other == b.other);
    Report { errors, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_range_means_anything() {
        assert!(satisfied("", "1.2.3"));
        assert!(satisfied("*", "1.2.3"));
    }

    #[test]
    fn a_bare_version_is_an_exact_match() {
        assert!(satisfied("1.2.3", "1.2.3"));
        assert!(!satisfied("1.2.3", "1.2.4"));
    }

    #[test]
    fn the_iris_sodium_range_is_understood() {
        assert!(satisfied(">=0.7.0 <0.8.0", "0.7.3"));
        assert!(!satisfied(">=0.7.0 <0.8.0", "0.8.7"));
        assert!(!satisfied(">=0.7.0 <0.8.0", "0.8.0-rc.1"));
    }

    #[test]
    fn a_tilde_minecraft_range_is_the_whole_minor_line() {
        assert!(fits_minecraft("~1.21.5", "1.21.10"));
        assert!(fits_minecraft("~1.21.5", "1.21.5"));
        assert!(fits_minecraft("~1.21", "1.21.11"));
        assert!(!fits_minecraft("~1.21.5", "1.22.0"));
        assert!(!fits_minecraft("~1.21.5", "1.20.6"));
    }

    #[test]
    fn a_tight_minecraft_range_is_still_honoured() {
        assert!(fits_minecraft(">=1.21.11", "1.21.11"));
        assert!(!fits_minecraft(">=1.21.11 <1.21.12", "1.21.10"));
    }

    #[test]
    fn a_placeholder_range_is_not_a_conflict() {
        assert!(fits_minecraft("${version}", "1.21.10"));
        assert!(satisfied("${version}", "0.7.3"));
    }

    #[test]
    fn a_tilde_is_a_soft_requirement() {
        assert!(satisfied("~1.21", "1.21.11"));
        assert!(!satisfied("~1.21", "1.20.6"));
        assert!(!satisfied("~1.21", "1.22.0"));
        assert!(satisfied("~1.21.5", "1.21.5"));
        assert!(!satisfied("~1.21.5", "1.21.11"));
        assert!(satisfied("~1.21.11", "1.21.11"));
    }

    #[test]
    fn a_maven_comma_range_works() {
        assert!(satisfied("[0.7.0,0.8.0)", "0.7.9"));
        assert!(!satisfied("[0.7.0,0.8.0)", "0.8.1"));
    }

    fn meta(id: &str, version: &str) -> ModMeta {
        ModMeta {
            id: id.to_string(),
            version: version.to_string(),
            name: id.to_string(),
            depends: Vec::new(),
            breaks: Vec::new(),
        }
    }

    fn target() -> Target {
        Target {
            minecraft: "1.21.11".to_string(),
            java: "21".to_string(),
            loader: "0.19.5".to_string(),
        }
    }

    fn iris_depends_sodium() -> ModMeta {
        let mut iris = meta("iris", "1.10.7");
        iris.depends.push(Depends {
            id: "sodium".to_string(),
            range: ">=0.7.0 <0.8.0".to_string(),
            required: true,
        });
        iris
    }

    #[test]
    fn the_machine_is_quiet_when_everything_fits() {
        let found = check(&[iris_depends_sodium(), meta("sodium", "0.7.3")], &target());
        assert!(found.errors.is_empty(), "{:?}", found.errors);
    }

    #[test]
    fn a_missing_mod_is_an_error() {
        let found = check(&[iris_depends_sodium()], &target());
        assert_eq!(found.errors.len(), 1);
        assert_eq!(found.errors[0].found, "missing");
    }

    #[test]
    fn the_wrong_sodium_is_an_error_with_both_numbers() {
        let found = check(&[iris_depends_sodium(), meta("sodium", "0.8.7")], &target());
        assert_eq!(found.errors.len(), 1);
        assert_eq!(found.errors[0].found, "0.8.7");
        assert!(found.errors[0].wanted.contains("0.7.0"));
    }

    #[test]
    fn platform_deps_are_not_looked_up_as_mods() {
        let mut m = meta("egn", "1.1");
        for (id, range) in [
            ("minecraft", "~1.21"),
            ("java", ">=21"),
            ("fabricloader", ">=0.18.1"),
            ("fabric", "*"),
        ] {
            m.depends.push(Depends {
                id: id.to_string(),
                range: range.to_string(),
                required: true,
            });
        }
        let found = check(&[m], &target());
        assert!(found.errors.is_empty(), "{:?}", found.errors);
    }

    #[test]
    fn a_minecraft_dep_that_does_not_fit_is_reported() {
        let mut m = meta("oldmod", "1.0");
        m.depends.push(Depends {
            id: "minecraft".to_string(),
            range: "~1.20.1".to_string(),
            required: true,
        });
        let found = check(&[m], &target());
        assert_eq!(found.errors.len(), 1);
        assert_eq!(found.errors[0].other, "minecraft");
        assert_eq!(found.errors[0].found, "1.21.11");
    }

    #[test]
    fn a_loader_dep_that_does_not_fit_is_reported() {
        let mut m = meta("needsnewloader", "1.0");
        m.depends.push(Depends {
            id: "fabricloader".to_string(),
            range: ">=0.99.0".to_string(),
            required: true,
        });
        let found = check(&[m], &target());
        assert_eq!(found.errors.len(), 1);
        assert_eq!(found.errors[0].found, "0.19.5");
    }

    #[test]
    fn fabric_api_covers_its_own_modules() {
        let mut sodium = meta("sodium", "0.7.3");
        sodium.depends.push(Depends {
            id: "fabric-rendering-v1".to_string(),
            range: "*".to_string(),
            required: true,
        });
        let quiet = check(&[sodium.clone(), meta("fabric-api", "0.141.6")], &target());
        assert!(quiet.errors.is_empty(), "{:?}", quiet.errors);
        let loud = check(&[sodium], &target());
        assert_eq!(loud.errors.len(), 1);
    }

    #[test]
    fn breaks_is_a_warning_not_an_error() {
        let mut sodium = meta("sodium", "0.8.7");
        sodium.breaks.push(Depends {
            id: "sodium-extra".to_string(),
            range: "<0.8.0".to_string(),
            required: true,
        });
        let found = check(
            &[sodium, meta("sodium-extra", "0.8.3")],
            &target(),
        );
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        assert_eq!(found.warnings.len(), 1);
    }

    #[test]
    fn an_optional_dependent_is_not_complained_about() {
        let mut iris = meta("iris", "1.10.7");
        iris.depends.push(Depends {
            id: "something-extra".to_string(),
            range: ">=1.0.0".to_string(),
            required: false,
        });
        assert!(check(&[iris], &target()).errors.is_empty());
    }

    #[test]
    fn fabric_json_parses() {
        let text = r#"{"schemaVersion":1,"id":"iris","version":"1.10.7",
          "depends":{"sodium":">=0.7.0 <0.8.0","fabricloader":">=0.15.0"}}"#;
        let meta = parse("fabric.mod.json", text).unwrap();
        assert_eq!(meta.id, "iris");
        assert_eq!(meta.version, "1.10.7");
        let sodium = meta.depends.iter().find(|d| d.id == "sodium").unwrap();
        assert_eq!(sodium.range, ">=0.7.0 <0.8.0");
        assert!(sodium.required);
    }

    #[test]
    fn neoforge_toml_parses() {
        let text = "modLoader=\"javafml\"\nloaderVersion=\"[1,)\"\nlicense=\"LGPL-3.0-or-later\"\n\n[[mods]]\n\tmodId=\"yet_another_config_lib_v3\"\n\tversion=\"3.8.2\"\n\tdisplayName=\"YetAnotherConfigLib\"\n\n[[mixins]]\n\tconfig=\"yacl.mixins.json\"\n\n[[\"dependencies.yet_another_config_lib_v3\"]]\n\tversionRange=\"[21.9,21.10]\"\n\tmodId=\"neoforge\"\n[['dependencies.yet_another_config_lib_v3']]\n\tversionRange=\"[1.21.9,1.21.10]\"\n\tmodId=\"minecraft\"\n";
        let meta = parse("META-INF/neoforge.mods.toml", text).unwrap();
        assert_eq!(meta.id, "yet_another_config_lib_v3");
        assert_eq!(meta.version, "3.8.2");
        assert_eq!(meta.name, "YetAnotherConfigLib");
        let deps: Vec<String> = meta.depends.iter().map(|d| d.id.clone()).collect();
        assert!(deps.contains(&"neoforge".to_string()), "{deps:?}");
        assert!(deps.contains(&"minecraft".to_string()), "{deps:?}");
    }

    #[test]
    fn an_x_wildcard_range_matches_any_patch() {
        assert!(fits_minecraft("1.21.x", "1.21.11"));
        assert!(fits_minecraft("1.21.x", "1.21.9"));
        assert!(fits_minecraft("1.21.*", "1.21.9"));
        assert!(fits_minecraft("1.21.X", "1.21.9"));
        assert!(!fits_minecraft("1.21.x", "1.22.0"));
        assert!(!fits_minecraft("1.21.x", "1.20.6"));
    }

    #[test]
    fn a_prerelease_suffix_does_not_inflate_a_version() {
        assert!(satisfied(">=1.21.9-beta.2 <1.22", "1.21.9"));
        assert!(satisfied(">=1.21.9-beta.1", "1.21.9"));
        assert!(satisfied(">=0.3.2", "0.3.2-beta.1"));
        assert!(!satisfied(">=1.21.9-beta.2 <1.22", "1.20.6"));
        assert!(satisfied("[21.10.0-beta,)", "21.10.0"));
    }

    #[test]
    fn maven_unbounded_brackets_work() {
        assert!(satisfied("[17,)", "21"));
        assert!(satisfied("[17,)", "17"));
        assert!(!satisfied("[17,)", "16"));
        assert!(satisfied("[1.21.10]", "1.21.10"));
        assert!(!satisfied("[1.21.10]", "1.21.11"));
        assert!(satisfied("(,1.21.10]", "1.21.9"));
        assert!(!satisfied("(,1.21.10]", "1.21.11"));
    }
}