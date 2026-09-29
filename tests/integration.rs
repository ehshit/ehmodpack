use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use ehmodpack::build::{build, inject_active, mrpack_index, output_name};
use ehmodpack::importer;
use ehmodpack::loaders::LoaderKind;
use ehmodpack::lock::{LOCK_VERSION, Lock, LockedPackage, LockedTarget};
use ehmodpack::manifest::{
    self, Env, LoaderSpec, Manifest, Package, PkgType, RpPosition, Softwares, Support, Target,
};
use ehmodpack::modrinth::{DEFAULT_API, Hashes, Modrinth};
use sha2::Digest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn target(minecraft: &str, kind: &str, version: &str) -> LockedTarget {
    LockedTarget {
        minecraft: minecraft.to_string(),
        java: Some("21".to_string()),
        loader: LoaderSpec {
            kind: kind.to_string(),
            version: version.to_string(),
        },
        packages: vec![pack("the-stupidest-pack", PkgType::Resourcepack, Some(RpPosition::Top))],
    }
}

fn pack(slug: &str, kind: PkgType, position: Option<RpPosition>) -> LockedPackage {
    let filename = format!("{slug}-1.0.0.zip");
    LockedPackage {
        kind,
        project: slug.to_string(),
        project_id: "abc123".to_string(),
        version_id: "ver123".to_string(),
        version_number: "1.0.0".to_string(),
        path: format!("{}/{filename}", kind.folder()),
        filename,
        hashes: Hashes {
            sha1: "aa".to_string(),
            sha512: "bb".to_string(),
        },
        downloads: vec![format!(
            "https://cdn.modrinth.com/data/abc123/versions/ver123/{slug}.zip"
        )],
        file_size: 4242,
        env: Env {
            client: Support::Required,
            server: Support::Unsupported,
        },
        position,
        source: "modrinth".to_string(),
    }
}

fn lock(targets: Vec<LockedTarget>) -> Lock {
    Lock::stamped(
        "2026-09-27T00:00:00Z".to_string(),
        "deadbeef".to_string(),
        "eh's pack".to_string(),
        "0.1.0".to_string(),
        "brrr".to_string(),
        targets,
    )
}

#[test]
fn the_lockfile_is_stamped_with_its_schema() {
    let t = target("1.21.11", "fabric", "0.19.5");
    let l = lock(vec![t]);
    let json: serde_json::Value = serde_json::from_str(&l.to_json()).unwrap();
    assert_eq!(
        json["$schema"],
        "https://docs.ehis.gay/ehmodpack/schema/lock.schema.json"
    );
    assert_eq!(json["schemaVersion"], manifest::SCHEMA_VERSION);
    assert_eq!(json["lock_version"], LOCK_VERSION);
    assert!(json.get("manifest_hash").is_some());
    assert!(json["targets"][0]["packages"][0]["file_size"].is_number());
}

#[test]
fn the_lock_schemas_are_valid_json() {
    for (name, body) in [
        ("packages", manifest::PACKAGES_SCHEMA),
        ("softwares", manifest::SOFTWARES_SCHEMA),
        ("lock", manifest::LOCK_SCHEMA),
    ] {
        let parsed: serde_json::Value = serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("{name} schema is not valid json: {e}"));
        assert_eq!(
            parsed["$schema"], "https://json-schema.org/draft/2020-12/schema",
            "{name}"
        );
        assert!(parsed["$id"].is_string(), "{name} has no $id");
        assert_eq!(parsed["type"], "object", "{name}");
        assert!(parsed["properties"].is_object(), "{name}");
    }
}

#[test]
fn every_schema_field_matches_what_the_code_writes() {
    let t = target("1.21.11", "fabric", "0.19.5");
    let l = lock(vec![t]);
    let written: serde_json::Value = serde_json::from_str(&l.to_json()).unwrap();
    let schema: serde_json::Value = serde_json::from_str(manifest::LOCK_SCHEMA).unwrap();
    let top = schema["properties"].as_object().unwrap();
    for key in written.as_object().unwrap().keys() {
        assert!(top.contains_key(key), "the lock schema is missing {key}");
    }
}

#[test]
fn no_active_packs_leaves_vanilla_alone() {
    let out = inject_active("resourcePacks:[\"vanilla\"]\n", &[], &[]);
    assert!(out.contains("resourcePacks:[\"vanilla\"]"), "{out}");
}

#[test]
fn injection_is_idempotent() {
    let active = vec!["Icons.zip".to_string()];
    let once = inject_active("resourcePacks:[\"vanilla\"]\n", &active, &[]);
    let twice = inject_active(&once, &active, &[]);
    assert_eq!(once, twice);
    assert_eq!(once.matches("file/Icons.zip").count(), 1);
}

#[test]
fn injection_clears_incompatible_packs() {
    let out = inject_active(
        "resourcePacks:[\"vanilla\"]\nincompatibleResourcePacks:[\"something\"]\n",
        &["Icons.zip".to_string()],
        &[],
    );
    assert!(out.contains("incompatibleResourcePacks:[]"));
    assert!(!out.contains("something"));
}

#[test]
fn index_has_the_mrpack_shape() {
    let t = target("1.21.11", "fabric", "0.19.5");
    let l = lock(vec![t.clone()]);
    let index = mrpack_index(&l, &t);
    assert_eq!(index["formatVersion"], 1);
    assert_eq!(index["game"], "minecraft");
    assert_eq!(index["versionId"], "0.1.0");
    assert_eq!(index["name"], "eh's pack");
    assert_eq!(index["dependencies"]["minecraft"], "1.21.11");
    assert_eq!(index["dependencies"]["fabric-loader"], "0.19.5");
    assert_eq!(index["files"][0]["path"], "resourcepacks/the-stupidest-pack-1.0.0.zip");
    assert_eq!(index["files"][0]["env"]["client"], "required");
    assert_eq!(index["files"][0]["env"]["server"], "unsupported");
    assert_eq!(index["files"][0]["fileSize"], 4242);
    assert_eq!(index["files"][0]["hashes"]["sha512"], "bb");
}

#[test]
fn neoforge_targets_get_a_neoforge_dependency_key() {
    let t = target("1.21.11", "neoforge", "21.11.4");
    let l = lock(vec![t.clone()]);
    let deps = mrpack_index(&l, &t)["dependencies"].clone();
    assert_eq!(deps["neoforge"], "21.11.4");
    assert!(deps.get("neoforge-loader").is_none());
    assert!(deps.get("fabric-loader").is_none());
}

#[test]
fn every_active_pack_is_kept_now() {
    let mut t = target("1.21.11", "fabric", "0.19.5");
    t.packages
        .push(pack("second", PkgType::Resourcepack, Some(RpPosition::Bottom)));
    let names: Vec<String> = t.active_packs().iter().map(|p| p.project.clone()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"the-stupidest-pack".to_string()));
    assert!(names.contains(&"second".to_string()));
}

#[test]
fn single_target_output_has_no_suffix() {
    let t = target("1.21.11", "fabric", "0.19.5");
    let l = lock(vec![t.clone()]);
    assert_eq!(output_name(&l, &t), "eh-s-pack");
}

#[test]
fn multi_target_output_is_disambiguated() {
    let a = target("1.21.11", "fabric", "0.19.5");
    let b = target("1.21.11", "quilt", "0.27.1");
    let l = lock(vec![a.clone(), b.clone()]);
    assert_eq!(output_name(&l, &a), "eh-s-pack-mc-1-21-11-fabric");
    assert_eq!(output_name(&l, &b), "eh-s-pack-mc-1-21-11-quilt");
}

#[test]
fn manifest_tolerates_comments_and_trailing_commas() -> anyhow::Result<()> {
    let text = r#"{
      // the good stuff
      "name": "eh's pack",
      /* block comment */
      "packages": [
        { "type": "mod", "project": "sodium", "version": "*" },
      ],
    }"#;
    let m = Manifest::parse(text)?;
    assert_eq!(m.name, "eh's pack");
    assert_eq!(m.packages.len(), 1);
    assert_eq!(m.packages[0].key(), "sodium");
    assert_eq!(m.overrides, "overrides");
    Ok(())
}

#[test]
fn manifest_fingerprint_ignores_formatting() -> anyhow::Result<()> {
    let a = Manifest::parse("{\"name\":\"x\",\"packages\":[]}")?;
    let b = Manifest::parse("{\n  \"name\": \"x\",\n  \"packages\": []\n}\n")?;
    assert_eq!(a.fingerprint(), b.fingerprint());
    Ok(())
}

fn spread_package() -> Package {
    let mut versions = std::collections::BTreeMap::new();
    versions.insert("1.21.11+fabric".to_string(), "0.6.0".to_string());
    versions.insert("1.21.11+quilt".to_string(), "0.6.0".to_string());
    versions.insert("1.21.8+fabric".to_string(), "0.5.3".to_string());
    Package {
        kind: PkgType::Mod,
        project: Some("sodium".to_string()),
        id: None,
        version: None,
        versions,
        env: None,
        optional: None,
        active: None,
        position: None,
        ids: None,
        lock: None,
    }
}

#[test]
fn version_lookup_prefers_the_exact_target() {
    let p = spread_package();
    assert_eq!(p.version_for("1.21.11", "fabric").as_deref(), Some("0.6.0"));
    assert_eq!(p.version_for("1.21.8", "fabric").as_deref(), Some("0.5.3"));
    assert_eq!(p.version_for("1.21.8", "quilt"), None);
}

#[test]
fn version_lookup_falls_back_to_the_minecraft_key() {
    let mut p = spread_package();
    p.versions.insert("1.21.8".to_string(), "0.4.0".to_string());
    assert_eq!(p.version_for("1.21.8", "quilt").as_deref(), Some("0.4.0"));
}

#[test]
fn version_lookup_falls_back_to_default_then_star() {
    let mut p = spread_package();
    p.versions.insert("default".to_string(), "0.3.0".to_string());
    assert_eq!(p.version_for("1.20.1", "fabric").as_deref(), Some("0.3.0"));
    p.versions.remove("default");
    p.versions.insert("*".to_string(), "0.2.0".to_string());
    assert_eq!(p.version_for("1.20.1", "neoforge").as_deref(), Some("0.2.0"));
}

#[test]
fn a_plain_version_shorthand_beats_everything() {
    let mut p = spread_package();
    p.version = Some("9.9.9".to_string());
    assert_eq!(p.version_for("1.21.8", "fabric").as_deref(), Some("9.9.9"));
}

fn soft(targets: &[(&str, &str)]) -> Softwares {
    Softwares {
        schema: None,
        schema_version: manifest::SCHEMA_VERSION,
        targets: targets
            .iter()
            .map(|(mc, kind)| Target {
                minecraft: (*mc).to_string(),
                java: Some("21".to_string()),
                loader: LoaderSpec {
                    kind: (*kind).to_string(),
                    version: "1.2.3".to_string(),
                },
            })
            .collect(),
    }
}

#[test]
fn pick_finds_the_only_match() -> anyhow::Result<()> {
    let s = soft(&[("1.21.11", "fabric"), ("1.21.8", "fabric")]);
    let t = manifest::pick(&s.targets, Some("1.21.8"), None)?;
    assert_eq!(t.minecraft, "1.21.8");
    Ok(())
}

#[test]
fn pick_matches_a_partial_minecraft_version() -> anyhow::Result<()> {
    let s = soft(&[("1.21.11", "fabric")]);
    let t = manifest::pick(&s.targets, Some("1.21"), None)?;
    assert_eq!(t.minecraft, "1.21.11");
    Ok(())
}

#[test]
fn pick_separates_loaders() -> anyhow::Result<()> {
    let s = soft(&[("1.21.11", "fabric"), ("1.21.11", "quilt")]);
    let t = manifest::pick(&s.targets, Some("1.21.11"), Some(LoaderKind::Quilt))?;
    assert_eq!(t.loader.kind, "quilt");
    Ok(())
}

#[test]
fn pick_refuses_an_ambiguous_request() {
    let s = soft(&[("1.21.11", "fabric"), ("1.21.11", "quilt")]);
    let err = manifest::pick(&s.targets, Some("1.21.11"), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("ambiguous"), "{err}");
}

#[test]
fn pick_lists_what_it_has_when_nothing_matches() {
    let s = soft(&[("1.21.11", "fabric")]);
    let err = manifest::pick(&s.targets, Some("1.7.10"), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("1.21.11/fabric"), "{err}");
}

#[test]
fn build_writes_an_mrpack_with_overrides() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let client = root.join("client-overrides");
    fs::create_dir_all(client.join("config/sodium"))?;
    fs::write(client.join("options.txt"), "resourcePacks:[\"vanilla\"]\n")?;
    fs::write(client.join("config/sodium/mixins.sodium.json"), "{}\n")?;

    let t = target("1.21.11", "fabric", "0.19.5");
    let l = lock(vec![t.clone()]);
    let out = build(
        &l,
        &t,
        &root.join("dist"),
        &[("client-overrides".to_string(), client.clone())],
    )?;
    assert!(out.exists(), "the archive should exist");
    assert_eq!(out.file_name().unwrap(), "eh-s-pack.mrpack");

    let file = fs::File::open(&out)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.contains(&"modrinth.index.json".to_string()));
    assert!(names.contains(&"overrides/options.txt".to_string()));
    assert!(names.contains(&"overrides/config/sodium/mixins.sodium.json".to_string()));

    let mut index = String::new();
    archive
        .by_name("modrinth.index.json")
        .unwrap()
        .read_to_string(&mut index)?;
    let parsed: serde_json::Value = serde_json::from_str(&index)?;
    assert_eq!(parsed["files"][0]["env"]["client"], "required");

    let mut options = String::new();
    archive
        .by_name("overrides/options.txt")
        .unwrap()
        .read_to_string(&mut options)?;
    assert!(
        options.contains("file/the-stupidest-pack-1.0.0.zip"),
        "{options}"
    );
    Ok(())
}

#[test]
fn the_index_never_writes_a_java_dependency() {
    let t = target("1.21.11", "fabric", "0.19.5");
    let l = lock(vec![t.clone()]);
    let index = mrpack_index(&l, &t);
    let deps = index["dependencies"].as_object().expect("dependencies is an object");
    assert!(deps.get("java").is_none(), "java is not a spec dependency key");
    for key in deps.keys() {
        assert!(
            manifest::validate_allowed(key),
            "{key} is not a dependency key the spec allows"
        );
    }
    assert_eq!(deps.len(), 2, "minecraft plus the loader");
}

#[test]
fn quilt_and_neoforge_get_their_own_dependency_keys() {
    let quilt = target("1.21.11", "quilt", "0.27.1");
    let l = lock(vec![quilt.clone()]);
    assert_eq!(
        mrpack_index(&l, &quilt)["dependencies"]["quilt-loader"],
        "0.27.1"
    );
    let neo = target("1.21.11", "neoforge", "21.11.4");
    let l = lock(vec![neo.clone()]);
    let deps = mrpack_index(&l, &neo)["dependencies"].clone();
    assert_eq!(deps["neoforge"], "21.11.4");
    assert!(deps.get("neoforge-loader").is_none());
}

#[test]
fn validation_catches_a_pack_with_no_downloads() {
    let mut t = target("1.21.11", "fabric", "0.19.5");
    t.packages[0].downloads.clear();
    t.packages[0].hashes.sha512 = String::new();
    let l = lock(vec![t.clone()]);
    let pack = Manifest::parse("{\"name\":\"x\",\"packages\":[]}").unwrap();
    let report = manifest::validate_report(&l, &pack);
    assert!(!report.ok());
    let joined = report.errors.join(" | ");
    assert!(joined.contains("no download url"), "{joined}");
    assert!(joined.contains("no sha512"), "{joined}");
}

#[test]
fn a_mod_cannot_be_marked_active() {
    let mut t = target("1.21.11", "fabric", "0.19.5");
    t.packages = vec![pack("sodium", PkgType::Mod, Some(RpPosition::Top))];
    let l = lock(vec![t]);
    let pack = Manifest::parse("{\"name\":\"x\",\"packages\":[]}").unwrap();
    let report = manifest::validate_report(&l, &pack);
    assert!(!report.ok());
    assert!(
        report.errors.join(" ").contains("resourcepack or shader"),
        "{:?}",
        report.errors
    );
}

#[test]
fn two_active_packs_are_fine() {
    let mut t = target("1.21.11", "fabric", "0.19.5");
    t.packages
        .push(pack("second", PkgType::Resourcepack, Some(RpPosition::Bottom)));
    let l = lock(vec![t]);
    let pack = Manifest::parse("{\"name\":\"x\",\"packages\":[]}").unwrap();
    let report = manifest::validate_report(&l, &pack);
    assert!(report.ok(), "{:?}", report.errors);
}

#[test]
fn validation_passes_on_a_sane_pack() {
    let t = target("1.21.11", "fabric", "0.19.5");
    let l = lock(vec![t]);
    let pack = Manifest::parse("{\"name\":\"x\",\"packages\":[]}").unwrap();
    let report = manifest::validate_report(&l, &pack);
    assert!(report.ok(), "{:?}", report.errors);
}

#[test]
fn a_loader_key_the_spec_rejects_is_caught() {
    let mut t = target("1.21.11", "fabric", "0.19.5");
    t.loader.kind = "rift".to_string();
    let l = lock(vec![t]);
    let pack = Manifest::parse("{\"name\":\"x\",\"packages\":[]}").unwrap();
    let report = manifest::validate_report(&l, &pack);
    assert!(!report.ok());
}

#[test]
fn active_packs_become_file_entries_in_options() {
    let out = inject_active(
        "resourcePacks:[\"vanilla\"]\n",
        &["Icons v.1.13.4.zip".to_string(), "Ginkgo Font.zip".to_string()],
        &[],
    );
    assert!(out.contains("resourcePacks:[\"vanilla\",\"file/Icons v.1.13.4.zip\",\"file/Ginkgo Font.zip\"]"), "{out}");
    assert!(!out.contains("fabric-resource-pack-v0"));
}

#[test]
fn only_the_active_packs_survive_in_options() {
    let before = "resourcePacks:[\"vanilla\",\"file/NotActive.zip\",\"file/AlsoNot.zip\"]\n";
    let out = inject_active(before, &["Wanted.zip".to_string()], &[]);
    assert!(out.contains("resourcePacks:[\"vanilla\",\"file/Wanted.zip\"]"), "{out}");
    assert!(!out.contains("NotActive"));
}

#[test]
fn the_same_pack_is_never_listed_twice() {
    let out = inject_active(
        "resourcePacks:[\"vanilla\"]\n",
        &["a.zip".to_string(), "a.zip".to_string()],
        &[],
    );
    assert_eq!(out.matches("file/a.zip").count(), 1, "{out}");
}

#[test]
fn client_overrides_land_in_the_overrides_folder() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::create_dir_all(root.join("overrides"))?;
    fs::create_dir_all(root.join("client-overrides"))?;
    fs::write(root.join("overrides/base.txt"), "base")?;
    fs::write(root.join("client-overrides/options.txt"), "resourcePacks:[\"vanilla\"]\n")?;

    let mut t = target("1.21.11", "fabric", "0.19.5");
    t.packages = vec![pack("Icons v.1.13.4.zip", PkgType::Resourcepack, Some(RpPosition::Top))];
    let l = lock(vec![t.clone()]);
    let out = build(
        &l,
        &t,
        &root.join("dist"),
        &[
            ("overrides".to_string(), root.join("overrides")),
            ("client-overrides".to_string(), root.join("client-overrides")),
        ],
    )?;
    let file = fs::File::open(&out)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.contains(&"overrides/base.txt".to_string()), "{names:?}");
    assert!(names.contains(&"overrides/options.txt".to_string()), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("client-overrides")),
        "there should be no client-overrides folder in the archive: {names:?}"
    );
    let mut options = String::new();
    archive
        .by_name("overrides/options.txt")
        .unwrap()
        .read_to_string(&mut options)?;
    assert!(options.contains("file/Icons v.1.13.4.zip"), "{options}");
    Ok(())
}

#[test]
fn a_pack_we_do_not_manage_is_left_alone() {
    let before = "resourcePacks:[\"vanilla\",\"file/Local Icons.zip\",\"file/Old Modrinth Pack.zip\"]\n";
    let local = vec!["Local Icons.zip".to_string()];
    let out = inject_active(before, &["New Modrinth Pack.zip".to_string()], &local);
    assert!(out.contains("\"file/Local Icons.zip\""), "local pack dropped: {out}");
    assert!(!out.contains("Old Modrinth Pack"), "stale entry kept: {out}");
    assert!(out.contains("\"file/New Modrinth Pack.zip\""), "{out}");
}

#[test]
fn an_entry_with_nothing_on_disk_is_dropped() {
    let before = "resourcePacks:[\"vanilla\",\"file/Ghost.zip\"]\n";
    let out = inject_active(before, &[], &[]);
    assert!(out.contains("resourcePacks:[\"vanilla\"]"), "{out}");
}

#[test]
fn the_options_list_follows_the_current_filename() {
    let first = inject_active("resourcePacks:[\"vanilla\"]\n", &["Icons-1.0.zip".to_string()], &[]);
    assert!(first.contains("file/Icons-1.0.zip"));
    let second = inject_active(&first, &["Icons-2.0.zip".to_string()], &[]);
    assert!(second.contains("file/Icons-2.0.zip"), "{second}");
    assert!(!second.contains("Icons-1.0"), "the old filename lingered: {second}");
    assert_eq!(second.matches("Icons").count(), 1, "{second}");
}

#[test]
fn a_body_that_matches_both_hashes_passes() {
    let body = b"hello there";
    let sha1 = ehmodpack::build::sha1_of_bytes(body).unwrap();
    let sha512 = {
        use sha2::Digest;
        let mut h = sha2::Sha512::new();
        h.update(body);
        format!("{:x}", h.finalize())
    };
    assert!(ehmodpack::staging::verify("x", body, &sha1, &sha512).is_ok());
}

#[test]
fn a_wrong_sha512_is_caught() {
    let body = b"hello there";
    let err = ehmodpack::staging::verify("sodium", body, "", "deadbeef")
        .unwrap_err()
        .to_string();
    assert!(err.contains("sha512 does not match"), "{err}");
}

#[test]
fn a_wrong_sha1_is_caught() {
    let body = b"hello there";
    let err = ehmodpack::staging::verify("sodium", body, "deadbeef", "")
        .unwrap_err()
        .to_string();
    assert!(err.contains("sha1 does not match"), "{err}");
}

#[test]
fn an_empty_hash_is_not_checked() {
    assert!(ehmodpack::staging::verify("x", b"anything", "", "").is_ok());
}

fn write_mrpack(path: &Path) -> anyhow::Result<()> {
    let file = fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let index = r#"{
      "formatVersion": 1,
      "game": "minecraft",
      "versionId": "1.2.3",
      "name": "Somebody's pack",
      "summary": "hello",
      "dependencies": {
        "minecraft": "1.21.11",
        "java": "21",
        "fabric-loader": "0.19.5"
      },
      "files": [
        {
          "path": "mods/sodium-fabric-0.6.0.jar",
          "hashes": { "sha1": "aa", "sha512": "bb" },
          "env": { "client": "required", "server": "unsupported" },
          "downloads": ["https://cdn.modrinth.com/data/AANobbMI/versions/vvv/sodium.jar"],
          "fileSize": 1234
        },
        {
          "path": "resourcepacks/pack.zip",
          "hashes": { "sha1": "cc", "sha512": "dd" },
          "env": { "client": "optional", "server": "unsupported" },
          "downloads": ["https://cdn.modrinth.com/data/CCC/versions/v2/pack.zip"],
          "fileSize": 99
        }
      ]
    }"#;
    zip.start_file("modrinth.index.json", opts)?;
    zip.write_all(index.as_bytes())?;
    zip.start_file("overrides/config/sodium/mixins.sodium.json", opts)?;
    zip.write_all(b"{\"injected\":true}")?;
    zip.start_file("client-overrides/options.txt", opts)?;
    zip.write_all(b"resourcePacks:[\"vanilla\"]\n")?;
    zip.finish()?;
    Ok(())
}

#[test]
fn importer_reads_the_index() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("in.mrpack");
    write_mrpack(&path)?;
    let index = importer::read_index(&path)?;
    assert_eq!(importer::name_of(&index), "Somebody's pack");
    assert_eq!(importer::summary_of(&index), "hello");
    assert_eq!(importer::version_of(&index), "1.2.3");
    assert_eq!(importer::minecraft_of(&index)?, "1.21.11");
    assert_eq!(importer::java_of(&index).as_deref(), Some("21"));
    assert_eq!(
        importer::loaders_of(&index),
        vec![("fabric".to_string(), "0.19.5".to_string())]
    );
    Ok(())
}

#[test]
fn importer_derives_kind_from_the_path() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("in.mrpack");
    write_mrpack(&path)?;
    let files = importer::files_of(&importer::read_index(&path)?)?;
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].kind(), PkgType::Mod);
    assert_eq!(files[1].kind(), PkgType::Resourcepack);
    assert_eq!(files[0].project_id, "AANobbMI");
    assert_eq!(files[0].version_id, "vvv");
    assert_eq!(files[1].env.client, Support::Optional);
    Ok(())
}

#[test]
fn importer_skips_configs_unless_asked() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("in.mrpack");
    write_mrpack(&path)?;
    let index = importer::read_index(&path)?;

    let mut out = Vec::new();
    let written = importer::extract_overrides(&path, dir.path(), false, &mut out)?;
    assert_eq!(written, 1, "only options.txt without the config");
    assert!(
        !dir.path().join("overrides/config/sodium/mixins.sodium.json").exists(),
        "configs should be left out"
    );
    assert!(dir.path().join("client-overrides/options.txt").exists());

    let mut out = Vec::new();
    let written = importer::extract_overrides(&path, dir.path(), true, &mut out)?;
    assert_eq!(written, 2, "options.txt plus the config");
    assert!(dir.path().join("overrides/config/sodium/mixins.sodium.json").exists());
    Ok(())
}

#[test]
fn importer_says_a_non_modrinth_file_cannot_be_tracked() -> anyhow::Result<()> {
    let file = fs::File::create("nothing.mrpack")?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("modrinth.index.json", opts)?;
    zip.write_all(
        br#"{"formatVersion":1,"files":[{"path":"mods/local.jar","env":{},"hashes":{},"downloads":[]}]}"#,
    )?;
    zip.finish()?;
    let index = importer::read_index(Path::new("nothing.mrpack"))?;
    let err = importer::files_of(&index).unwrap_err().to_string();
    assert!(err.contains("overrides"), "{err}");
    fs::remove_file("nothing.mrpack")?;
    Ok(())
}

async fn mock_modrinth() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/project/sodium"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "AANobbMI",
            "slug": "sodium",
            "title": "Sodium",
            "project_type": "mod",
            "author": "CaffeineMC",
            "loaders": ["fabric"],
            "game_versions": ["1.21.11"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/project/AANobbMI/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "ver-beta",
                "project_id": "AANobbMI",
                "name": "Sodium 0.7.0-beta",
                "version_number": "0.7.0-beta",
                "version_type": "beta",
                "date_published": "2026-09-01T00:00:00Z",
                "game_versions": ["1.21.11"],
                "loaders": ["fabric"],
                "files": [{
                    "filename": "sodium-fabric-0.7.0-beta.jar",
                    "url": "https://cdn.modrinth.com/data/AANobbMI/versions/ver-beta/s.jar",
                    "primary": true,
                    "size": 999,
                    "hashes": { "sha1": "cc", "sha512": "dd" }
                }]
            },
            {
                "id": "ver-release",
                "project_id": "AANobbMI",
                "name": "Sodium 0.6.0",
                "version_number": "0.6.0",
                "version_type": "release",
                "date_published": "2026-01-01T00:00:00Z",
                "game_versions": ["1.21.11"],
                "loaders": ["fabric"],
                "files": [
                    {
                        "filename": "sodium-fabric-0.6.0-sources.jar",
                        "url": "https://cdn.modrinth.com/data/AANobbMI/versions/ver-release/sources.jar",
                        "primary": false,
                        "size": 111,
                        "hashes": { "sha1": "ee", "sha512": "ff" }
                    },
                    {
                        "filename": "sodium-fabric-0.6.0.jar",
                        "url": "https://cdn.modrinth.com/data/AANobbMI/versions/ver-release/s.jar",
                        "primary": true,
                        "size": 1234,
                        "hashes": { "sha1": "aa", "sha512": "bb" }
                    }
                ]
            }
        ])))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn resolve_prefers_releases_and_the_primary_file() -> anyhow::Result<()> {
    let server = mock_modrinth().await;
    let client = Modrinth::new(server.uri())?;
    let resolved = client
        .resolve("sodium", Some("1.21.11"), &["fabric".to_string()])
        .await?;
    assert_eq!(resolved.project.slug, "sodium");
    assert_eq!(resolved.version.version_number, "0.6.0");
    assert_eq!(resolved.file.filename, "sodium-fabric-0.6.0.jar");
    assert_eq!(resolved.file.size, 1234);
    Ok(())
}

#[tokio::test]
async fn resolve_pinned_rejects_a_missing_version() -> anyhow::Result<()> {
    let server = mock_modrinth().await;
    let client = Modrinth::new(server.uri())?;
    let err = client
        .resolve_pinned("sodium", Some("1.21.11"), &["fabric".to_string()], "9.9.9")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("9.9.9"), "{err}");
    Ok(())
}

#[tokio::test]
async fn a_wrong_loader_is_a_clear_error() -> anyhow::Result<()> {
    let server = mock_modrinth().await;
    let client = Modrinth::new(server.uri())?;
    let err = client
        .resolve("sodium", Some("1.21.11"), &["quilt".to_string()])
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("supports"), "{err}");
    Ok(())
}

#[tokio::test]
async fn a_fabric_ask_never_hands_back_a_neoforge_build() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/project/sodium"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "AANobbMI",
            "slug": "sodium",
            "project_type": "mod",
            "loaders": ["fabric", "neoforge", "forge"],
            "game_versions": ["1.21.10", "1.21.11"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/project/AANobbMI/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "neo-new",
                "project_id": "AANobbMI",
                "version_number": "mc1.21.10-0.7.3-neoforge",
                "version_type": "release",
                "date_published": "2026-09-20T00:00:00Z",
                "game_versions": ["1.21.10"],
                "loaders": ["neoforge"],
                "files": [{ "filename": "sodium-neoforge.jar", "url": "http://x/n.jar",
                            "primary": true, "size": 1, "hashes": { "sha1": "a", "sha512": "b" } }]
            },
            {
                "id": "fabric-old",
                "project_id": "AANobbMI",
                "version_number": "mc1.21.10-0.6.6-fabric",
                "version_type": "release",
                "date_published": "2026-01-20T00:00:00Z",
                "game_versions": ["1.21.10"],
                "loaders": ["fabric"],
                "files": [{ "filename": "sodium-fabric.jar", "url": "http://x/f.jar",
                            "primary": true, "size": 2, "hashes": { "sha1": "c", "sha512": "d" } }]
            }
        ])))
        .mount(&server)
        .await;
    let client = Modrinth::new(server.uri())?;
    let got = client
        .resolve("sodium", Some("1.21.10"), &["fabric".to_string()])
        .await?;
    assert_eq!(got.version.version_number, "mc1.21.10-0.6.6-fabric");
    assert_eq!(got.file.filename, "sodium-fabric.jar");
    Ok(())
}

#[tokio::test]
async fn resolve_in_range_also_respects_the_loader() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/project/iris"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "iris", "slug": "iris", "project_type": "mod",
            "loaders": ["fabric"], "game_versions": ["1.21.11"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/project/iris/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "a", "project_id": "iris", "version_number": "1.10.7-fabric",
                "version_type": "release", "date_published": "2026-01-01T00:00:00Z",
                "game_versions": ["1.21.11"], "loaders": ["fabric"],
                "files": [{ "filename": "iris.jar", "url": "http://x/i.jar", "primary": true,
                            "size": 1, "hashes": { "sha1": "a", "sha512": "b" } }]
            }
        ])))
        .mount(&server)
        .await;
    let client = Modrinth::new(server.uri())?;
    let got = client
        .resolve_in_range("iris", Some("1.21.11"), &["fabric".to_string()], ">=1.0.0")
        .await?;
    assert_eq!(got.version.version_number, "1.10.7-fabric");
    let err = client
        .resolve_in_range("iris", Some("1.21.11"), &["fabric".to_string()], ">=99.0.0")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no version matching"), "{err}");
    Ok(())
}

#[tokio::test]
async fn resolve_outside_steps_away_from_a_break_range() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/project/iris"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "iris", "slug": "iris", "project_type": "mod",
            "loaders": ["fabric"], "game_versions": ["1.21.11"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/project/iris/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "a", "project_id": "iris", "version_number": "0.8.0",
                "version_type": "release", "date_published": "2026-01-01T00:00:00Z",
                "game_versions": ["1.21.11"], "loaders": ["fabric"],
                "files": [{ "filename": "iris-0.8.0.jar", "url": "http://x/080.jar",
                            "primary": true, "size": 1, "hashes": { "sha1": "a", "sha512": "b" } }]
            },
            {
                "id": "b", "project_id": "iris", "version_number": "1.0.0",
                "version_type": "release", "date_published": "2026-09-01T00:00:00Z",
                "game_versions": ["1.21.11"], "loaders": ["fabric"],
                "files": [{ "filename": "iris-1.0.0.jar", "url": "http://x/100.jar",
                            "primary": true, "size": 2, "hashes": { "sha1": "c", "sha512": "d" } }]
            }
        ])))
        .mount(&server)
        .await;
    let client = Modrinth::new(server.uri())?;
    let got = client
        .resolve_outside("iris", Some("1.21.11"), &["fabric".to_string()], "<0.9.0")
        .await?;
    assert_eq!(got.version.version_number, "1.0.0");
    let err = client
        .resolve_outside("iris", Some("1.21.11"), &["fabric".to_string()], "[0.0.0,9.9.9]")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no version out of"), "{err}");
    Ok(())
}

#[tokio::test]
async fn a_resource_pack_is_not_filtered_by_the_mod_loader() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/project/icons"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "ICC", "slug": "icons", "project_type": "resourcepack",
            "loaders": ["minecraft"], "game_versions": ["1.21.11"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/project/ICC/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "v1", "project_id": "ICC", "version_number": "1.13.4",
                "version_type": "release", "date_published": "2026-01-01T00:00:00Z",
                "game_versions": ["1.21.11"], "loaders": ["minecraft"],
                "files": [{ "filename": "icons.zip", "url": "http://x/icons.zip",
                            "primary": true, "size": 3, "hashes": { "sha1": "e", "sha512": "f" } }]
            }
        ])))
        .mount(&server)
        .await;
    let client = Modrinth::new(server.uri())?;
    let got = client
        .resolve("icons", Some("1.21.10"), &["fabric".to_string()])
        .await?;
    assert_eq!(got.project.project_type, "resourcepack");
    assert_eq!(got.version.version_number, "1.13.4");
    assert_eq!(got.file.filename, "icons.zip");
    Ok(())
}

#[tokio::test]
async fn batch_lookups_return_what_was_asked_for() -> anyhow::Result<()> {
    let server = mock_modrinth().await;
    Mock::given(method("GET"))
        .and(path("/versions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{
            "id": "ver-release",
            "project_id": "AANobbMI",
            "name": "Sodium 0.6.0",
            "version_number": "0.6.0",
            "version_type": "release",
            "date_published": "2026-01-01T00:00:00Z",
            "game_versions": ["1.21.11"],
            "loaders": ["fabric"],
            "files": []
        }])))
        .mount(&server)
        .await;
    let client = Modrinth::new(server.uri())?;
    let ids = vec!["ver-release".to_string()];
    let found = client.versions_by_ids(&ids).await?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].version_number, "0.6.0");
    assert!(client.versions_by_ids(&[]).await?.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "hits the real Modrinth API"]
async fn live_smoke_resolves_a_real_project() -> anyhow::Result<()> {
    let client = Modrinth::new(DEFAULT_API)?;
    let r = client
        .resolve("sodium", Some("1.21.11"), &["fabric".to_string()])
        .await?;
    println!("{} {} -> {}", r.project.slug, r.version.version_number, r.file.filename);
    Ok(())
}