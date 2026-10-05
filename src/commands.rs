use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use crate::build;
use crate::importer;
use crate::loaders::{self, LoaderKind};
use crate::lock::{Lock, LockedPackage, LockedTarget};
use crate::manifest::{
    self, Env, LoaderSpec, Manifest, Package, PkgType, RpPosition, Softwares, Support, Target,
    Targetish, LOCK_FILE, MANIFEST_FILE, SOFTWARES_FILE,
};
use crate::minecraft;
use crate::modmeta;
use futures_util::StreamExt;
use crate::modrinth::{
    is_modrinth_id, AuthFailure, Category, CreateVersion, Disclosure, Environment, Modrinth,
    Resolved, SearchFilters, SearchHit, Sort, DEFAULT_API,
};
use crate::scaffold::{self, NewPlan};
use crate::secret;
use crate::staging;
use crate::validate;
use crate::Launcher;
use crate::{Cli, Cmd, WorkflowKind};

pub async fn dispatch(cli: Cli) -> Result<()> {
    let api = cli
        .api
        .clone()
        .unwrap_or_else(|| DEFAULT_API.to_string());
    match &cli.cmd {
        Cmd::Search {
            query,
            kind,
            mc,
            loader,
            sort,
            limit,
            page,
        } => {
            let browsing = query.as_deref().unwrap_or("").trim().is_empty();
            let sort = sort.or(if browsing { Some(Sort::Downloads) } else { None });
            let seeded = SearchFilters {
                query: query.clone().unwrap_or_default(),
                project_types: kind.iter().map(|k| k.modrinth_type().to_string()).collect(),
                versions: mc.iter().cloned().collect(),
                loaders: loader.map(|l| l.as_str().to_string()).into_iter().collect(),
                sort,
                limit: *limit,
                page: *page,
                ..SearchFilters::default()
            };
            if std::io::stdin().is_terminal() {
                interactive_search(&cli, &api, seeded).await
            } else {
                search_page(&cli, &api, &seeded).await
            }
        }
        Cmd::Browse {
            kind,
            mc,
            loader,
            sort,
            limit,
        } => browse(&cli, &api, *kind, mc.as_deref(), *loader, *sort, *limit).await,
        Cmd::Info { project } => {
            let project_type = info(&cli, &api, project).await?;
            if std::io::stdin().is_terminal() {
                println!();
                after_show(&cli, project, &project_type, true).await?;
            }
            Ok(())
        }
        Cmd::Add {
            project,
            version,
            mc,
            kind,
        } => {
            add(
                &cli,
                &api,
                project,
                version.as_deref(),
                mc.as_deref(),
                *kind,
            )
            .await
        }
        Cmd::Remove { project } => remove(&cli, project),
        Cmd::Find { query, remove } => find_cmd(&cli, query, *remove),
        Cmd::List { mc, loader } => list(&cli, mc.as_deref(), *loader),
        Cmd::Lock { mc, loader, check } => lock_cmd(&cli, &api, mc.as_deref(), *loader, *check).await,
        Cmd::Build {
            mc,
            loader,
            locked,
            out,
            all,
        } => build_cmd(&cli, &api, mc.as_deref(), *loader, *locked, out, *all).await,
        Cmd::New {
            name,
            folder,
            mc,
            loader,
            loader_version,
            workflow,
            git,
            force,
        } => {
            let partial = NewPlan {
                name: name.clone(),
                folder: folder.clone(),
                minecraft: mc.clone(),
                loaders: loader.clone(),
                loader_version: loader_version.clone(),
                workflow: *workflow,
                git: None,
            };
            new_cmd(&cli, &api, partial, *git, *force).await
        }
        Cmd::NewFromMrpack {
            mrpack,
            name,
            folder,
            include_configs,
            workflow,
            git,
            force,
        } => {
            new_from_mrpack(
                &cli,
                &api,
                mrpack,
                name.as_deref(),
                folder.as_deref(),
                *include_configs,
                *workflow,
                *git,
                *force,
            )
            .await
        }
        Cmd::AddVer { version, loader } => add_ver(&cli, &api, version, loader).await,
        Cmd::Update {
            mc,
            loader,
            dry_run,
            update_changelogs,
        } => update_cmd(&cli, &api, mc.as_deref(), *loader, *dry_run, *update_changelogs).await,
        Cmd::OneVer { version } => one_ver(&cli, &api, version).await,
        Cmd::SyncPackActive {
            mc,
            loader,
            no_download,
        } => sync_pack_active(&cli, &api, mc.as_deref(), *loader, *no_download).await,
        Cmd::Order {
            mc,
            loader,
            off,
            packs,
        } => order_cmd(&cli, mc.as_deref(), *loader, *off, packs.clone()),
        Cmd::FixLoader {
            mc,
            loader,
            all,
            dry_run,
        } => fix_loader(&cli, &api, mc.as_deref(), *loader, *all, *dry_run).await,
        Cmd::Verify {
            mc,
            loader,
            all,
            keep_going,
        } => verify_cmd(&cli, &api, mc.as_deref(), *loader, *all, *keep_going).await,
        Cmd::Publish {
            token,
            project_id,
            out,
            mc,
            loader,
        } => publish(
            &cli,
            &api,
            token.as_deref(),
            project_id.as_deref(),
            out,
            mc.as_deref(),
            *loader,
        )
        .await,
        Cmd::Validate => validate_cmd(&cli).await,
        Cmd::SetRelease { version } => set_release(&cli, &api, &version).await,
        Cmd::UpdateSchema => update_schema(&cli),
        Cmd::Test {
            ver,
            loader,
            all,
            launcher,
            launch,
            profile,
            keep,
        } => {
            test_cmd(
                &cli,
                &api,
                ver.as_deref(),
                *loader,
                *all,
                *launcher,
                launch.as_deref(),
                profile.as_deref(),
                *keep,
            )
            .await
        }
    }
}

struct Project {
    dir: PathBuf,
    manifest: Manifest,
    softwares: Softwares,
}

fn load_project(cli: &Cli) -> Result<Project> {
    let dir = manifest::find_manifest(&cli.dir)?;
    let pack = Manifest::load(&dir)?;
    let softwares = Softwares::load(&dir)?
        .context("no softwares.json here, run `ehmodpack new` first")?;
    Ok(Project {
        dir,
        manifest: pack,
        softwares,
    })
}

fn loaders_for(target: &Target) -> Vec<String> {
    vec![target.loader.kind.clone()]
}

fn version_candidates(pkg: &Package, target: &Target) -> Vec<String> {
    let key = format!("{}+{}", target.minecraft, target.loader.kind);
    let mut out: Vec<String> = Vec::new();
    let push = |value: Option<&String>, out: &mut Vec<String>| {
        if let Some(v) = value {
            let v = v.trim();
            if !v.is_empty() && v != "*" && !out.iter().any(|e| e == v) {
                out.push(v.to_string());
            }
        }
    };
    push(pkg.versions.get(&key), &mut out);
    push(pkg.versions.get(&target.minecraft), &mut out);
    push(pkg.versions.get("default"), &mut out);
    push(pkg.versions.get("*"), &mut out);
    push(pkg.version.as_ref(), &mut out);
    out
}

async fn resolve_one(
    client: &Modrinth,
    target: &Target,
    pkg: &Package,
) -> Result<Resolved> {
    let key = pkg.key();
    let loaders = loaders_for(target);
    let mut last = String::new();
    for wanted in version_candidates(pkg, target) {
        match client
            .resolve_pinned(&key, Some(&target.minecraft), &loaders, &wanted)
            .await
        {
            Ok(found) => {
                ensure_kind_fits(pkg.kind, &found)?;
                return Ok(found);
            }
            Err(e) => last = format!("{wanted} ({e:#})"),
        }
    }
    if !last.is_empty() {
        detail_note(&format!("{key} had no {} on this target, taking the newest", last));
    }
    let found = client
        .resolve(&key, Some(&target.minecraft), &loaders)
        .await
        .with_context(|| format!("{key} has nothing for {}", target.label()))?;
    ensure_kind_fits(pkg.kind, &found)?;
    Ok(found)
}

fn ensure_kind_fits(kind: PkgType, found: &Resolved) -> Result<()> {
    if kind != PkgType::Mod {
        return Ok(());
    }
    if found.project.project_type == "mod" {
        return Ok(());
    }
    bail!(
        "{} is a {}, it cannot be a mod in mods/",
        found.project.slug,
        found.project.project_type
    )
}

fn detail_note(text: &str) {
    if crate::progress::interactive() {
        println!("\r  {text}");
    }
}

async fn unsupported_note(client: &Modrinth, key: &str, target: &Target) -> String {
    let what = match client.project(key).await {
        Ok(p) => format!(
            "{} on {}",
            p.loaders.join("/"),
            if p.game_versions.is_empty() {
                "no minecraft versions".to_string()
            } else {
                format!("{} versions", p.game_versions.len())
            }
        ),
        Err(_) => "nothing we can see".to_string(),
    };
    format!(
        "{key} only supports {what} and we couldn't find one for {}, so building the modpack will don't have this {key}",
        target.label()
    )
}

fn to_locked(p: &Package, r: &Resolved, target: &Target) -> LockedPackage {
    let stackable = matches!(p.kind, PkgType::Resourcepack | PkgType::Shader);
    let position = if stackable && p.is_active() {
        Some(
            p.position_for(&target.minecraft, &target.loader.kind)
                .unwrap_or_default(),
        )
    } else {
        None
    };
    let env = if position.is_some() {
        Env {
            client: Support::Required,
            server: Support::Unsupported,
        }
    } else {
        p.env()
    };
    LockedPackage {
        kind: p.kind,
        project: r.project.slug.clone(),
        project_id: r.project.id.clone(),
        version_id: r.version.id.clone(),
        version_number: r.version.version_number.clone(),
        filename: r.file.filename.clone(),
        path: format!("{}/{}", p.kind.folder(), r.file.filename),
        hashes: r.file.hashes.clone(),
        downloads: vec![r.file.url.clone()],
        file_size: r.file.size,
        env,
        position,
        source: "modrinth".to_string(),
    }
}

fn version_key(target: &Target, single: bool) -> String {
    if single {
        "default".to_string()
    } else {
        format!("{}+{}", target.minecraft, target.loader.kind)
    }
}

fn kind_of(project_type: &str) -> PkgType {
    match project_type {
        "resourcepack" => PkgType::Resourcepack,
        "shader" => PkgType::Shader,
        "datapack" => PkgType::Datapack,
        _ => PkgType::Mod,
    }
}

async fn keep_loader(client: &Modrinth, hits: Vec<SearchHit>, loader: LoaderKind) -> Vec<SearchHit> {
    let mut kept = Vec::new();
    for hit in hits {
        match client.project(&hit.slug).await {
            Ok(p) if p.loaders.iter().any(|l| l.eq_ignore_ascii_case(loader.as_str())) => {
                kept.push(hit)
            }
            Ok(_) => {}
            Err(_) => kept.push(hit),
        }
    }
    kept
}

#[allow(clippy::too_many_arguments)]
fn summarise_versions(versions: &[String]) -> String {
    if versions.is_empty() {
        return "unknown".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    for v in versions {
        if !v.starts_with('1') {
            continue;
        }
        let short = v.rsplit_once('-').map(|(a, _)| a).unwrap_or(v);
        let parts: Vec<&str> = short.split('.').collect();
        let line = if parts.len() >= 2 {
            format!("{}.{}", parts[0], parts[1])
        } else {
            short.to_string()
        };
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    let head: Vec<String> = lines.iter().take(3).cloned().collect();
    format!(
        "{} ({} versions)",
        head.join(", "),
        versions.len()
    )
}

async fn with_details(
    client: &Modrinth,
    hits: &[SearchHit],
) -> Vec<(String, String)> {
    let jobs = hits.iter().map(|h| {
        let slug = h.slug.clone();
        async move { client.project(&slug).await }
    });
    let projects: Vec<anyhow::Result<crate::modrinth::Project>> =
        futures_util::stream::iter(jobs).buffer_unordered(12).collect().await;
    projects
        .into_iter()
        .map(|p| match p {
            Ok(p) => (
                if p.loaders.is_empty() {
                    "unknown".to_string()
                } else {
                    p.loaders.join(", ")
                },
                summarise_versions(&p.game_versions),
            ),
            Err(_) => ("unknown".to_string(), "unknown".to_string()),
        })
        .collect()
}

async fn search_page(cli: &Cli, api: &str, filters: &SearchFilters) -> Result<()> {
    let client = Modrinth::new(api)?;
    let mut found = client.search_page(filters).await?;
    if let Some(want) = filters.loaders.first() {
        if let Some(kind) = LoaderKind::parse(want) {
            found.hits = keep_loader(&client, found.hits.clone(), kind).await;
        }
    }
    if found.hits.is_empty() {
        say(
            cli,
            &if filters.query.is_empty() {
                "nothing came back".to_string()
            } else {
                format!("nothing matched {:?}", filters.query)
            },
        );
        return Ok(());
    }
    let added = added_slugs(cli);
    let details = with_details(&client, &found.hits).await;
    println!(
        "{}",
        hits_table(cli, &found.hits, &added, &details)
    );
    let limit = filters.limit.clamp(1, 100);
    let pages = found.total_hits.div_ceil(limit);
    println!(
        "\npage {} of {pages}, showing {} of {} results",
        filters.page,
        found.hits.len(),
        found.total_hits
    );
    let active = filters.active_lines();
    if !active.is_empty() {
        println!("filters: {}", active.join(" | "));
    }
    if filters.page < pages {
        println!("next: ehmodpack search --page {}", filters.page + 1);
    }
    println!("look at one with: ehmodpack info <slug>");
    println!("add one with: ehmodpack add <slug>");
    Ok(())
}

fn added_slugs(cli: &Cli) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(dir) = manifest::find_manifest(&cli.dir) else {
        return out;
    };
    let Ok(pack) = Manifest::load(&dir) else {
        return out;
    };
    for p in &pack.packages {
        if let Some(slug) = &p.project {
            out.insert(slug.to_ascii_lowercase());
        }
        if let Some(id) = &p.id {
            out.insert(id.to_ascii_lowercase());
        }
    }
    out
}

enum Nav {
    Next,
    Prev,
    EditFilters,
    EditCategory,
    BackToStart,
    NewSearch,
    Quit,
}

fn marked(chosen: &[String], name: &str) -> String {
    if chosen.iter().any(|c| c == name) {
        format!("{name}  (on)")
    } else {
        format!("{name}  (off)")
    }
}

const BACK: &str = "back";

fn with_back(mut options: Vec<String>) -> Vec<String> {
    options.push(BACK.to_string());
    options
}

fn went_back(chosen: &[String]) -> bool {
    chosen.iter().any(|c| c == BACK)
}

fn picked_indices(options: &[String], chosen: &[String]) -> Vec<usize> {
    options
        .iter()
        .enumerate()
        .filter(|(_, o)| chosen.iter().any(|c| c == *o))
        .map(|(i, _)| i)
        .collect()
}

async fn edit_category(
    cli: &Cli,
    client: &Modrinth,
    cats: &mut Option<Vec<Category>>,
    filters: &mut SearchFilters,
) -> Result<()> {
    if cats.is_none() {
        say(cli, "asking modrinth what categories it has...");
        *cats = Some(client.categories().await?);
    }
    let list = cats.as_ref().expect("just filled");
    let keep = [
        PkgType::Mod,
        PkgType::Resourcepack,
        PkgType::Shader,
        PkgType::Datapack,
    ]
    .iter()
    .map(|k| k.modrinth_type())
    .collect::<Vec<_>>();
    let list: Vec<&Category> = list
        .iter()
        .filter(|c| keep.contains(&c.project_type.as_str()))
        .collect();

    let mut groups: Vec<(String, Vec<&Category>)> = Vec::new();
    for c in &list {
        if let Some(slot) = groups.iter_mut().find(|(k, _)| *k == c.project_type) {
            slot.1.push(c);
        } else {
            groups.push((c.project_type.clone(), vec![c]));
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut names: Vec<String> = Vec::new();
    for (kind, items) in &groups {
        for c in items {
            let header = match c.header.trim() {
                "" | "categories" => String::new(),
                h => format!("  ({h})"),
            };
            names.push(format!("{kind} / {}{header}", c.name));
        }
    }

    loop {
        let menu = vec![
            "only show these categories".to_string(),
            "hide these categories".to_string(),
            "back".to_string(),
        ];
        let picked = inquire::Select::new("edit category", menu.clone())
            .prompt()
            .map_err(ask_failed)?;
        let Some(i) = menu.iter().position(|o| *o == picked) else {
            continue;
        };
        match i {
            0 => {
                let current = filters.categories.clone();
                let options: Vec<String> =
                    names.iter().map(|n| marked(&current, n)).collect();
                let chosen = inquire::MultiSelect::new(
                    "only show projects with these categories",
                    with_back(options.clone()),
                )
                .with_page_size(24)
                .prompt()
                .map_err(ask_failed)?;
                if went_back(&chosen) {
                    continue;
                }

                filters.categories = picked_indices(&options, &chosen)
                    .iter()
                    .filter_map(|i| names.get(*i).cloned())
                    .collect();
                filters.project_types = groups
                    .iter()
                    .filter(|(_, items)| {
                        items.iter().any(|c| {
                            filters.categories.iter().any(|f| f.ends_with(&c.name))
                        })
                    })
                    .map(|(k, _)| k.clone())
                    .collect();
            }
            1 => {
                let base: Vec<String> = list
                    .iter()
                    .map(|c| c.name.clone())
                    .filter(|n| !filters.categories.contains(n))
                    .collect();
                let picked =
                    inquire::MultiSelect::new("hide these categories", with_back(base.clone()))
                        .with_page_size(24)
                        .prompt()
                        .map_err(ask_failed)?;
                if went_back(&picked) {
                    continue;
                }
                let mut exclude: Vec<String> = Vec::new();
                for label in &picked {
                    if let Some(name) = base.iter().find(|b| *b == label) {
                        exclude.push(name.clone());
                    }
                }
                filters.exclude_categories = exclude;
            }
            _ => return Ok(()),
        }
    }
}

async fn edit_filters(cli: &Cli, client: &Modrinth, filters: &mut SearchFilters) -> Result<()> {
    loop {
        let active = filters.active_lines();
        let header = if active.is_empty() {
            "filters, nothing set".to_string()
        } else {
            format!("filters, {}", active.join(" | "))
        };
        let options = vec![
            "Open Source".to_string(),
            "Supports".to_string(),
            "Disclosure Exclusions".to_string(),
            "Only with these disclosures".to_string(),
            "Minecraft Version".to_string(),
            "loader".to_string(),
            "Sorted By".to_string(),
            "Clear every filter".to_string(),
            "Back".to_string(),
        ];
        let picked = inquire::Select::new(&header, options.clone())
            .with_page_size(12)
            .prompt()
            .map_err(ask_failed)?;
        let Some(i) = options.iter().position(|o| *o == picked) else {
            continue;
        };

        match i {
            0 => {
                let open = inquire::Select::new(
                    "Open Source",
                    with_back(vec![
                        "any".to_string(),
                        "only open source".to_string(),
                        "only closed source".to_string(),
                    ]),
                )
                .prompt()
                .map_err(ask_failed)?;
                if went_back(std::slice::from_ref(&open)) {
                    continue;
                }
                filters.open_source = match open.as_str() {
                    "only open source" => Some(true),
                    "only closed source" => Some(false),
                    _ => None,
                };
            }
            1 => {
                let current: Vec<String> = filters
                    .environments
                    .iter()
                    .map(|e| e.label().to_string())
                    .collect();
                let options: Vec<String> = Environment::all()
                    .iter()
                    .map(|e| marked(&current, e.label()))
                    .collect();
                let chosen = inquire::MultiSelect::new(
                    "Supports",
                    with_back(options.clone()),
                )
                .with_page_size(12)
                .prompt()
                .map_err(ask_failed)?;
                if went_back(&chosen) {
                    continue;
                }
                filters.environments = picked_indices(&options, &chosen)
                    .iter()
                    .filter_map(|i| Environment::all().get(*i).copied())
                    .collect();
            }
            2 => {
                let current: Vec<String> = filters
                    .exclude_disclosures
                    .iter()
                    .map(|d| d.label().to_string())
                    .collect();
                let options: Vec<String> = Disclosure::all()
                    .iter()
                    .map(|d| marked(&current, d.label()))
                    .collect();
                let chosen = inquire::MultiSelect::new(
                    "Hides any project with any of the disclosures applied",
                    with_back(options.clone()),
                )
                .with_page_size(12)
                .prompt()
                .map_err(ask_failed)?;
                if went_back(&chosen) {
                    continue;
                }
                filters.exclude_disclosures = picked_indices(&options, &chosen)
                    .iter()
                    .filter_map(|i| Disclosure::all().get(*i).copied())
                    .collect();
            }
            3 => {
                let current: Vec<String> = filters
                    .include_disclosures
                    .iter()
                    .map(|d| d.label().to_string())
                    .collect();
                let options: Vec<String> = Disclosure::all()
                    .iter()
                    .map(|d| marked(&current, d.label()))
                    .collect();
                let chosen = inquire::MultiSelect::new(
                    "Only with these disclosures",
                    with_back(options.clone()),
                )
                .with_page_size(12)
                .prompt()
                .map_err(ask_failed)?;
                if went_back(&chosen) {
                    continue;
                }
                filters.include_disclosures = picked_indices(&options, &chosen)
                    .iter()
                    .filter_map(|i| Disclosure::all().get(*i).copied())
                    .collect();
            }
            4 => {
                let current = filters.versions.first().cloned().unwrap_or_default();
                let raw = ask(&format!(
                    "Minecraft Version (currently {:?}, \"any\" clears it, \"back\" keeps it)",
                    current
                ))?;
                let trimmed = raw.trim().to_string();
                if trimmed.eq_ignore_ascii_case("back") || trimmed.is_empty() {
                    continue;
                }
                filters.versions = if trimmed.eq_ignore_ascii_case("any") || trimmed == "*" {
                    Vec::new()
                } else {
                    trimmed.split(',').map(|s| s.trim().to_string()).collect()
                };
            }
            5 => {
                let tags = client.loaders().await?;
                let current = filters.loaders.clone();
                let options: Vec<String> = tags
                    .iter()
                    .map(|t| marked(&current, &t.name))
                    .collect();
                let chosen =
                    inquire::MultiSelect::new("loader", with_back(options.clone()))
                        .with_page_size(16)
                        .prompt()
                        .map_err(ask_failed)?;
                if went_back(&chosen) {
                    continue;
                }
                filters.loaders = picked_indices(&options, &chosen)
                    .iter()
                    .filter_map(|i| tags.get(*i).map(|t| t.name.clone()))
                    .collect();
            }
            6 => {
                let choices = [
                    ("relevance", Sort::Relevance),
                    ("follows", Sort::Follows),
                    ("downloads", Sort::Downloads),
                    ("newest", Sort::Newest),
                    ("updated", Sort::Updated),
                ];
                let mut options: Vec<String> =
                    choices.iter().map(|(n, _)| (*n).to_string()).collect();
                options.push(BACK.to_string());
                let picked = inquire::Select::new("Sorted By", options)
                    .prompt()
                    .map_err(ask_failed)?;
                if picked == BACK {
                    continue;
                }
                filters.sort = choices
                    .iter()
                    .find(|(n, _)| *n == picked)
                    .map(|(_, s)| *s);
            }
            7 => {
                filters.project_types.clear();
                filters.categories.clear();
                filters.exclude_categories.clear();
                filters.versions.clear();
                filters.loaders.clear();
                filters.environments.clear();
                filters.open_source = None;
                filters.include_disclosures.clear();
                filters.exclude_disclosures.clear();
                warn(cli, "cleared every filter");
            }
            _ => return Ok(()),
        }
    }
}

async fn interactive_search(cli: &Cli, api: &str, mut filters: SearchFilters) -> Result<()> {
    let client = Modrinth::new(api)?;
    let mut added = added_slugs(cli);
    let mut cats: Option<Vec<Category>> = None;
    let mut at_menu = false;

    loop {
        if filters.query.trim().is_empty() || at_menu {
            at_menu = false;
            let active = filters.active_lines();
            let header = if active.is_empty() {
                "Search for what in modrinth".to_string()
            } else {
                format!("Search for what in modrinth, filters: {}", active.join(" | "))
            };
            let options = vec![
                "search for something".to_string(),
                "edit category".to_string(),
                "edit filters".to_string(),
                "quit".to_string(),
            ];
            let picked = inquire::Select::new(&header, options.clone())
                .prompt()
                .map_err(ask_failed)?;
            let Some(i) = options.iter().position(|o| *o == picked) else {
                continue;
            };
            match i {
                0 => {
                    let raw = ask("search modrinth for")?;
                    filters.query = raw.trim().to_string();
                    filters.page = 1;
                    if filters.query.is_empty() {
                        continue;
                    }
                }
                1 => {
                    edit_category(cli, &client, &mut cats, &mut filters).await?;
                    continue;
                }
                2 => {
                    edit_filters(cli, &client, &mut filters).await?;
                    continue;
                }
                _ => return Ok(()),
            }
        }

        loop {
            let mut page = filters.page;
            let found = client.search_page(&filters).await?;
            if found.hits.is_empty() {
                warn(cli, &format!("nothing matched {:?}", filters.query));
                filters.page = 1;
                break;
            }
            let limit = filters.limit.clamp(1, 100);
            let pages = found.total_hits.div_ceil(limit);
            let details = with_details(&client, &found.hits).await;
            let mut options: Vec<String> = found
                .hits
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let mark = if added.contains(&h.slug.to_ascii_lowercase()) {
                        " (added)"
                    } else {
                        ""
                    };
                    let loaders = details
                        .get(i)
                        .map(|(l, _)| l.clone())
                        .unwrap_or_default();
                    format!(
                        "{:<26} {:>9}{mark}  {:<22} {}",
                        h.slug,
                        thousands(h.downloads),
                        loaders,
                        truncate(&h.description, 30)
                    )
                })
                .collect();

            let first_nav = options.len();
            let mut navs: Vec<Nav> = Vec::new();
            if page < pages {
                options.push(format!("next page ({}/{})", page + 1, pages));
                navs.push(Nav::Next);
            }
            if page > 1 {
                options.push("previous page".to_string());
                navs.push(Nav::Prev);
            }
            options.push("edit filters".to_string());
            navs.push(Nav::EditFilters);
            options.push("edit category".to_string());
            navs.push(Nav::EditCategory);
            options.push("search again".to_string());
            navs.push(Nav::NewSearch);
            options.push("back to the start".to_string());
            navs.push(Nav::BackToStart);
            options.push("quit".to_string());
            navs.push(Nav::Quit);

            let active = filters.active_lines();
            let mut header = format!(
                "\"{}\"  -  {} of {} results",
                filters.query,
                found.hits.len() + (page - 1) * limit,
                found.total_hits
            );
            if !active.is_empty() {
                header.push_str(&format!("\nfilters: {}", active.join(" | ")));
            }
            let picked = inquire::Select::new(&header, options.clone())
                .with_page_size(20)
                .prompt()
                .map_err(ask_failed)?;
            let Some(choice) = options.iter().position(|o| *o == picked) else {
                continue;
            };

            if choice < first_nav {
                let hit = found.hits[choice].clone();
                pick_hit(cli, api, &hit).await?;
                added = added_slugs(cli);
                continue;
            }
            match navs[choice - first_nav] {
                Nav::Next => {
                    page += 1;
                    filters.page = page;
                }
                Nav::Prev => {
                    page = page.saturating_sub(1);
                    filters.page = page;
                }
                Nav::EditFilters => {
                    edit_filters(cli, &client, &mut filters).await?;
                    filters.page = 1;
                }
                Nav::EditCategory => {
                    edit_category(cli, &client, &mut cats, &mut filters).await?;
                    filters.page = 1;
                }
                Nav::NewSearch => {
                    filters.query.clear();
                    filters.page = 1;
                    break;
                }
                Nav::BackToStart => {
                    at_menu = true;
                    filters.page = 1;
                    break;
                }
                Nav::Quit => return Ok(()),
            }
        }
    }
}

fn hit_line(h: &SearchHit, added: &BTreeSet<String>) -> String {
    let mark = if added.contains(&h.slug.to_ascii_lowercase())
        || added.contains(&h.project_id.to_ascii_lowercase())
    {
        " (added)"
    } else {
        ""
    };
    format!(
        "{:<26} {:>9}{mark}  {}",
        h.slug,
        thousands(h.downloads),
        truncate(&h.description, 46)
    )
}

fn hits_table(
    cli: &Cli,
    hits: &[SearchHit],
    added: &BTreeSet<String>,
    details: &[(String, String)],
) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut flags: Vec<bool> = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let is_added = added.contains(&h.slug.to_ascii_lowercase())
            || added.contains(&h.project_id.to_ascii_lowercase());
        let (loaders, versions) = details
            .get(i)
            .cloned()
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
        let mut row = vec![
            h.slug.clone(),
            h.project_type.clone(),
            thousands(h.downloads),
            loaders,
            versions,
            truncate(&h.description, 34),
        ];
        if is_added {
            row.push("(added)".to_string());
        }
        rows.push(row);
        flags.push(is_added);
    }
    let mut headers = vec![
        "slug",
        "type",
        "downloads",
        "loaders",
        "minecraft",
        "description",
    ];
    if added_had_any(&flags) {
        headers.push("");
    }
    table(cli, &headers, &rows, &flags)
}

fn added_had_any(flags: &[bool]) -> bool {
    flags.iter().any(|f| *f)
}

fn truncate(text: &str, max: usize) -> String {
    let flat = text.replace(['\n', '\r', '\t'], " ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

async fn info(cli: &Cli, api: &str, project: &str) -> Result<String> {
    let client = Modrinth::new(api)?;
    let p = client.project(project).await?;
    let people = client.people_of(&p).await;
    let disclosures = client.disclosures_of(&p).await.unwrap_or_default();
    say(cli, &p.title);
    println!("  slug       {}", p.slug);
    println!("  id         {}", p.id);
    println!("  type       {}", p.project_type);
    println!(
        "  by         {}",
        if people.is_empty() {
            "unknown".to_string()
        } else {
            people.join(", ")
        }
    );
    println!("  downloads  {}", thousands(p.downloads));
    println!("  followers  {}", thousands(p.followers));
    println!("  updated    {}", short_date(&p.updated));
    println!("  loaders    {}", p.loaders.join(", "));
    println!("  versions   {}", p.game_versions.len());
    if let Some(l) = &p.license {
        println!("  license    {} ({})", l.name, l.id);
    }
    for (label, url) in [
        ("source", &p.source_url),
        ("issues", &p.issues_url),
        ("wiki", &p.wiki_url),
        ("discord", &p.discord_url),
    ] {
        if let Some(url) = url {
            println!("  {label:<10} {url}");
        }
    }
    for d in &p.donation_urls {
        if d.url.is_empty() {
            continue;
        }
        let label = if d.platform.is_empty() {
            d.id.clone()
        } else {
            d.platform.clone()
        };
        println!("  donate     {label}  {}", d.url);
    }
    if !disclosures.is_empty() {
        println!("  disclosures");
        for d in &disclosures {
            println!("    {}", d.heading());
            for line in d.lines() {
                println!("      {line}");
            }
        }
    }
    if !p.description.is_empty() {
        println!("\n{}", p.description);
    }
    Ok(p.project_type)
}

fn pick_targets(p: &Project) -> Result<Vec<Target>> {
    let all = &p.softwares.targets;
    if all.len() <= 1 {
        return Ok(all.clone());
    }
    let labels: Vec<String> = all.iter().map(|t| t.label()).collect();
    let options: Vec<String> = labels
        .iter()
        .cloned()
        .chain(std::iter::once("all of them".to_string()))
        .collect();
    let picked = inquire::MultiSelect::new(
        &format!("{} has {} targets, where should this go?", MANIFEST_FILE, all.len()),
        options,
    )
    .prompt()
    .map_err(ask_failed)?;
    if picked.iter().any(|o| o == "all of them") || picked.is_empty() {
        return Ok(all.clone());
    }
    let chosen: Vec<Target> = labels
        .iter()
        .enumerate()
        .filter(|(_, l)| picked.iter().any(|p| p == *l))
        .filter_map(|(i, _)| all.get(i).cloned())
        .collect();
    if chosen.is_empty() {
        bail!("nothing to install into, {MANIFEST_FILE} targets got emptied");
    }
    Ok(chosen)
}

fn short_date(stamp: &str) -> String {
    stamp.chars().take(10).collect()
}

fn is_addable_kind(project_type: &str) -> bool {
    matches!(
        project_type,
        "mod" | "resourcepack" | "shader" | "datapack"
    )
}

async fn after_show(cli: &Cli, slug: &str, project_type: &str, from_info: bool) -> Result<()> {
    let addable = is_addable_kind(project_type);
    let leave = if from_info { "Exit" } else { "Go back" };
    if !addable {
        say(
            cli,
            &format!("{slug} is a {project_type}, a pack has nowhere to put that"),
        );
        println!("{}", grey(cli, &format!("this is a {project_type}")));
    }
    let add_label = if addable {
        "Add this".to_string()
    } else {
        grey(cli, "Add this")
    };
    let menu = vec![add_label, leave.to_string()];

    loop {
        let picked = inquire::Select::new("What do you want to do?", menu.clone())
            .with_starting_cursor(0)
            .prompt()
            .map_err(ask_failed)?;
        let Some(choice) = menu.iter().position(|o| *o == picked) else {
            continue;
        };
        if choice != 0 {
            return Ok(());
        }
        if !addable {
            say(cli, &format!("{slug} is a {project_type}, so no"));
            continue;
        }
        let Ok(p) = load_project(cli) else {
            println!("add it with: ehmodpack add {slug}");
            return Ok(());
        };
        if added_slugs(cli).contains(&slug.to_ascii_lowercase()) {
            say(
                cli,
                &format!("{slug} is already in {MANIFEST_FILE}, leaving it be"),
            );
            return Ok(());
        }
        let targets = pick_targets(&p)?;
        add_project(cli, &p, slug, None, targets).await?;
        return Ok(());
    }
}

async fn pick_hit(cli: &Cli, api: &str, hit: &SearchHit) -> Result<()> {
    info(cli, api, &hit.slug).await?;
    println!();
    after_show(cli, &hit.slug, &hit.project_type, false).await
}

#[allow(clippy::too_many_arguments)]
async fn browse(
    cli: &Cli,
    api: &str,
    kind: Option<PkgType>,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    sort: Sort,
    limit: usize,
) -> Result<()> {
    let mut filters = SearchFilters {
        project_types: kind.iter().map(|k| k.modrinth_type().to_string()).collect(),
        versions: mc.iter().copied().map(str::to_string).collect(),
        loaders: loader.map(|l| l.as_str().to_string()).into_iter().collect(),
        sort: Some(sort),
        limit,
        page: 1,
        ..SearchFilters::default()
    };
    if !std::io::stdin().is_terminal() {
        return search_page(cli, api, &filters).await;
    }
    let client = Modrinth::new(api)?;
    let mut added = added_slugs(cli);
    loop {
        let mut found = client.search_page(&filters).await?;
        if let Some(want) = filters.loaders.first() {
            if let Some(kind) = LoaderKind::parse(want) {
                found.hits = keep_loader(&client, found.hits.clone(), kind).await;
            }
        }
        if found.hits.is_empty() {
            say(cli, "nothing came back");
            return Ok(());
        }
        let limit = filters.limit.clamp(1, 100);
        let pages = found.total_hits.div_ceil(limit);
        let mut options: Vec<String> =
            found.hits.iter().map(|h| hit_line(h, &added)).collect();
        let first_nav = options.len();
        let mut navs: Vec<Nav> = Vec::new();
        if filters.page < pages {
            options.push(format!("next page ({}/{})", filters.page + 1, pages));
            navs.push(Nav::Next);
        }
        if filters.page > 1 {
            options.push("previous page".to_string());
            navs.push(Nav::Prev);
        }
        options.push("edit filters".to_string());
        navs.push(Nav::EditFilters);
        options.push("quit".to_string());
        navs.push(Nav::Quit);

        let active = filters.active_lines();
        let mut header = format!(
            "browsing by {}  -  {} of {}  -  enter adds it",
            sort.as_str(),
            found.hits.len() + (filters.page - 1) * limit,
            found.total_hits
        );
        if !active.is_empty() {
            header.push_str(&format!("\nfilters: {}", active.join(" | ")));
        }
        let picked = inquire::Select::new(&header, options.clone())
            .with_page_size(20)
            .prompt()
            .map_err(ask_failed)?;
        let Some(choice) = options.iter().position(|o| *o == picked) else {
            continue;
        };

        if choice < first_nav {
            let hit = found.hits[choice].clone();
            pick_hit(cli, api, &hit).await?;
            added = added_slugs(cli);
            continue;
        }
        match navs[choice - first_nav] {
            Nav::Next => filters.page += 1,
            Nav::Prev => filters.page = filters.page.saturating_sub(1),
            Nav::EditFilters => {
                edit_filters(cli, &client, &mut filters).await?;
                filters.page = 1;
            }
            _ => return Ok(()),
        }
    }
}

async fn add(
    cli: &Cli,
    api: &str,
    project: &str,
    version: Option<&str>,
    mc: Option<&str>,
    kind: Option<PkgType>,
) -> Result<()> {
    let p = load_project(cli)?;
    let targets: Vec<Target> = match mc {
        Some(want) => vec![manifest::pick(&p.softwares.targets, Some(want), None)?.clone()],
        None => p.softwares.targets.clone(),
    };
    let total = targets.len();
    step(cli, 1, 3, &format!("finding {project} on modrinth"));
    add_project(cli, &p, project, version, targets).await?;
    step(cli, 3, 3, "done");
    let _ = (api, kind, total);
    Ok(())
}

async fn add_project(
    cli: &Cli,
    p: &Project,
    project: &str,
    version: Option<&str>,
    targets: Vec<Target>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    add_with_deps(cli, p, project, version, targets, &mut seen).await
}

fn add_with_deps<'a>(
    cli: &'a Cli,
    p: &'a Project,
    project: &'a str,
    version: Option<&'a str>,
    targets: Vec<Target>,
    seen: &'a mut BTreeSet<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(add_dep_tree(cli, p, project, version, targets, seen))
}

async fn add_dep_tree(
    cli: &Cli,
    p: &Project,
    project: &str,
    version: Option<&str>,
    targets: Vec<Target>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    seen.insert(project.to_ascii_lowercase());
    let client = Modrinth::new(cli.api.clone().unwrap_or_else(|| DEFAULT_API.to_string()))?;
    let single = p.softwares.targets.len() == 1;

    let mut versions: BTreeMap<String, String> = BTreeMap::new();
    let mut kind_out: Option<PkgType> = None;
    let mut slug = project.to_string();
    let mut file_urls: Vec<String> = Vec::new();
    let mut required: Vec<String> = Vec::new();

    for target in &targets {
        let loaders = loaders_for(target);
        let found = match version {
            Some(v) => client
                .resolve_pinned(project, Some(&target.minecraft), &loaders, v)
                .await,
            None => client.resolve(project, Some(&target.minecraft), &loaders).await,
        };
        match found {
            Ok(r) => {
                if r.project.project_type == "modpack" {
                    bail!(
                        "{} is a modpack, ehmodpack does not stuff modpacks inside modpacks",
                        r.project.slug
                    );
                }
                slug = r.project.slug.clone();
                kind_out = kind_out.or_else(|| Some(kind_of(&r.project.project_type)));
                file_urls.push(r.file.url.clone());
                for d in &r.version.dependencies {
                    if d.dependency_type == "required" {
                        if let Some(pid) = d.project_id.clone() {
                            required.push(pid);
                        }
                    }
                }
                versions.insert(
                    version_key(target, single),
                    r.version.version_number.clone(),
                );
            }
            Err(e) if targets.len() == 1 => {
                return Err(e).context("nothing was added");
            }
            Err(e) => warn(
                cli,
                &format!("skipped {} for {}: {e:#}", project, target.label()),
            ),
        }
    }

    if versions.is_empty() {
        bail!("nothing was added");
    }
    if p
        .manifest
        .packages
        .iter()
        .any(|x| x.key().eq_ignore_ascii_case(&slug))
    {
        bail!("{slug} is already in {MANIFEST_FILE}");
    }

    let mut learnt: Vec<String> = Vec::new();
    for url in &file_urls {
        let Ok(body) = client.get_bytes(url).await else {
            continue;
        };
        let Ok(Some(meta)) = modmeta::read_bytes(&body) else {
            continue;
        };
        if !meta.id.is_empty() && !learnt.iter().any(|l| l.eq_ignore_ascii_case(&meta.id)) {
            learnt.push(meta.id.clone());
        }
    }
    let ids = if learnt.is_empty() {
        None
    } else {
        Some(learnt)
    };
    if let Some(list) = &ids {
        say(cli, &format!("learned {slug} identifies as {}", list.join(", ")));
    }

    let kind = kind_out.unwrap_or(PkgType::Mod);
    let flat = if single {
        versions.values().next().cloned()
    } else {
        None
    };
    let spread = if single { BTreeMap::new() } else { versions };
    let mut pack = p.manifest.clone();
    pack.packages.push(Package {
        kind,
        project: Some(slug.clone()),
        id: None,
        version: flat,
        versions: spread,
        skip: Vec::new(),
        env: None,
        optional: None,
        active: matches!(kind, PkgType::Resourcepack).then_some(false),
        position: None,
        positions: BTreeMap::new(),
        ids,
        lock: None,
    });
    pack.write(&p.dir)?;
    let count = pack.packages.last().map_or(0, |x| {
        if x.version.is_some() {
            1
        } else {
            x.versions.len()
        }
    });
    say(
        cli,
        &format!(
            "added {slug} to {MANIFEST_FILE} for {count} of {} targets",
            targets.len()
        ),
    );
    let last = pack.packages.last().expect("just pushed");
    if let Some(v) = &last.version {
        println!("  {:<24} {v}", "default");
    }
    for (k, v) in &last.versions {
        println!("  {k:<24} {v}");
    }
    if kind == PkgType::Resourcepack {
        println!("  it is inactive, set \"active\": true once you want it bundled");
    }

    let mut pulled: Vec<String> = Vec::new();
    let existing: BTreeSet<String> = p
        .manifest
        .packages
        .iter()
        .map(|x| x.key().to_ascii_lowercase())
        .chain(
            p.manifest
                .packages
                .iter()
                .filter_map(|x| x.ids.clone())
                .flatten()
                .map(|i| i.to_ascii_lowercase()),
        )
        .collect();
    let wanted: Vec<String> = required
        .into_iter()
        .filter(|id| {
            let low = id.to_ascii_lowercase();
            !existing.contains(&low) && !seen.contains(&low) && !modmeta::is_platform(id)
        })
        .collect();
    for id in wanted {
        detail(cli, &format!("{slug} requires {id}"));
        let fresh = load_project(cli)?;
        match add_with_deps(cli, &fresh, &id, None, targets.clone(), seen).await {
            Ok(()) => pulled.push(id),
            Err(e) => warn(cli, &format!("could not add {id}, skipping: {e:#}")),
        }
    }
    if !pulled.is_empty() {
        say(
            cli,
            &format!(
                "{slug} installed {} because it was required by the mod",
                pulled.join(", ")
            ),
        );
    }
    Ok(())
}

fn remove(cli: &Cli, project: &str) -> Result<()> {
    let mut p = load_project(cli)?;
    let before = p.manifest.packages.len();
    p.manifest
        .packages
        .retain(|x| !x.key().eq_ignore_ascii_case(project));
    if p.manifest.packages.len() == before {
        bail!("{project} is not in {MANIFEST_FILE}");
    }
    p.manifest.write(&p.dir)?;
    say(cli, &format!("removed {project} from {MANIFEST_FILE}"));
    Ok(())
}

fn lock_project_has(lock: Option<&Lock>, package_key: &str, query: &str) -> bool {
    let Some(lock) = lock else {
        return false;
    };
    lock.targets.iter().flat_map(|t| t.packages.iter()).any(|lp| {
        lp.project.eq_ignore_ascii_case(package_key)
            && (lp.project.eq_ignore_ascii_case(query)
                || lp.project_id.eq_ignore_ascii_case(query)
                || lp.version_number == query)
    })
}

fn find_cmd(cli: &Cli, query: &str, remove_all: bool) -> Result<()> {
    let p = load_project(cli)?;
    let q = query.trim();
    if q.is_empty() {
        bail!("give a project id, slug, embedded id, or an exact version string");
    }
    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut hit_pkgs: Vec<usize> = Vec::new();
    for (i, pkg) in p.manifest.packages.iter().enumerate() {
        let mut why = None;
        if pkg.key().eq_ignore_ascii_case(q) {
            why = Some("project/slug".to_string());
        } else if pkg.ids.as_deref().is_some_and(|ids| ids.iter().any(|x| x.eq_ignore_ascii_case(q))) {
            why = Some("embedded id".to_string());
        } else if pkg.version.as_deref() == Some(q) {
            why = Some("default version".to_string());
        } else if pkg.versions.values().any(|v| v == q) {
            why = Some("version".to_string());
        } else if lock_project_has(lock.as_ref(), &pkg.key(), q) {
            why = Some("locked id or version".to_string());
        }
        if let Some(why) = why {
            let versions = if let Some(v) = &pkg.version {
                v.clone()
            } else if pkg.versions.is_empty() {
                "*".to_string()
            } else {
                pkg.versions.values().cloned().collect::<Vec<_>>().join(", ")
            };
            rows.push(vec![
                format!("{i}"),
                pkg.kind.modrinth_type().to_string(),
                pkg.key(),
                versions,
                why,
            ]);
            hit_pkgs.push(i);
        }
    }

    let mut hit_targets: Vec<usize> = Vec::new();
    for (i, t) in p.softwares.targets.iter().enumerate() {
        if t.label().eq_ignore_ascii_case(q)
            || t.minecraft == q
            || t.loader.kind.eq_ignore_ascii_case(q)
            || t.loader.version == q
        {
            hit_targets.push(i);
        }
    }

    println!();
    if rows.is_empty() && hit_targets.is_empty() {
        say(cli, &format!("nothing in this pack matches {q:?}"));
        return Ok(());
    }
    if !rows.is_empty() {
        say(
            cli,
            &format!(
                "{} package{} in {MANIFEST_FILE}",
                rows.len(),
                if rows.len() == 1 { "" } else { "s" }
            ),
        );
        println!(
            "{}",
            table(
                cli,
                &["#", "type", "project", "version(s)", "matched by"],
                &rows,
                &vec![false; rows.len()]
            )
        );
    }
    if !hit_targets.is_empty() {
        say(
            cli,
            &format!(
                "{} target{} in {SOFTWARES_FILE}",
                hit_targets.len(),
                if hit_targets.len() == 1 { "" } else { "s" }
            ),
        );
        let trows: Vec<Vec<String>> = hit_targets
            .iter()
            .map(|&i| {
                let t = &p.softwares.targets[i];
                vec![
                    t.label(),
                    t.minecraft.clone(),
                    t.loader.kind.clone(),
                    t.loader.version.clone(),
                ]
            })
            .collect();
        println!(
            "{}",
            table(
                cli,
                &["target", "minecraft", "loader", "loader version"],
                &trows,
                &vec![false; trows.len()]
            )
        );
    }
    if !remove_all {
        say(cli, "pass --remove to delete them after a confirm");
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("removing needs a terminal so you can confirm");
    }
    if hit_pkgs.is_empty() && hit_targets.is_empty() {
        return Ok(());
    }
    let yes = inquire::Confirm::new(&format!(
        "remove {} package{} and {} target{}?",
        hit_pkgs.len(),
        if hit_pkgs.len() == 1 { "" } else { "s" },
        hit_targets.len(),
        if hit_targets.len() == 1 { "" } else { "s" }
    ))
    .with_default(false)
    .prompt()
    .map_err(ask_failed)?;
    if !yes {
        say(cli, "left it all alone");
        return Ok(());
    }

    if !hit_pkgs.is_empty() {
        let mut p = load_project(cli)?;
        for idx in hit_pkgs.iter().rev() {
            let name = p.manifest.packages[*idx].key();
            p.manifest.packages.remove(*idx);
            say(cli, &format!("removed {name} from {MANIFEST_FILE}"));
        }
        p.manifest.write(&p.dir)?;
    }
    if !hit_targets.is_empty() {
        let mut p = load_project(cli)?;
        let mut gone = 0usize;
        for idx in hit_targets.iter().rev() {
            let label = p.softwares.targets[*idx].label();
            p.softwares.targets.remove(*idx);
            say(cli, &format!("dropped target {label} from {SOFTWARES_FILE}"));
            gone += 1;
        }
        p.softwares.write(&p.dir)?;
        if gone > 0 {
            say(cli, &format!("wrote {SOFTWARES_FILE}"));
        }
    }
    say(cli, "run `ehmodpack lock` after to refresh the lockfile");
    Ok(())
}

fn list(cli: &Cli, mc: Option<&str>, loader: Option<LoaderKind>) -> Result<()> {
    let p = load_project(cli)?;
    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };
    if p.manifest.packages.is_empty() {
        say(cli, "no packages yet, try `ehmodpack search`");
        return Ok(());
    }
    let all_targets: Vec<LockedTarget> = lock
        .as_ref()
        .map(|l| l.targets.clone())
        .unwrap_or_default();
    let targets: Vec<LockedTarget> = if mc.is_some() || loader.is_some() {
        vec![manifest::pick(&all_targets, mc, loader)?.clone()]
    } else {
        all_targets
    };

    let mut headers: Vec<String> = vec!["type".into(), "package".into(), "wanted".into()];
    headers.extend(targets.iter().map(|t| t.label()));
    let head: Vec<&str> = headers.iter().map(String::as_str).collect();

    let mut rows: Vec<Vec<String>> = Vec::new();
    for pkg in &p.manifest.packages {
        let mut row = vec![
            pkg.kind.modrinth_type().to_string(),
            pkg.key(),
            pkg.version.clone().unwrap_or_else(|| "*".to_string()),
        ];
        for t in &targets {
            let resolved = lock
                .as_ref()
                .and_then(|l| {
                    l.targets.iter().find(|lt| {
                        lt.minecraft == t.minecraft && lt.loader.kind == t.loader.kind
                    })
                })
                .and_then(|lt| {
                    lt.packages
                        .iter()
                        .find(|lp| lp.project.eq_ignore_ascii_case(&pkg.key()))
                })
                .map(|lp| lp.version_number.clone());
            row.push(match resolved {
                Some(v) => v,
                None => "-".to_string(),
            });
        }
        rows.push(row);
    }
    let no_dim: Vec<bool> = rows.iter().map(|_| false).collect();
    println!("{}", table(cli, &head, &rows, &no_dim));
    if lock.is_none() {
        println!("\nnothing is locked yet, run `ehmodpack lock`");
    }
    Ok(())
}

async fn lock_cmd(
    cli: &Cli,
    api: &str,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    check: bool,
) -> Result<()> {
    if check {
        return lock_check(cli, api, mc, loader).await;
    }
    let mut p = load_project(cli)?;
    do_lock(cli, api, &mut p, mc, loader).await
}

async fn do_lock(
    cli: &Cli,
    api: &str,
    p: &mut Project,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
) -> Result<()> {
    let wanted: Vec<String> = if mc.is_some() || loader.is_some() {
        vec![manifest::pick(&p.softwares.targets, mc, loader)?.label()]
    } else {
        p.softwares
            .targets
            .iter()
            .map(Targetish::label)
            .collect()
    };

    let keep: BTreeMap<String, LockedTarget> = if Lock::exists(&p.dir) {
        Lock::load(&p.dir)?
            .targets
            .into_iter()
            .map(|t| (Targetish::label(&t), t))
            .collect()
    } else {
        BTreeMap::new()
    };

    let client = Modrinth::new(api)?;
    let single = p.softwares.targets.len() == 1;
    let mut fresh: BTreeMap<String, LockedTarget> = BTreeMap::new();
    let mut pulled_all: Vec<String> = Vec::new();
    for target in &p.softwares.targets {
        if !wanted.contains(&Targetish::label(target)) {
            continue;
        }
        let mut packages = Vec::new();
        let mut steps = crate::progress::Steps::new(p.manifest.packages.len());
        let mut missing: Vec<String> = Vec::new();
        let mut deps: Vec<crate::modrinth::VersionDependency> = Vec::new();
        for (i, pkg) in p.manifest.packages.iter().enumerate() {
            if pkg.is_skipped(&target.minecraft, &target.loader.kind) {
                detail(
                    cli,
                    &format!("{} is skipped (which will not include in the modpack)", pkg.key()),
                );
                continue;
            }
            steps.at(i + 1, &pkg.key());
            match resolve_one(&client, target, pkg).await {
                Ok(found) => {
                    packages.push(to_locked(pkg, &found, target));
                    deps.extend(found.version.dependencies.iter().cloned());
                }
                Err(_) => {
                    warn(cli, &unsupported_note(&client, &pkg.key(), target).await);
                    missing.push(pkg.key());
                }
            }
        }
        let pulled = pull_deps(cli, &client, &mut p.manifest, target, single, &mut packages, deps).await?;
        steps.finish();
        if packages.is_empty() {
            bail!("nothing at all resolves for {}", target.label());
        }
        for slug in &pulled {
            if !pulled_all.iter().any(|x| x == slug) {
                pulled_all.push(slug.clone());
            }
        }
        say(
            cli,
            &format!(
                "locked {} packages for {}{}",
                packages.len(),
                target.label(),
                if missing.is_empty() {
                    String::new()
                } else {
                    format!(", {} could not be had: {}", missing.len(), missing.join(", "))
                }
            ),
        );
        fresh.insert(
            Targetish::label(target),
            LockedTarget {
                minecraft: target.minecraft.clone(),
                java: target.java.clone(),
                loader: target.loader.clone(),
                packages,
                external_packs: p
                    .manifest
                    .external_packs
                    .iter()
                    .map(|e| crate::lock::LockedExternal {
                        name: e.name.clone(),
                        active: e.is_active(),
                        builtin: e.is_builtin(),
                        position: e.position_for(&target.minecraft, &target.loader.kind),
                    })
                    .collect(),
            },
        );
    }
    if !pulled_all.is_empty() {
        say(
            cli,
            &format!("pulled {} required dep{}: {}", pulled_all.len(), if pulled_all.len() == 1 { "" } else { "s" }, pulled_all.join(", ")),
        );
        p.manifest.write(&p.dir)?;
        say(cli, &format!("wrote {MANIFEST_FILE}"));
    }

    let mut targets: Vec<LockedTarget> = Vec::new();
    for target in &p.softwares.targets {
        let label = Targetish::label(target);
        if let Some(f) = fresh.remove(&label) {
            targets.push(f);
        } else if let Some(k) = keep.get(&label) {
            targets.push(k.clone());
        }
    }
    let name = if p.manifest.name.is_empty() {
        p.dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "pack".to_string())
    } else {
        p.manifest.name.clone()
    };
    Lock::stamped(
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        p.manifest.fingerprint(),
        name,
        p.manifest.version.clone(),
        p.manifest.summary.clone(),
        targets,
    )
    .write(&p.dir)?;
    say(cli, &format!("wrote {LOCK_FILE}"));
    check_schema(cli, p);
    Ok(())
}

fn add_dep_pin(
    manifest: &mut Manifest,
    kind: PkgType,
    slug: &str,
    key: &str,
    version: &str,
    single: bool,
) {
    if let Some(pkg) = manifest
        .packages
        .iter_mut()
        .find(|x| x.key().eq_ignore_ascii_case(slug))
    {
        if single {
            pkg.versions.clear();
            pkg.version = Some(version.to_string());
        } else {
            pkg.versions.insert(key.to_string(), version.to_string());
        }
        return;
    }
    let mut versions = BTreeMap::new();
    if !single {
        versions.insert(key.to_string(), version.to_string());
    }
    manifest.packages.push(Package {
        kind,
        project: Some(slug.to_string()),
        id: None,
        version: if single { Some(version.to_string()) } else { None },
        versions,
        skip: Vec::new(),
        env: None,
        optional: None,
        active: None,
        position: None,
        positions: BTreeMap::new(),
        ids: None,
        lock: None,
    });
}

async fn pull_deps(
    cli: &Cli,
    client: &Modrinth,
    manifest: &mut Manifest,
    target: &Target,
    single: bool,
    packages: &mut Vec<LockedPackage>,
    deps: Vec<crate::modrinth::VersionDependency>,
) -> Result<Vec<String>> {
    let mut visited: BTreeSet<String> = BTreeSet::new();
    for p in &manifest.packages {
        visited.insert(p.key().to_ascii_lowercase());
        if let Some(ids) = &p.ids {
            for id in ids {
                visited.insert(id.to_ascii_lowercase());
            }
        }
    }
    for lp in packages.iter() {
        visited.insert(lp.project.to_ascii_lowercase());
        visited.insert(lp.project_id.to_ascii_lowercase());
    }
    let mut queue: std::collections::VecDeque<crate::modrinth::VersionDependency> = deps.into();
    let mut pulled: Vec<String> = Vec::new();
    let loaders = vec![target.loader.kind.clone()];
    while let Some(dep) = queue.pop_front() {
        let Some(pid) = dep.project_id.clone() else {
            continue;
        };
        let low = pid.to_ascii_lowercase();
        if dep.dependency_type != "required" {
            detail(
                cli,
                &format!("dep {pid} is {}, not pulling it", dep.dependency_type),
            );
            continue;
        }
        if visited.contains(&low) || modmeta::is_platform(&low) {
            detail(
                cli,
                &format!("dep {pid} already seen or a platform lib, skipping"),
            );
            continue;
        }
        if dep.file_name.is_some() && dep.version_id.is_none() {
            continue;
        }
        visited.insert(low);
        let resolved = if let Some(vid) = &dep.version_id {
            match (client.project(&pid).await, client.version(vid).await) {
                (Ok(project), Ok(version)) => {
                    match crate::modrinth::resolve_from_project(project, version) {
                        Ok(r) => r,
                        Err(e) => {
                            warn(cli, &format!("dep {pid} {vid} could not be used, skipping: {e:#}"));
                            continue;
                        }
                    }
                }
                _ => {
                    warn(cli, &format!("dep {pid} ({vid}) could not be fetched, skipping"));
                    continue;
                }
            }
        } else {
            match client.resolve(&pid, Some(&target.minecraft), &loaders).await {
                Ok(r) => r,
                Err(e) => {
                    warn(cli, &format!("dep {pid} has nothing here, skipping: {e:#}"));
                    continue;
                }
            }
        };
        let slug = resolved.project.slug.clone();
        if resolved.project.project_type != "mod" {
            warn(
                cli,
                &format!(
                    "dep {pid} is a {}, cannot be a mod dependency, skipping",
                    resolved.project.project_type
                ),
            );
            continue;
        }
        if let Some(declared) = manifest
            .packages
            .iter()
            .find(|p| p.key().eq_ignore_ascii_case(&slug))
        {
            if declared.is_skipped(&target.minecraft, &target.loader.kind) {
                warn(
                    cli,
                    &format!(
                        "{slug} was supposed to be a required dependency but it was skipped because of the configuration file"
                    ),
                );
                continue;
            }
        }
        if packages.iter().any(|lp| lp.project.eq_ignore_ascii_case(&slug))
            || visited.contains(&slug.to_ascii_lowercase())
        {
            detail(cli, &format!("already have {slug}, skipping"));
            continue;
        }
        visited.insert(slug.to_ascii_lowercase());
        queue.extend(resolved.version.dependencies.iter().cloned());
        let version_number = resolved.version.version_number.clone();
        let key = format!("{}+{}", target.minecraft, target.loader.kind);
        let kind = kind_of(&resolved.project.project_type);
        let mut versions = BTreeMap::new();
        if !single {
            versions.insert(key.clone(), version_number.clone());
        }
        let dep_pkg = Package {
            kind,
            project: Some(slug.clone()),
            id: None,
            version: if single { Some(version_number.clone()) } else { None },
            versions,
            skip: Vec::new(),
            env: None,
            optional: None,
            active: None,
            position: None,
            positions: BTreeMap::new(),
            ids: None,
            lock: None,
        };
        packages.push(to_locked(&dep_pkg, &resolved, target));
        add_dep_pin(manifest, kind, &slug, &key, &version_number, single);
        pulled.push(slug);
    }
    Ok(pulled)
}

async fn lock_check(
    cli: &Cli,
    api: &str,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
) -> Result<()> {
    let p = load_project(cli)?;
    let lock = Lock::load(&p.dir)?;
    if lock.manifest_hash != p.manifest.fingerprint() {
        bail!("{MANIFEST_FILE} changed since {LOCK_FILE} was written, run `ehmodpack lock`");
    }
    let wanted: Vec<String> = p.softwares.targets.iter().map(Targetish::label).collect();
    let have = lock.labels();
    if wanted != have {
        bail!(
            "targets changed, {SOFTWARES_FILE} has {} but {LOCK_FILE} has {}, run `ehmodpack lock`",
            wanted.join(", "),
            have.join(", ")
        );
    }
    let client = Modrinth::new(api)?;
    let targets: Vec<&LockedTarget> = if mc.is_some() || loader.is_some() {
        vec![lock.target(mc, loader)?]
    } else {
        lock.targets.iter().collect()
    };
    let mut stale: Vec<String> = Vec::new();
    for t in targets {
        for pkg in &t.packages {
            match client
                .resolve_pinned(
                    &pkg.project_id,
                    Some(&t.minecraft),
                    &[t.loader.kind.clone()],
                    &pkg.version_number,
                )
                .await
            {
                Ok(r) if r.version.id == pkg.version_id => {}
                Ok(r) => stale.push(format!(
                    "{} {} was replaced by {}",
                    pkg.project, pkg.version_number, r.version.version_number
                )),
                Err(e) => stale.push(format!(
                    "{} {} is gone: {e:#}",
                    pkg.project, pkg.version_number
                )),
            }
        }
    }
    if stale.is_empty() {
        say(
            cli,
            &format!("lockfile is fresh, {} targets", lock.targets.len()),
        );
        return Ok(());
    }
    for line in &stale {
        warn(cli, line);
    }
    bail!("{LOCK_FILE} is stale, run `ehmodpack lock`");
}

async fn build_cmd(
    cli: &Cli,
    api: &str,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    locked: bool,
    out: &Path,
    all: bool,
) -> Result<()> {
    let p = load_project(cli)?;
    let lock = if locked {
        let l = Lock::load(&p.dir)?;
        if l.manifest_hash != p.manifest.fingerprint() {
            bail!("{MANIFEST_FILE} changed since {LOCK_FILE} was written, run `ehmodpack lock` first");
        }
        l
    } else {
        if !Lock::exists(&p.dir) {
            lock_cmd(cli, api, None, None, false).await?;
        }
        Lock::load(&p.dir)?
    };

    let chosen: Vec<&LockedTarget> = if all {
        lock.targets.iter().collect()
    } else {
        vec![lock.target(mc, loader)?]
    };

    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    if p.manifest.client {
        sources.push((
            p.manifest.client_overrides.clone(),
            p.dir.join(&p.manifest.client_overrides),
        ));
    }
    if p.manifest.server {
        sources.push((
            p.manifest.server_overrides.clone(),
            p.dir.join(&p.manifest.server_overrides),
        ));
    }
    sources.push((
        p.manifest.overrides.clone(),
        p.dir.join(&p.manifest.overrides),
    ));

    let out_dir = if out.is_absolute() {
        out.to_path_buf()
    } else {
        p.dir.join(out)
    };

    println!();
    say(cli, "checking the pack before building");
    print_report(cli, &validate::check(&lock, &p.manifest, &sources))?;
    print_report(cli, &validate::check_sources(&sources))?;
    check_schema(cli, &p);

    for target in &chosen {
        let active = target.active_packs();
        if !active.is_empty() {
            println!();
            say(
                cli,
                &format!("active resource packs for {}:", target.label()),
            );
            for pkg in &active {
                println!("  {}", pkg.path);
            }
        }
    }
    println!();

    let client = Modrinth::new(api)?;
    for target in chosen {
        say(
            cli,
            &format!(
                "building for {}, {}",
                target.minecraft, target.loader.kind
            ),
        );
        let on_disk_packs = build::pack_paths_on_disk(&sources);
        for pkg in target.active_packs() {
            let fname = pkg.path.rsplit('/').next().unwrap_or(&pkg.path);
            if !pkg.downloads.is_empty() {
                detail(
                    cli,
                    &format!(
                        "{}, {} comes from modrinth as-is, skipping the local copy",
                        pkg.project, fname
                    ),
                );
                continue;
            }
            match on_disk_packs.iter().find(|(n, _)| n == fname) {
                Some((_, path)) => {
                    let got = build::sha1_of(path);
                    if got.as_deref().is_some() && got.as_deref() != Some(&pkg.hashes.sha1) {
                        warn(
                            cli,
                            &format!(
                                "{} exists but its sha does not match the lock, this build may bundle a wrong version",
                                pkg.project
                            ),
                        );
                    }
                }
                None => warn(
                    cli,
                    &format!(
                        "{} is active but {} is not in the overrides, re-run sync-pack-active so this version can activate it",
                        pkg.project, fname
                    ),
                ),
            }
        }
        let staged = staging::fresh_dir(&lock.name, target)?;
        let verified = match staging::download(&client, target, &staged, |line| {
            detail(cli, line);
        })
        .await
        {
            Ok(v) => v,
            Err(e) => {
                let _ = staging::wipe(&staged);
                return Err(e).with_context(|| {
                    format!("{} did not verify, not printing a build for it", target.label())
                });
            }
        };
        detail(
            cli,
            &format!(
                "{} files out of {} shas",
                verified.downloaded, verified.shas
            ),
        );
        let path = build::build(&lock, target, &out_dir, &sources)?;
        let _ = staging::wipe(&staged);
        say(cli, &format!("built {}", path.display()));
        println!(
            "  mc {}  loader {} {}  java {}  packages {}",
            target.minecraft,
            target.loader.kind,
            target.loader.version,
            target.java.as_deref().unwrap_or("-"),
            target.packages.len()
        );
        for pkg in &target.packages {
            if let Some(position) = pkg.position {
                println!("  active {} ({position:?})", pkg.project);
            }
        }
    }
    check_schema(cli, &p);
    Ok(())
}

fn closing_lines(cli: &Cli, dir: &Path, targets: &[Target], committed: Option<bool>) {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string());
    say(cli, &format!("Done you can visit your project at cd ./{name}"));
    say(cli, "you can view README.md in there for the next steps");
    println!();
    println!("You can view projects by either:");
    println!();
    println!("- Searching with ehmodpack search");
    println!("- Or browsing for it with ehmodpack browse");
    println!();
    for t in targets {
        println!(
            "Created a modpack for {} {} with {} {}",
            t.minecraft,
            t.loader.kind,
            t.loader.kind,
            t.loader.version
        );
    }
    match committed {
        Some(true) => println!("  git repo initialised with one commit"),
        Some(false) => warn(
            cli,
            "git repo initialised but the first commit failed, check your git identity",
        ),
        None => {}
    }
}

async fn new_cmd(
    cli: &Cli,
    api: &str,
    partial: NewPlan,
    git_flag: bool,
    force: bool,
) -> Result<()> {
    let plan = ask_missing(cli, api, partial, git_flag).await?;
    let (dir, committed, targets) = scaffold::new(plan, &cli.dir, force, api).await?;
    println!();
    closing_lines(
        cli,
        &dir,
        &targets,
        if git_flag || committed { Some(committed) } else { None },
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn new_from_mrpack(
    cli: &Cli,
    api: &str,
    mrpack: &Path,
    name: Option<&str>,
    folder: Option<&str>,
    include_configs: bool,
    workflow: Option<WorkflowKind>,
    git: bool,
    force: bool,
) -> Result<()> {
    let workflow = pick_workflow(cli, workflow)?;
    notice(cli, importer::LICENSE_WARNING);
    println!();

    step(cli, 1, 6, "reading the archive");
    let index = importer::read_index(mrpack)?;
    let pack_name = name
        .map(str::to_string)
        .unwrap_or_else(|| importer::name_of(&index));
    let dir = cli
        .dir
        .join(folder.map_or_else(
            || scaffold::slugify(&pack_name),
            |f| scaffold::slugify(f),
        ));
    importer::ensure_empty_dir(&dir, force)?;

    let minecraft = importer::minecraft_of(&index)?;
    let loader_pairs = importer::loaders_of(&index);
    if loader_pairs.is_empty() {
        bail!("this modpack does not name a loader, so there is nothing to build against");
    }

    let files = importer::files_of(&index)?;
    step(
        cli,
        2,
        6,
        &format!("looking {} files up on modrinth", files.len()),
    );
    let client = Modrinth::new(api)?;
    let version_ids: Vec<String> = files.iter().map(|f| f.version_id.clone()).collect();
    let mut versions = client.versions_by_ids(&version_ids).await?;
    let by_number: Vec<&crate::importer::IndexFile> = files
        .iter()
        .filter(|f| !crate::modrinth::is_modrinth_id(&f.version_id))
        .collect();
    for f in by_number {
        let Some(project) = client.project(&f.project_id).await.ok() else {
            continue;
        };
        let listed = client.versions(&project, Some(&minecraft), &[]).await?;
        if let Some(v) = listed
            .into_iter()
            .find(|v| v.version_number == f.version_id)
        {
            versions.push(v);
        }
    }
    let project_ids: Vec<String> = versions.iter().map(|v| v.project_id.clone()).collect();
    let projects = client.projects_by_ids(&project_ids).await?;
    detail(
        cli,
        &format!(
            "GET /v2/versions?ids=  {} ids, {} came back",
            version_ids.len(),
            versions.len()
        ),
    );
    detail(
        cli,
        &format!(
            "GET /v2/projects?ids=  {} ids, {} came back",
            project_ids.len(),
            projects.len()
        ),
    );
    let mut by_version: BTreeMap<&str, &crate::modrinth::MVersion> =
        versions.iter().map(|v| (v.id.as_str(), v)).collect();
    for v in &versions {
        if !is_modrinth_id(&v.version_number) && by_version.get(v.id.as_str()).is_none() {
            by_version.insert(v.version_number.as_str(), v);
        }
    }
    let by_project: BTreeMap<&str, &crate::modrinth::Project> =
        projects.iter().map(|p| (p.id.as_str(), p)).collect();

    step(cli, 3, 6, "writing packages.json");
    let mut packages = Vec::new();
    for f in &files {
        let Some(v) = by_version.get(f.version_id.as_str()) else {
            detail(
                cli,
                &format!("{}  MISSING on modrinth, skipped", f.path),
            );
            warn(
                cli,
                &format!("{} is gone from modrinth, skipping it", f.path),
            );
            continue;
        };
        let Some(pj) = by_project.get(v.project_id.as_str()) else {
            detail(cli, &format!("{}  no project, skipped", f.path));
            warn(
                cli,
                &format!("{} has no project on modrinth, skipping it", f.path),
            );
            continue;
        };
        detail(
            cli,
            &format!("{}  ->  {} {}", f.path, pj.slug, v.version_number),
        );
        packages.push(Package {
            kind: f.kind(),
            project: Some(pj.slug.clone()),
            id: None,
            version: Some(v.version_number.clone()),
            versions: BTreeMap::new(),
            skip: Vec::new(),
            env: Some(f.env),
            optional: Some(f.env.client == Support::Optional),
            active: None,
            position: None,
            positions: BTreeMap::new(),
            ids: None,
            lock: None,
        });
    }

    let java = importer::java_of(&index)
        .or_else(|| Some(loaders::java_for(&minecraft).to_string()));
    let targets: Vec<Target> = loader_pairs
        .iter()
        .map(|(kind, version)| Target {
            minecraft: minecraft.clone(),
            java: java.clone(),
            loader: LoaderSpec {
                kind: kind.clone(),
                version: version.clone(),
            },
        })
        .collect();

    let pack = Manifest {
        schema: None,
        schema_version: 0,
        name: pack_name.clone(),
        modrinth_project_id: None,
        summary: importer::summary_of(&index),
        version: importer::version_of(&index),
        top: Some(minecraft.clone()),
        client: true,
        server: true,
        overrides: manifest::DEFAULT_OVERRIDES.to_string(),
        client_overrides: manifest::DEFAULT_CLIENT_OVERRIDES.to_string(),
        server_overrides: manifest::DEFAULT_SERVER_OVERRIDES.to_string(),
        packages,
        external_packs: Vec::new(),
    };
    let mut softwares = Softwares {
        schema: None,
        schema_version: 0,
        targets: targets.clone(),
    };
    let mut pack = pack;
    pack.stamp();
    softwares.stamp();
    scaffold::write_project_files(&dir, &pack, &softwares, &pack_name, workflow)?;

    step(cli, 4, 6, "pulling the overrides out");
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    let written = importer::extract_overrides(mrpack, &dir, include_configs, &mut sources)?;
    if include_configs {
        say(cli, &format!("  {written} override files, configs included"));
    } else {
        say(
            cli,
            &format!("  {written} override files, pass --include-configs for the configs too"),
        );
    }

    step(cli, 5, 6, "locking every mod to the version the pack wanted");
    let mut project = Project {
        dir: dir.clone(),
        manifest: pack,
        softwares,
    };
    do_lock(cli, api, &mut project, None, None).await?;

    step(cli, 6, 6, "wrapping up");
    let committed = if git {
        Some(scaffold::init_repo(&dir)?)
    } else {
        None
    };
    println!();
    closing_lines(cli, &dir, &project.softwares.targets, committed);
    Ok(())
}

async fn add_ver(
    cli: &Cli,
    api: &str,
    version: &str,
    loaders: &[LoaderKind],
) -> Result<()> {
    let mut p = load_project(cli)?;
    let kinds: Vec<LoaderKind> = if loaders.is_empty() {
        let mut seen: Vec<LoaderKind> = Vec::new();
        for t in &p.softwares.targets {
            if let Some(k) = LoaderKind::parse(&t.loader.kind) {
                if !seen.contains(&k) {
                    seen.push(k);
                }
            }
        }
        seen
    } else {
        loaders.to_vec()
    };
    if kinds.is_empty() {
        bail!("this pack has no loader ehmodpack knows, pass --loader");
    }

    let mut fresh = Vec::new();
    for k in &kinds {
        if p
            .softwares
            .targets
            .iter()
            .any(|t| t.minecraft == version && t.loader.kind == k.as_str())
        {
            bail!("{version}/{} is already a target", k.as_str());
        }
        let lv = loaders::resolve(*k, version)
            .await
            .with_context(|| format!("could not resolve the {k} loader for {version}"))?;
        fresh.push(Target::new(version, *k, &lv));
    }
    for t in &fresh {
        say(cli, &format!("adding {}", t.label()));
    }

    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };
    let client = Modrinth::new(api)?;
    p.softwares.targets.extend(fresh);

    step(
        cli,
        1,
        2,
        "working out a version for every mod on every target",
    );
    let total = p.manifest.packages.len() * p.softwares.targets.len();
    let mut steps = crate::progress::Steps::new(total);
    let mut maps: Vec<BTreeMap<String, String>> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for pkg in &p.manifest.packages {
        let mut map = BTreeMap::new();
        for t in &p.softwares.targets {
            steps.at(map.len() + maps.iter().map(|m| m.len()).sum::<usize>() + 1, &pkg.key());
            let key = format!("{}+{}", t.minecraft, t.loader.kind);
            match pick_version(pkg, &t.minecraft, lock.as_ref(), t, &client).await {
                Some(v) => {
                    map.insert(key, v);
                }
                None => {
                    warn(cli, &unsupported_note(&client, &pkg.key(), t).await);
                    missing.push(format!("{} for {}", pkg.key(), t.label()));
                }
            }
        }
        maps.push(map);
    }
    steps.finish();
    for (pkg, map) in p.manifest.packages.iter_mut().zip(maps) {
        pkg.versions = map;
        pkg.version = None;
    }
    p.manifest.write(&p.dir)?;
    p.softwares.write(&p.dir)?;

    step(cli, 2, 2, "locking");
    do_lock(cli, api, &mut p, None, None).await?;
    say(
        cli,
        &format!(
            "now building for {}",
            p.softwares
                .targets
                .iter()
                .map(Targetish::label)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    Ok(())
}

async fn confirm_identity(
    cli: &Cli,
    client: &Modrinth,
    slug: &str,
    url: &str,
    known: &[String],
) -> Result<Option<String>> {
    let body = match client.get_bytes(url).await {
        Ok(b) => b,
        Err(e) => {
            warn(
                cli,
                &format!("could not read the jar of {slug} to confirm its identity: {e:#}"),
            );
            return Ok(None);
        }
    };
    let Some(id) = (match modmeta::read_bytes(&body) {
        Ok(Some(m)) if !m.id.is_empty() => Some(m.id),
        _ => None,
    }) else {
        return Ok(None);
    };
    if known.is_empty() {
        return Ok(Some(id));
    }
    if known.iter().any(|k| k.eq_ignore_ascii_case(&id)) {
        return Ok(Some(id));
    }
    bail!(
        "{slug} now identifies as {id}, but this project is known to be {}",
        known.join(", ")
    )
}

async fn update_cmd(
    cli: &Cli,
    api: &str,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    dry_run: bool,
    update_changelogs: bool,
) -> Result<()> {
    let mut p = load_project(cli)?;
    let targets: Vec<Target> = if mc.is_some() || loader.is_some() {
        vec![manifest::pick(&p.softwares.targets, mc, loader)?.clone()]
    } else {
        p.softwares.targets.clone()
    };
    let single = p.softwares.targets.len() == 1;
    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };
    let client = Modrinth::new(api)?;

    step(
        cli,
        1,
        2,
        "working out the newest version of every mod on every target",
    );
    let total: usize = p.manifest.packages.len() * targets.len();
    let mut steps = crate::progress::Steps::new(total);
    let mut n = 0usize;
    let mut changes: Vec<(String, String, String, String, String)> = Vec::new();
    let mut learn: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut bad: Vec<String> = Vec::new();
    let mut skipped: Vec<(String, String, String)> = Vec::new();

    for target in &targets {
        let key = format!("{}+{}", target.minecraft, target.loader.kind);
        let loaders = vec![target.loader.kind.clone()];
        for idx in 0..p.manifest.packages.len() {
            n += 1;
            let slug = p.manifest.packages[idx].key();
            steps.at(n, &slug);
            if slug.is_empty() {
                continue;
            }
            if p.manifest.packages[idx].is_locked() {
                detail(cli, &format!("{slug} is locked, update ignores it"));
                continue;
            }
            if p.manifest.packages[idx].is_skipped(&target.minecraft, &target.loader.kind) {
                skipped.push((
                    slug.clone(),
                    target.label(),
                    "thing was skipped".to_string(),
                ));
                continue;
            }
            let kind = p.manifest.packages[idx].kind;
            let known = p.manifest.packages[idx].ids.clone().unwrap_or_default();
            let pinned = p.manifest.packages[idx]
                .version_for(&target.minecraft, &target.loader.kind);
            let locked = lock.as_ref().and_then(|l| {
                l.targets
                    .iter()
                    .find(|t| {
                        t.minecraft == target.minecraft && t.loader.kind == target.loader.kind
                    })
                    .and_then(|t| {
                        t.packages
                            .iter()
                            .find(|lp| lp.project.eq_ignore_ascii_case(&slug))
                            .map(|lp| lp.version_number.clone())
                    })
            });
            let was = locked.or(pinned);

            let Ok(project) = client.project(&slug).await else {
                skipped.push((slug.clone(), target.label(), "not on modrinth".to_string()));
                continue;
            };
            let want = if kind == PkgType::Mod {
                loaders.clone()
            } else {
                Vec::new()
            };
            let Ok(list) = client
                .versions(&project, Some(&target.minecraft), &want)
                .await
            else {
                skipped.push((
                    slug.clone(),
                    target.label(),
                    "could not list versions".to_string(),
                ));
                continue;
            };
            if list.is_empty() {
                warn(
                    cli,
                    &format!(
                        "{slug} has nothing for {}, that target stays put",
                        target.label()
                    ),
                );
                skipped.push((
                    slug.clone(),
                    target.label(),
                    format!("no {} build for {}", target.loader.kind, target.minecraft),
                ));
                continue;
            }
            let best = crate::modrinth::best_of(list.clone());
            if best.is_empty() {
                continue;
            }
            if was.as_deref() == Some(best.as_str()) {
                continue;
            }
            let Some(version) = list
                .into_iter()
                .find(|v| v.version_number == best)
            else {
                continue;
            };
            let title = project.title.clone();
            let found = match crate::modrinth::resolve_from_project(project, version) {
                Ok(f) => f,
                Err(e) => {
                    bad.push(format!("{slug}: {e:#}"));
                    continue;
                }
            };
            let url = found.file.url.clone();
            let observed = match confirm_identity(cli, &client, &slug, &url, &known).await {
                Ok(o) => o,
                Err(e) => {
                    bad.push(format!("{slug}: {e:#}"));
                    continue;
                }
            };
            if let Some(id) = observed {
                if known.is_empty() {
                    learn.entry(slug.clone()).or_default().push(id);
                }
            }
            changes.push((
                slug,
                title,
                key.clone(),
                was.unwrap_or_else(|| "*".to_string()),
                best,
            ));
        }
    }
    steps.finish();
    let _ = manifest::write_file(
        &p.dir.join(".ehmodpack-updates"),
        &changes.len().to_string(),
    );

    println!();
    if changes.is_empty() && bad.is_empty() {
        say(cli, "everything is already the newest it can be");
    } else {
        let rows: Vec<Vec<String>> = changes
            .iter()
            .map(|(s, _t, k, w, n)| {
                vec![s.clone(), k.clone(), w.clone(), n.clone()]
            })
            .collect();
        if !rows.is_empty() {
            say(
                cli,
                &format!("{} update{} ready", rows.len(), if rows.len() == 1 { "" } else { "s" }),
            );
            println!(
                "{}",
                table(
                    cli,
                    &["package", "target", "was", "now"],
                    &rows,
                    &vec![false; rows.len()]
                )
            );
        }
    }
    if !skipped.is_empty() {
        warn(
            cli,
            &format!("{} target(s) could not be updated", skipped.len()),
        );
        for (s, t, why) in &skipped {
            detail(cli, &format!("{s}, {}: {why}", t));
        }
    }
    if dry_run {
        say(cli, "nothing was written");
        return Ok(());
    }
    if !bad.is_empty() {
        for line in &bad {
            eprintln!("{}", line.red().bold());
        }
        bail!("update was aborted before anything was written");
    }
    if changes.is_empty() {
        return Ok(());
    }
    step(
        cli,
        2,
        2,
        "writing the new pins and locking",
    );
    for (slug, _title, key, _was, now) in &changes {
        set_pin(&mut p.manifest, slug, key, now, single);
    }
    for (slug, ids) in &learn {
        if let Some(pkg) = p
            .manifest
            .packages
            .iter_mut()
            .find(|x| x.key().eq_ignore_ascii_case(slug))
        {
            for id in ids {
                pkg.remember(id);
            }
        }
    }
    p.manifest.write(&p.dir)?;
    say(cli, &format!("wrote {MANIFEST_FILE}"));
    let mut p = load_project(cli)?;
    if mc.is_some() || loader.is_some() {
        for target in &targets {
            do_lock(
                cli,
                api,
                &mut p,
                Some(&target.minecraft),
                LoaderKind::parse(&target.loader.kind),
            )
            .await?;
        }
    } else {
        do_lock(cli, api, &mut p, None, None).await?;
    }
    if update_changelogs && !changes.is_empty() {
        let mut lines: Vec<String> = Vec::new();
        lines.push("## Mod Updates".to_string());
        lines.push(String::new());
        for (slug, title, _key, was, now) in &changes {
            let old = if was == "*" { "*".to_string() } else { was.clone() };
            lines.push(format!(
                "- [{title}](https://modrinth.com/mod/{slug}) has been updated from {old} to {now}"
            ));
        }
        if let Some(lock) = &lock {
            for t in &p.softwares.targets {
                let old = lock
                    .targets
                    .iter()
                    .find(|lt| {
                        lt.minecraft == t.minecraft && lt.loader.kind == t.loader.kind
                    })
                    .map(|lt| lt.loader.version.clone());
                if let Some(o) = old {
                    if o != t.loader.version {
                        lines.push(format!(
                            "- {} has been updated from {} to {}",
                            t.loader.kind, o, t.loader.version
                        ));
                    }
                }
            }
        }
        append_changelog_toml(cli, &p.dir, &lines)?;
    }
    say(cli, "lockfile is fresh");
    Ok(())
}

fn append_changelog_toml(cli: &Cli, dir: &Path, lines: &[String]) -> Result<()> {
    let now = time::OffsetDateTime::now_utc();
    let date = format!(
        "{}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    );
    let mut out = std::fs::read_to_string(dir.join(crate::manifest::CHANGELOG_FILE))
        .unwrap_or_default();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "[[{date}]]\nchangelogtext = \"\"\"\n{}\n\"\"\"\n",
        lines.join("\n")
    ));
    crate::manifest::write_file(&dir.join(crate::manifest::CHANGELOG_FILE), &out)?;
    say(
        cli,
        &format!("wrote {}", crate::manifest::CHANGELOG_FILE),
    );
    Ok(())
}

async fn pick_version(
    pkg: &Package,
    minecraft: &str,
    lock: Option<&Lock>,
    target: &Target,
    client: &Modrinth,
) -> Option<String> {
    let key = format!("{}+{}", target.minecraft, target.loader.kind);
    let mut candidates: Vec<String> = Vec::new();
    let push = |value: Option<&String>, out: &mut Vec<String>| {
        if let Some(v) = value {
            let v = v.trim();
            if !v.is_empty() && v != "*" && !out.iter().any(|e| e == v) {
                out.push(v.to_string());
            }
        }
    };
    push(pkg.versions.get(&key), &mut candidates);
    push(pkg.versions.get(minecraft), &mut candidates);
    if let Some(v) = locked_for(lock, target, &pkg.key()) {
        push(Some(&v), &mut candidates);
    }
    push(pkg.versions.get("default"), &mut candidates);
    push(pkg.version.as_ref(), &mut candidates);

    let loaders = loaders_for(target);
    for wanted in candidates {
        if let Ok(r) = client
            .resolve_pinned(&pkg.key(), Some(&target.minecraft), &loaders, &wanted)
            .await
        {
            return Some(r.version.version_number.clone());
        }
    }
    match client
        .resolve(&pkg.key(), Some(&target.minecraft), &loaders)
        .await
    {
        Ok(r) => Some(r.version.version_number.clone()),
        Err(_) => None,
    }
}

fn locked_for(lock: Option<&Lock>, target: &Target, slug: &str) -> Option<String> {
    lock?
        .targets
        .iter()
        .find(|lt| lt.minecraft == target.minecraft && lt.loader.kind == target.loader.kind)?
        .packages
        .iter()
        .find(|lp| lp.project.eq_ignore_ascii_case(slug))
        .map(|lp| lp.version_number.clone())
}

async fn one_ver(cli: &Cli, api: &str, version: &str) -> Result<()> {
    let mut p = load_project(cli)?;
    let kept: Vec<Target> = p
        .softwares
        .targets
        .iter()
        .filter(|t| t.minecraft == version)
        .cloned()
        .collect();
    if kept.is_empty() {
        bail!(
            "no target for {version}, this pack has {}",
            p.softwares
                .targets
                .iter()
                .map(Targetish::label)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if kept.len() == p.softwares.targets.len() {
        say(cli, &format!("this pack is already only {version}"));
    } else {
        let dropped = p.softwares.targets.len() - kept.len();
        say(
            cli,
            &format!("dropping {dropped} target{}", if dropped == 1 { "" } else { "s" }),
        );
    }
    p.softwares.targets = kept.clone();
    for pkg in &mut p.manifest.packages {
        let chosen = kept
            .iter()
            .find_map(|t| {
                let key = format!("{}+{}", t.minecraft, t.loader.kind);
                pkg.versions
                    .get(&key)
                    .or_else(|| pkg.versions.get(&t.minecraft))
                    .cloned()
            })
            .or_else(|| pkg.version.clone())
            .or_else(|| pkg.versions.get("default").cloned())
            .or_else(|| pkg.versions.get("*").cloned())
            .unwrap_or_else(|| "*".to_string());
        pkg.versions.clear();
        pkg.version = Some(chosen);
    }
    p.manifest.write(&p.dir)?;
    p.softwares.write(&p.dir)?;
    do_lock(cli, api, &mut p, None, None).await?;
    for t in &kept {
        say(cli, &format!("keeping {}", t.label()));
    }
    Ok(())
}

fn pick_workflow(cli: &Cli, given: Option<WorkflowKind>) -> Result<WorkflowKind> {
    if let Some(w) = given {
        return Ok(w);
    }
    if !std::io::stdin().is_terminal() {
        say(
            cli,
            "no --workflow given and no terminal, its workflow will be From Source by default",
        );
        return Ok(WorkflowKind::Source);
    }
    let picked = inquire::Select::new(
        "What kind of workflow you want for grabbing Eh's Modpack?",
        vec!["From Source".to_string(), "From its Binaries".to_string()],
    )
    .prompt()
    .map_err(ask_failed)?;
    Ok(if picked.starts_with("From its B") {
        WorkflowKind::Binaries
    } else {
        WorkflowKind::Source
    })
}

async fn ask_missing(cli: &Cli, api: &str, mut plan: NewPlan, git_flag: bool) -> Result<NewPlan> {
    if !std::io::stdin().is_terminal() {
        if plan.name.is_none() || plan.minecraft.is_empty() || plan.loaders.is_empty() {
            bail!("this needs a terminal, pass --mc and --loader instead");
        }
        if git_flag {
            plan.git = Some(true);
        }
        plan.workflow = Some(pick_workflow(cli, plan.workflow)?);
        return Ok(plan);
    }
    if plan.name.is_none() {
        plan.name = Some(ask("What's the modpack name")?);
    }
    if plan.minecraft.is_empty() {
        plan.minecraft = ask_versions(api).await?;
    }
    if plan.loaders.is_empty() {
        let names: Vec<String> = LoaderKind::all()
            .iter()
            .map(|k| k.as_str().to_string())
            .collect();
        let picked = inquire::Select::new("Which Loader do you want", names)
            .prompt()
            .map_err(ask_failed)?;
        plan.loaders = vec![LoaderKind::parse(&picked).unwrap_or(LoaderKind::Fabric)];
    }
    if plan.workflow.is_none() {
        plan.workflow = Some(pick_workflow(cli, None)?);
    }
    if plan.git.is_none() {
        plan.git = Some(git_flag || ask_yes_no("Do you want to be a git repo?"));
    }
    Ok(plan)
}

async fn ask_versions(api: &str) -> Result<Vec<String>> {
    let releases: Vec<String> = Modrinth::new(api)?
        .game_versions()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.version_type == "release")
        .map(|t| t.name)
        .filter(|n| {
            !n.is_empty()
                && n.starts_with('1')
                && n.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .collect();
    if releases.is_empty() {
        let raw = ask("What version(s) will it be, comma separated")?;
        let picked = split_versions(&raw);
        if picked.is_empty() {
            bail!("pick at least one minecraft version");
        }
        return Ok(picked);
    }
    let picked = inquire::MultiSelect::new("What version(s) will it be", releases)
        .with_page_size(15)
        .prompt()
        .map_err(ask_failed)?;
    if picked.is_empty() {
        bail!("pick at least one minecraft version");
    }
    Ok(picked)
}

fn split_versions(raw: &str) -> Vec<String> {
    raw.split([',', ' ', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn ask(message: &str) -> Result<String> {
    inquire::Text::new(message).prompt().map_err(ask_failed)
}

fn ask_yes_no(message: &str) -> bool {
    inquire::Confirm::new(message)
        .with_default(false)
        .prompt()
        .unwrap_or(false)
}

fn ask_failed(e: inquire::InquireError) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn override_sources(p: &Project) -> Vec<(String, PathBuf)> {
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    sources.push((
        p.manifest.overrides.clone(),
        p.dir.join(&p.manifest.overrides),
    ));
    if p.manifest.client {
        sources.push((
            p.manifest.client_overrides.clone(),
            p.dir.join(&p.manifest.client_overrides),
        ));
    }
    if p.manifest.server {
        sources.push((
            p.manifest.server_overrides.clone(),
            p.dir.join(&p.manifest.server_overrides),
        ));
    }
    sources
}

#[allow(clippy::too_many_arguments)]
async fn test_cmd(
    cli: &Cli,
    api: &str,
    ver: Option<&str>,
    loader: Option<LoaderKind>,
    all: bool,
    how: Launcher,
    launch: Option<&str>,
    profile: Option<&str>,
    keep: bool,
) -> Result<()> {
    let p = load_project(cli)?;
    let lock = if Lock::exists(&p.dir) {
        Lock::load(&p.dir)?
    } else {
        lock_cmd(cli, api, None, None, false).await?;
        Lock::load(&p.dir)?
    };

    let chosen: Vec<LockedTarget> = if all {
        lock.targets.clone()
    } else if let Some(want) = ver {
        vec![lock.target(Some(want), loader)?.clone()]
    } else if let Some(want) = loader {
        vec![lock.target(None, Some(want))?.clone()]
    } else {
        vec![lock
            .targets
            .first()
            .context("this pack has no targets")?
            .clone()]
    };

    let sources = override_sources(&p);
    let client = Modrinth::new(api)?;

    for (index, target) in chosen.iter().enumerate() {
        let total = 6;
        let dir = staging::fresh_dir(&lock.name, target)?;
        step(
            cli,
            index as usize + 1,
            total * chosen.len(),
            &format!("staging {}", target.label()),
        );

        let copied = staging::copy_overrides(&sources, &dir)?;
        detail(cli, &format!("{copied} override files"));

        step(
            cli,
            index as usize + 2,
            total * chosen.len(),
            "downloading and verifying every file",
        );
        let staged = staging::download(&client, target, &dir, |line| detail(cli, line)).await?;
        detail(
            cli,
            &format!(
                "{} files, {}, all sha512 checked",
                staged.downloaded,
                staging::human(staged.bytes)
            ),
        );

        step(
            cli,
            index as usize + 3,
            total * chosen.len(),
            "checking the mods actually fit together",
        );
        let conflicts = staging::scan_conflicts(
            &dir,
            &modmeta::Target {
                minecraft: target.minecraft.clone(),
                java: target.java.clone().unwrap_or_default(),
                loader: target.loader.version.clone(),
            },
        );
        if !conflicts.warnings.is_empty() {
            detail(cli, &format!("{} soft note(s)", conflicts.warnings.len()));
        }
        if conflicts.errors.is_empty() {
            detail(cli, "no dependency conflicts");
        } else {
            println!();
            say(
                cli,
                &format!(
                    "{} conflict{} found",
                    conflicts.errors.len(),
                    if conflicts.errors.len() == 1 { "" } else { "s" }
                ),
            );
            for c in &conflicts.errors {
                eprintln!(
                    "{}",
                    format!(
                        "  {} {} {}, wanted {}, found {}",
                        c.offender, c.kind, c.other, c.wanted, c.found
                    )
                    .red()
                    .bold()
                );
            }
            let go = inquire::Confirm::new("launch it anyway?")
                .with_default(false)
                .prompt()
                .map_err(ask_failed)?;
            if !go {
                say(cli, "stopping before the game starts");
                if keep {
                    say(cli, &format!("kept {}", dir.display()));
                } else {
                    let _ = staging::wipe(&dir);
                }
                return Ok(());
            }
            println!();
        }

        step(
            cli,
            index as usize + 4,
            total * chosen.len(),
            "installing the loader and the game itself",
        );
        let http = crate::http::Http::new()?;
        let base = dir.join(".minecraft-ehmodpack");
        let natives = minecraft::install(&http, &base, target, |line| detail(cli, line)).await?;
        detail(
            cli,
            &format!(
                "mc {}  {} {}  java {}  libs {}  assets {}",
                target.minecraft,
                target.loader.kind,
                target.loader.version,
                natives.java_major,
                natives.libraries,
                natives.assets
            ),
        );

        step(
            cli,
            index as usize + 4,
            total * chosen.len(),
            "starting the game",
        );
        let mut child = minecraft::spawn(&natives, &dir)?;
        detail(cli, &format!("pid {}", child.id()));

        match how {
            Launcher::None => {
                say(cli, &format!("instance is at {}", dir.display()));
                staging::wait_for_enter("press enter to stop the game and clean up: ");
            }
            _ => {
                let _ = child.wait();
            }
        }
        let _ = (launch, profile);

        step(
            cli,
            index as usize + 5,
            total * chosen.len(),
            "cleaning up",
        );
        if keep {
            say(cli, &format!("kept {}", dir.display()));
        } else if let Err(e) = staging::wipe(&dir) {
            warn(cli, &format!("could not delete {}: {e}", dir.display()));
        } else {
            detail(cli, "temporary instance deleted");
        }
    }
    Ok(())
}
async fn fetch_and_check(
    client: &Modrinth,
    pkg: &LockedPackage,
    out: &std::path::Path,
) -> Result<()> {
    let url = pkg
        .downloads
        .first()
        .with_context(|| format!("{} has no download url", pkg.project))?;
    if out.is_file() {
        let body = std::fs::read(out)?;
        if crate::staging::check_sha512(&pkg.project, &body, &pkg.hashes.sha512).is_ok() {
            return Ok(());
        }
    }
    let body = client.get_bytes(url).await?;
    crate::staging::check_sha512(&pkg.project, &body, &pkg.hashes.sha512)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, &body)?;
    Ok(())
}

fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn matches_entry(
    entry: &str,
    candidates: &[(String, String, String, String)],
) -> Option<usize> {
    let bare = entry.trim_start_matches("file/");
    if let Some(i) = candidates
        .iter()
        .position(|(_, file, _, _)| file.eq_ignore_ascii_case(bare))
    {
        return Some(i);
    }
    let want = norm(bare);
    candidates.iter().position(|(_, _, slug, _)| {
        let have = norm(slug);
        !have.is_empty() && have.len() >= 3 && want.contains(&have)
    })
}

async fn sync_pack_active(
    cli: &Cli,
    api: &str,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    no_download: bool,
) -> Result<()> {
    let mut p = load_project(cli)?;
    let mut lock = if Lock::exists(&p.dir) {
        Lock::load(&p.dir)?
    } else {
        Lock::stamped(
            String::new(),
            p.manifest.fingerprint(),
            p.manifest.name.clone(),
            p.manifest.version.clone(),
            p.manifest.summary.clone(),
            Vec::new(),
        )
    };
    let wanted: Vec<String> = if mc.is_some() || loader.is_some() {
        vec![lock.target(mc, loader)?.label()]
    } else {
        lock.labels()
    };

    let sources = override_sources(&p);
    let mut on_disk = build::pack_paths_on_disk(&sources);
    let client = Modrinth::new(api)?;

    let options_path = p.dir.join(&p.manifest.client_overrides).join("options.txt");
    let existing = std::fs::read_to_string(&options_path).unwrap_or_else(|_| {
        "resourcePacks:[\"vanilla\"]\nincompatibleResourcePacks:[]\n".to_string()
    });
    let listed: Vec<String> = existing
        .lines()
        .find(|l| l.trim_start().starts_with("resourcePacks:"))
        .and_then(|l| {
            let start = l.find('[')?;
            let end = l.rfind(']')?;
            serde_json::from_str::<Vec<String>>(&format!("[{}]", &l[start + 1..end])).ok()
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e != "vanilla")
        .collect();

    let candidates: Vec<(String, String, String, String)> = lock
        .targets
        .iter()
        .filter(|t| wanted.contains(&t.label()))
        .flat_map(|t| t.packages.iter())
        .filter(|pkg| matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader))
        .map(|pkg| {
            (
                pkg.project_id.clone(),
                pkg.path.rsplit('/').next().unwrap_or(&pkg.path).to_string(),
                pkg.project.clone(),
                pkg.hashes.sha1.clone(),
            )
        })
        .collect();

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut on: BTreeSet<String> = BTreeSet::new();
    let mut matched_files: BTreeSet<String> = BTreeSet::new();

    for entry in &listed {
        let bare = entry.trim_start_matches("file/").to_string();
        if let Some(i) = matches_entry(entry, &candidates) {
            let (_, _, slug, _) = &candidates[i];
            on.insert(slug.clone());
            matched_files.insert(bare);
            rows.push(vec![slug.clone(), entry.clone(), "on, matched by name".to_string()]);
            continue;
        }
        let here = on_disk.iter().find(|(n, _)| *n == bare);
        let sha = here.and_then(|(_, path)| build::sha1_of(path));
        if let Some(h) = &sha {
            if let Some(i) = candidates.iter().position(|(_, _, _, locked)| locked == h) {
                let (_, _, slug, _) = &candidates[i];
                on.insert(slug.clone());
                matched_files.insert(bare);
                rows.push(vec![
                    slug.clone(),
                    entry.clone(),
                    "on, matched the lock by sha".to_string(),
                ]);
                continue;
            }
        }
        let found = match sha.as_deref() {
            Some(h) => client.version_file(h).await.unwrap_or(None),
            None => None,
        };
        match found {
            Some(v) => {
                let slug = match client.project(&v.project_id).await {
                    Ok(pr) => pr.slug,
                    Err(_) => v.project_id.clone(),
                };
                on.insert(slug.clone());
                matched_files.insert(bare);
                rows.push(vec![
                    slug,
                    v.version_number.clone(),
                    "on, matched modrinth by hash".to_string(),
                ]);
            }
            None => rows.push(vec![
                "(not on modrinth)".to_string(),
                entry.clone(),
                if here.is_some() {
                    "on, local file"
                } else {
                    "on, but there is nothing here to check it against"
                }
                .to_string(),
            ]),
        }
    }

    if !no_download {
        step(
            cli,
            1,
            2,
            "downloading the active packs and checking them against the lockfile",
        );
        let dest = p.dir.join(&p.manifest.overrides).join("resourcepacks");
        let mut steps = crate::progress::Steps::new(on.len());
        let mut n = 0usize;
        let mut bad: Vec<String> = Vec::new();
        for target in lock.targets.iter_mut() {
            if !wanted.contains(&target.label()) {
                continue;
            }
            for pkg in &target.packages {
                if !on.contains(&pkg.project) {
                    continue;
                }
                n += 1;
                steps.at(n, &pkg.project);
                let file = pkg.path.rsplit('/').next().unwrap_or(&pkg.path).to_string();
                let out = dest.join(&file);
                if let Err(e) = fetch_and_check(&client, pkg, &out).await {
                    bad.push(format!("{} {file}: {e:#}", pkg.project));
                    continue;
                }
                on_disk.push((file.clone(), out));
                matched_files.insert(file);
            }
        }
        steps.finish();
        for line in &bad {
            warn(cli, line);
        }
        if bad.is_empty() {
            say(cli, &format!("{n} active packs fetched and hash checked"));
        }
    }

    for (name, _) in &on_disk {
        if !matched_files.contains(name) {
            rows.push(vec![
                "(not on modrinth)".to_string(),
                name.clone(),
                "off".to_string(),
            ]);
        }
    }
    for (_, file, slug, _) in &candidates {
        if !matched_files.contains(file) {
            rows.push(vec![
                slug.clone(),
                file.clone(),
                "off, options.txt did not list it".to_string(),
            ]);
        }
    }

    let mut changed = 0usize;
    for pkg in &mut p.manifest.packages {
        let is_pack = matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader);
        let next = if is_pack && on.contains(&pkg.key()) {
            Some(true)
        } else {
            None
        };
        if pkg.active != next {
            pkg.active = next;
            changed += 1;
        }
    }
    for target in &mut lock.targets {
        if !wanted.contains(&target.label()) {
            continue;
        }
        for pkg in &mut target.packages {
            let is_pack = matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader);
            let want = is_pack && on.contains(&pkg.project);
            let next = if want { Some(RpPosition::default()) } else { None };
            if pkg.position != next {
                pkg.position = next;
                if want {
                    pkg.env.client = Support::Required;
                }
                changed += 1;
            }
        }
    }

    p.manifest.write(&p.dir)?;
    lock.manifest_hash = p.manifest.fingerprint();
    lock.write(&p.dir)?;

    let mut generated: Vec<String> = on_disk
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    for (name, _) in &on_disk {
        if !generated.iter().any(|g| g == name) {
            generated.push(name.clone());
        }
    }
    for (_, file, slug, _) in &candidates {
        if on.contains(slug) && !generated.iter().any(|g| g == file) {
            generated.push(file.clone());
        }
    }
    let local: Vec<String> = on_disk.iter().map(|(n, _)| n.clone()).collect();
    let canonical = build::inject_active(&existing, &generated, &local);
    manifest::write_file(&options_path, &canonical)?;

    println!();
    say(
        cli,
        &format!("read the enabled packs out of {}", options_path.display()),
    );
    println!(
        "{}",
        table(
            cli,
            &["package", "file", "what options.txt said"],
            &rows,
            &vec![false; rows.len()]
        )
    );
    println!();
    say(
        cli,
        &format!(
            "{changed} activ{} changed",
            if changed == 1 { "e" } else { "es" }
        ),
    );
    say(
        cli,
        &format!(
            "on: {}",
            if on.is_empty() {
                "nothing".to_string()
            } else {
                on.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        ),
    );
    say(cli, &format!("{MANIFEST_FILE} and {LOCK_FILE} are up to date"));
    say(
        cli,
        &format!("wrote {} too", options_path.display()),
    );
    for line in canonical.lines().filter(|l| l.starts_with("resourcePacks:")) {
        println!("  {line}");
    }
    say(cli, "build still rewrites it per target inside each .mrpack");
    Ok(())
}

async fn fix_loader(
    cli: &Cli,
    api: &str,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    all: bool,
    dry_run: bool,
) -> Result<()> {
    let mut p = load_project(cli)?;
    let targets: Vec<Target> = if all || (mc.is_none() && loader.is_none()) {
        p.softwares.targets.clone()
    } else {
        vec![manifest::pick(&p.softwares.targets, mc, loader)?.clone()]
    };
    let single = p.softwares.targets.len() == 1;
    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };
    if lock.is_none() {
        say(cli, "nothing is locked yet, working off packages.json alone");
    }
    let client = Modrinth::new(api)?;

    let total: usize = targets.len() * p.manifest.packages.len();
    let mut steps = crate::progress::Steps::new(total);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut changed = 0usize;
    let mut n = 0usize;

    for target in &targets {
        let key = format!("{}+{}", target.minecraft, target.loader.kind);
        let loaders = vec![target.loader.kind.clone()];
        for idx in 0..p.manifest.packages.len() {
            n += 1;
            steps.at(n, &p.manifest.packages[idx].key());
            let slug = p.manifest.packages[idx].key();
            if slug.is_empty() {
                continue;
            }
            let pinned = p.manifest.packages[idx]
                .version_for(&target.minecraft, &target.loader.kind);
            let locked = lock.as_ref().and_then(|l| {
                l.targets
                    .iter()
                    .find(|t| {
                        t.minecraft == target.minecraft && t.loader.kind == target.loader.kind
                    })
                    .and_then(|t| {
                        t.packages
                            .iter()
                            .find(|lp| lp.project.eq_ignore_ascii_case(&slug))
                            .map(|lp| lp.version_number.clone())
                    })
            });
            if p.manifest.packages[idx].is_locked() {
                continue;
            }
            if p.manifest.packages[idx].is_skipped(&target.minecraft, &target.loader.kind) {
                rows.push(vec![
                    slug,
                    "skipped".to_string(),
                    "not on this target".to_string(),
                ]);
                continue;
            }
            let Ok(project) = client.project(&slug).await else {
                rows.push(vec![slug, "?".to_string(), "not on modrinth".to_string()]);
                continue;
            };
            let is_mod = p.manifest.packages[idx].kind == PkgType::Mod;
            let want = if is_mod { loaders.clone() } else { Vec::new() };
            let Ok(list) = client
                .versions(&project, Some(&target.minecraft), &want)
                .await
            else {
                rows.push(vec![slug, "?".to_string(), "could not list versions".to_string()]);
                continue;
            };
            if list.is_empty() {
                rows.push(vec![
                    slug,
                    locked.clone().or(pinned.clone()).unwrap_or_else(|| "*".to_string()),
                    if is_mod {
                        format!("no {} build for {}", target.loader.kind, target.minecraft)
                    } else {
                        format!("no build for {}", target.minecraft)
                    },
                ]);
                continue;
            }
            let locked_fits = match &locked {
                Some(v) if v != "*" => list.iter().any(|x| &x.version_number == v),
                _ => true,
            };
            let pinned_fits = match &pinned {
                Some(v) if v != "*" => list.iter().any(|x| &x.version_number == v),
                _ => false,
            };
            if locked_fits && (pinned_fits || locked.is_some()) {
                continue;
            }
            let best = crate::modrinth::best_of(list);
            rows.push(vec![
                slug,
                locked
                    .clone()
                    .or(pinned.clone())
                    .unwrap_or_else(|| "*".to_string()),
                best.clone(),
            ]);
            if !dry_run {
                set_pin(&mut p.manifest, &project.slug, &key, &best, single);
            }
            changed += 1;
        }
    }
    steps.finish();

    println!();
    say(
        cli,
        &format!(
            "{} version{} did not fit the loader",
            changed,
            if changed == 1 { "" } else { "s" }
        ),
    );
    if rows.is_empty() {
        say(cli, "everything already matches its target loader");
    } else {
        println!(
            "{}",
            table(
                cli,
                &["package", "was", "now"],
                &rows,
                &vec![false; rows.len()]
            )
        );
    }
    if dry_run {
        say(cli, "nothing was written");
        return Ok(());
    }
    if changed == 0 {
        return Ok(());
    }
    p.manifest.write(&p.dir)?;
    say(cli, &format!("wrote {MANIFEST_FILE}"));
    let go = inquire::Confirm::new("lock the targets that changed?")
        .with_default(true)
        .prompt()
        .map_err(ask_failed)?;
    if !go {
        return Ok(());
    }
    for target in &targets {
        let mut lock_project = Project {
            dir: p.dir.clone(),
            manifest: p.manifest.clone(),
            softwares: p.softwares.clone(),
        };
        do_lock(
            cli,
            api,
            &mut lock_project,
            Some(&target.minecraft),
            LoaderKind::parse(&target.loader.kind),
        )
        .await?;
    }
    Ok(())
}

struct VerConflict {
    project: String,
    label: String,
    loader: String,
    version_number: String,
    detail: String,
}

async fn verify_cmd(
    cli: &Cli,
    api: &str,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    all: bool,
    keep_going: bool,
) -> Result<()> {
    let mut p = load_project(cli)?;
    let lock = if Lock::exists(&p.dir) {
        Lock::load(&p.dir)?
    } else {
        say(cli, "nothing is locked yet, run `ehmodpack lock` first");
        return Ok(());
    };
    let targets: Vec<LockedTarget> = if all || (mc.is_none() && loader.is_none()) {
        lock.targets.clone()
    } else {
        vec![lock.target(mc, loader)?.clone()]
    };

    let client = Modrinth::new(api)?;
    let total: usize = targets.iter().map(|t| t.packages.len()).sum();
    let mut steps = crate::progress::Steps::new(total);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    let mut wrong_id: Vec<String> = Vec::new();
    let mut version_conflicts: Vec<VerConflict> = Vec::new();
    let mut metas: Vec<modmeta::ModMeta> = Vec::new();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut n = 0usize;

    for target in &targets {
        for pkg in &target.packages {
            n += 1;
            steps.at(n, &pkg.project);
            let url = match pkg.downloads.first() {
                Some(u) => u.clone(),
                None => {
                    rows.push(vec![
                        pkg.project.clone(),
                        pkg.version_number.clone(),
                        "no download url".to_string(),
                    ]);
                    bad.push(format!("{} has no download url", pkg.project));
                    continue;
                }
            };
            let body = match client.get_bytes(&url).await {
                Ok(b) => b,
                Err(e) => {
                    rows.push(vec![
                        pkg.project.clone(),
                        pkg.version_number.clone(),
                        format!("could not download: {e}"),
                    ]);
                    bad.push(format!("{} could not be downloaded: {e}", pkg.project));
                    if !keep_going {
                        steps.finish();
                        eprintln!("{}", format!("  {e}").red().bold());
                        bail!("stopped at the first failure, pass --keep-going to carry on");
                    }
                    continue;
                }
            };
            if let Ok(Some(meta)) = modmeta::read_bytes(&body) {
                if !meta.id.is_empty() {
                    metas.push(meta.clone());
                    ids.entry(meta.id.to_ascii_lowercase())
                        .or_insert_with(|| pkg.project.clone());
                    if pkg.kind == PkgType::Mod {
                        let known = p.manifest.packages.iter().find(|m| {
                            m.key().eq_ignore_ascii_case(&pkg.project)
                        });
                        if let Some(pkg_known) = known {
                            if let Some(list) = &pkg_known.ids {
                                if !list.is_empty()
                                    && !list.iter().any(|k| k.eq_ignore_ascii_case(&meta.id))
                                {
                                    wrong_id.push(format!(
                                        "{} identifies as {} but this project is known to be {}",
                                        pkg.project,
                                        meta.id,
                                        list.join(", ")
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            if pkg.kind == PkgType::Mod && !pkg.version_id.is_empty() {
                if let Ok(api_ver) = client.version(&pkg.version_id).await {
                    let loaders = &api_ver.loaders;
                    if !loaders.is_empty()
                        && !loaders
                            .iter()
                            .any(|l| l.eq_ignore_ascii_case(&target.loader.kind))
                    {
                        version_conflicts.push(VerConflict {
                            project: pkg.project.clone(),
                            label: target.label(),
                            loader: target.loader.kind.clone(),
                            version_number: pkg.version_number.clone(),
                            detail: format!(
                                "{} was downloaded for {} for {} when this is a {} pack",
                                pkg.project,
                                pkg.version_number,
                                loaders.join("/"),
                                target.loader.kind
                            ),
                        });
                    }
                    let listed = &api_ver.game_versions;
                    if !listed.is_empty() && !modmeta::game_mc_ok(&target.minecraft, listed) {
                        version_conflicts.push(VerConflict {
                            project: pkg.project.clone(),
                            label: target.label(),
                            loader: target.loader.kind.clone(),
                            version_number: pkg.version_number.clone(),
                            detail: format!(
                                "The version of minecraft for {} was downloaded incorrectly ({} does not support {})",
                                pkg.project,
                                pkg.version_number,
                                target.minecraft
                            ),
                        });
                    }
                }
            }
            match staging::verify(
                &pkg.project,
                &body,
                &pkg.hashes.sha1,
                &pkg.hashes.sha512,
            ) {
                Ok(()) => rows.push(vec![
                    pkg.project.clone(),
                    pkg.version_number.clone(),
                    format!("ok, {}", staging::human(body.len() as u64)),
                ]),
                Err(e) => {
                    rows.push(vec![
                        pkg.project.clone(),
                        pkg.version_number.clone(),
                        format!("{e:#}"),
                    ]);
                    bad.push(format!("{} {}", pkg.project, e));
                    if !keep_going {
                        steps.finish();
                        eprintln!("{}", format!("  {e}").red().bold());
                        bail!("stopped at the first failure, pass --keep-going to carry on");
                    }
                }
            }
        }
    }
    steps.finish();

    println!();
    say(cli, "checked every file against its hashes");
    println!(
        "{}",
        table(
            cli,
            &["package", "version", "result"],
            &rows,
            &vec![false; rows.len()]
        )
    );

    if !wrong_id.is_empty() {
        println!();
        say(
            cli,
            &format!(
                "{} mod{} changed identity, nothing is auto-fixed for those",
                wrong_id.len(),
                if wrong_id.len() == 1 { "" } else { "s" }
            ),
        );
        for line in &wrong_id {
            eprintln!("{}", line.red().bold());
        }
        bail!("some mods changed identity, update packages.json by hand before continuing");
    }

    let mut per_target: Vec<(String, Vec<modmeta::ModMeta>)> = Vec::new();
    for target in &targets {
        let mut metas: Vec<modmeta::ModMeta> = Vec::new();
        for pkg in &target.packages {
            let Some(url) = pkg.downloads.first() else {
                continue;
            };
            let Ok(body) = client.get_bytes(url).await else {
                continue;
            };
            if let Ok(Some(meta)) = modmeta::read_bytes(&body) {
                if !meta.id.is_empty() {
                    metas.push(meta);
                }
            }
        }
        per_target.push((target.label(), metas));
    }

    let mut errors: Vec<(String, modmeta::Conflict)> = Vec::new();
    let mut warnings: Vec<(String, modmeta::Conflict)> = Vec::new();
    for (label, metas) in &per_target {
        let Some(t) = targets
            .iter()
            .find(|t| manifest::Targetish::label(*t) == *label)
        else {
            continue;
        };
        let info = modmeta::Target {
            minecraft: t.minecraft.clone(),
            java: t.java.clone().unwrap_or_default(),
            loader: t.loader.version.clone(),
        };
        let report = modmeta::check(metas, &info);
        for c in report.errors {
            errors.push((label.clone(), c));
        }
        for c in report.warnings {
            warnings.push((label.clone(), c));
        }
    }

    if !warnings.is_empty() {
        println!();
        notice(
            cli,
            &format!(
                "{} soft conflict{} (Those are warnings!)",
                warnings.len(),
                if warnings.len() == 1 { "" } else { "s" }
            ),
        );
        for (label, c) in &warnings {
            println!(
                "  {label}: {} breaks {} (wants {}), found {}",
                c.offender, c.other, c.wanted, c.found
            );
        }
    }

    if !errors.is_empty() {
        println!();
        say(
            cli,
            &format!(
                "{} real conflict{}",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            ),
        );
        for (label, c) in &errors {
            eprintln!(
                "{}",
                format!(
                    "  {label}: {} {} {}, wanted {}, found {}",
                    c.offender, c.kind, c.other, c.wanted, c.found
                )
                .red()
                .bold()
            );
        }
    }

    if !version_conflicts.is_empty() {
        println!();
        say(
            cli,
            &format!(
                "{} version conflict{}",
                version_conflicts.len(),
                if version_conflicts.len() == 1 { "" } else { "s" }
            ),
        );
        for v in &version_conflicts {
            eprintln!("  {}: {}", v.label, v.detail.red().bold());
        }
    }

    if bad.is_empty()
        && errors.is_empty()
        && warnings.is_empty()
        && version_conflicts.is_empty()
    {
        println!();
        say(
            cli,
            &format!("{n} files, every hash matched and the mods fit together"),
        );
        return Ok(());
    }

    println!();
    if !bad.is_empty() {
        for line in &bad {
            eprintln!("{}", format!("  {line}").red().bold());
        }
    }

    let mut manifest_ids: BTreeMap<String, String> = BTreeMap::new();
    for m in &p.manifest.packages {
        if let Some(list) = &m.ids {
            for id in list {
                manifest_ids
                    .entry(id.to_ascii_lowercase())
                    .or_insert_with(|| m.key());
            }
        }
    }

    let fix = inquire::Confirm::new("fix these?")
        .with_default(true)
        .prompt()
        .map_err(ask_failed)?;
    if !fix {
        say(
            cli,
            &format!(
                "Ok i am not doing those, but the {} version conflicts, {} conflicts, {} soft conflicts and {} bad hashes will be as it is",
                version_conflicts.len(),
                errors.len(),
                warnings.len(),
                bad.len(),
            ),
        );
        return Ok(());
    }

    let label = targets.first().map(|t| t.label()).unwrap_or_default();
    let fix_target = targets.first().map(|t| (t.minecraft.clone(), t.loader.kind.clone()));
    let mut fixes: Vec<String> = Vec::new();
    for (err_label, c) in &errors {
        if c.kind != "needs" || modmeta::is_platform(&c.other) {
            continue;
        }
        let Some(t) = targets
            .iter()
            .find(|t| manifest::Targetish::label(*t) == *err_label)
        else {
            continue;
        };
        let range = if c.wanted.is_empty() || c.wanted == "any version" {
            "*"
        } else {
            c.wanted.as_str()
        };
        let loaders = vec![t.loader.kind.clone()];
        let key = format!("{}+{}", t.minecraft, t.loader.kind);
        match fix_missing(
            cli,
            &client,
            &c.other,
            Some(&t.minecraft),
            &loaders,
            range,
            &ids,
            &manifest_ids,
        )
        .await
        {
            Ok((slug, version)) => {
                if pkg_is_locked(&p.manifest, &slug) {
                    warn(cli, &format!("{slug} is locked, if you meant to update {slug} edit slug to false or delete the slug line"));
                } else {
                    set_pin(&mut p.manifest, &slug, &key, &version, targets.len() == 1);
                    fixes.push(format!(
                        "{err_label}: {} -> {version} for {key} (wanted {range})",
                        c.other
                    ));
                }
            }
            Err(e) => warn(
                cli,
                &format!("could not satisfy {} {}: {e:#}", c.other, c.wanted),
            ),
        }
    }
    let mut warn_fixes: Vec<String> = Vec::new();
    for (warn_label, c) in &warnings {
        if c.wanted.is_empty() || c.wanted == "*" || modmeta::is_platform(&c.other) {
            continue;
        }
        if !modmeta::satisfied(&c.wanted, &c.found) {
            continue;
        }
        let Some(t) = targets
            .iter()
            .find(|t| manifest::Targetish::label(*t) == *warn_label)
        else {
            continue;
        };
        let loaders = vec![t.loader.kind.clone()];
        let key = format!("{}+{}", t.minecraft, t.loader.kind);
        match fix_breaks(
            cli,
            &client,
            &c.other,
            &c.wanted,
            Some(&t.minecraft),
            &loaders,
            &ids,
            &manifest_ids,
        )
        .await
        {
            Ok((slug, version)) => {
                if pkg_is_locked(&p.manifest, &slug) {
                    warn(cli, &format!("{slug} is locked, if you meant to update {slug} edit slug to false or delete the slug line"));
                } else {
                    set_pin(&mut p.manifest, &slug, &key, &version, targets.len() == 1);
                    warn_fixes.push(format!(
                        "{warn_label}: {slug} -> {version} for {key} (was {} inside {})",
                        c.found, c.wanted
                    ));
                }
            }
            Err(e) => warn(
                cli,
                &format!("could not step {} out of {}: {e:#}", c.other, c.wanted),
            ),
        }
    }
    let mut vc_fixes: Vec<String> = Vec::new();
    for v in &version_conflicts {
        let Some(t) = targets
            .iter()
            .find(|t| manifest::Targetish::label(*t) == v.label)
        else {
            continue;
        };
        let loaders = vec![t.loader.kind.clone()];
        let Ok(project) = client.project(&v.project).await else {
            warn(cli, &format!("could not fix {}: not on modrinth", v.project));
            continue;
        };
        let Ok(list) = client
            .versions(&project, Some(&t.minecraft), &loaders)
            .await
        else {
            warn(cli, &format!("could not fix {}: version list failed", v.project));
            continue;
        };
        let key = format!("{}+{}", t.minecraft, t.loader.kind);
        let same = list
            .iter()
            .find(|x| x.version_number == v.version_number)
            .cloned();
        let chosen = if let Some(x) = same {
            x
        } else if !list.is_empty() {
            let best = crate::modrinth::best_of(list.clone());
            let Some(x) = list.into_iter().find(|x| x.version_number == best) else {
                continue;
            };
            x
        } else {
            continue;
        };
        if pkg_is_locked(&p.manifest, &v.project) {
            warn(
                cli,
                &format!(
                    "{} is locked, meaning the updates are disabled for {}",
                    v.project, v.project
                ),
            );
            continue;
        }
        set_pin(
            &mut p.manifest,
            &project.slug,
            &key,
            &chosen.version_number,
            targets.len() == 1,
        );
        if chosen.version_number == v.version_number {
            vc_fixes.push(format!(
                "{}: the version of {} can be replaced safely with the same version for {}",
                v.label, v.project, v.loader
            ));
        } else {
            vc_fixes.push(format!(
                "{}: a correct version will be downloaded for the right loader because there wasn't a similar one for {} ({})",
                v.label, v.loader, chosen.version_number
            ));
        }
    }
    if !fixes.is_empty() || !warn_fixes.is_empty() || !vc_fixes.is_empty() {
        p.manifest.write(&p.dir)?;
        if !fixes.is_empty() {
            let count = fixes.len();
            say(cli, &format!("{count} package(s) adjusted so the mods fit"));
            for line in &fixes {
                println!("  {line}");
            }
        }
        if !warn_fixes.is_empty() {
            let count = warn_fixes.len();
            say(cli, &format!("{count} soft conflict(s) fixed"));
            for line in &warn_fixes {
                println!("  {line}");
            }
        }
        if !vc_fixes.is_empty() {
            let count = vc_fixes.len();
            say(cli, &format!("{count} version conflict(s) fixed"));
            for line in &vc_fixes {
                println!("  {line}");
            }
        }
    }
    say(cli, &format!("locking {label} again"));
    match &fix_target {
        Some((mc, kind)) => lock_cmd(
            cli,
            api,
            Some(mc),
            LoaderKind::parse(kind),
            false,
        )
        .await,
        None => Ok(()),
    }
}

fn candidate_slugs(
    mod_id: &str,
    ids: &BTreeMap<String, String>,
    manifest_ids: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut slugs: Vec<String> = Vec::new();
    let push = |slugs: &mut Vec<String>, s: &str| {
        if !s.is_empty() && !slugs.iter().any(|x| x.eq_ignore_ascii_case(s)) {
            slugs.push(s.to_string());
        }
    };
    if let Some(known) = ids.get(&mod_id.to_ascii_lowercase()) {
        push(&mut slugs, known);
    }
    if let Some(known) = manifest_ids.get(&mod_id.to_ascii_lowercase()) {
        push(&mut slugs, known);
    }
    push(&mut slugs, mod_id);
    if mod_id.contains('_') {
        push(&mut slugs, &mod_id.replace('_', "-"));
    }
    slugs
}

async fn fix_missing(
    cli: &Cli,
    client: &Modrinth,
    mod_id: &str,
    mc: Option<&str>,
    loaders: &[String],
    range: &str,
    ids: &BTreeMap<String, String>,
    manifest_ids: &BTreeMap<String, String>,
) -> Result<(String, String)> {
    let mut slugs = candidate_slugs(mod_id, ids, manifest_ids);
    if let Ok(found) = client
        .search_page(&SearchFilters {
            query: mod_id.to_string(),
            versions: mc.iter().copied().map(str::to_string).collect(),
            sort: Some(Sort::Downloads),
            limit: 10,
            ..SearchFilters::default()
        })
        .await
    {
        for hit in &found.hits {
            if !slugs.contains(&hit.slug) {
                slugs.push(hit.slug.clone());
            }
        }
    }

    let mut seen_candidate = false;
    for slug in &slugs {
        let Ok(found) = client.resolve_in_range(slug, mc, loaders, range).await else {
            continue;
        };
        seen_candidate = true;
        let real = found.project.slug.clone();
        let version = found.version.version_number.clone();
        let confirmed = match client.get_bytes(&found.file.url).await {
            Ok(body) => matches!(
                modmeta::read_bytes(&body),
                Ok(Some(meta)) if meta.id.eq_ignore_ascii_case(mod_id)
            ),
            Err(_) => false,
        };
        if confirmed {
            return Ok((real, version));
        }
        detail(
            cli,
            &format!("{slug} is not {mod_id} when cracked open, skipping it"),
        );
    }
    if seen_candidate {
        bail!(
            "no modrinth build of {mod_id} can be confirmed for {} (the candidates were not that mod)",
            mc.unwrap_or("any minecraft version")
        );
    }
    bail!(
        "no modrinth project provides {mod_id} in {range} for {}",
        mc.unwrap_or("any minecraft version")
    );
}

async fn fix_breaks(
    cli: &Cli,
    client: &Modrinth,
    mod_id: &str,
    range: &str,
    mc: Option<&str>,
    loaders: &[String],
    ids: &BTreeMap<String, String>,
    manifest_ids: &BTreeMap<String, String>,
) -> Result<(String, String)> {
    let slugs = candidate_slugs(mod_id, ids, manifest_ids);
    let mut seen_candidate = false;
    for slug in &slugs {
        let Ok(found) = client.resolve_outside(slug, mc, loaders, range).await else {
            continue;
        };
        seen_candidate = true;
        let real = found.project.slug.clone();
        let version = found.version.version_number.clone();
        let confirmed = match client.get_bytes(&found.file.url).await {
            Ok(body) => matches!(
                modmeta::read_bytes(&body),
                Ok(Some(meta)) if meta.id.eq_ignore_ascii_case(mod_id)
            ),
            Err(_) => false,
        };
        if confirmed {
            return Ok((real, version));
        }
        detail(
            cli,
            &format!("{slug} is not {mod_id} when cracked open, skipping it"),
        );
    }
    if seen_candidate {
        bail!(
            "no modrinth build of {mod_id} can be confirmed out of {range} (the candidates were not that mod)"
        );
    }
    bail!(
        "no modrinth version of {mod_id} stays out of {range} for {}",
        mc.unwrap_or("any minecraft version")
    );
}

fn set_pin(manifest: &mut Manifest, slug: &str, key: &str, version: &str, single: bool) {
    if let Some(pkg) = manifest
        .packages
        .iter_mut()
        .find(|x| x.key().eq_ignore_ascii_case(slug))
    {
        if pkg.is_locked() {
            return;
        }
        if single || pkg.versions.is_empty() {
            pkg.versions.clear();
            pkg.version = Some(version.to_string());
        } else {
            pkg.versions.insert(key.to_string(), version.to_string());
        }
        return;
    }
    let mut versions = BTreeMap::new();
    versions.insert(key.to_string(), version.to_string());
    manifest.packages.push(Package {
        kind: PkgType::Mod,
        project: Some(slug.to_string()),
        id: None,
        version: if single { Some(version.to_string()) } else { None },
        versions: if single { BTreeMap::new() } else { versions },
        skip: Vec::new(),
        env: None,
        optional: None,
        active: None,
        position: None,
        positions: BTreeMap::new(),
        ids: None,
        lock: None,
    });
}

fn pkg_is_locked(manifest: &Manifest, slug: &str) -> bool {
    manifest
        .packages
        .iter()
        .find(|x| x.key().eq_ignore_ascii_case(slug))
        .is_some_and(Package::is_locked)
}

fn print_report(cli: &Cli, report: &validate::Report) -> Result<()> {
    for line in &report.warnings {
        warn(cli, line);
    }
    for line in &report.errors {
        eprintln!("{}", format!("  {line}").red().bold());
    }
    if report.ok() {
        say(
            cli,
            &format!(
                "everything checks out, {} warning{}",
                report.warnings.len(),
                if report.warnings.len() == 1 { "" } else { "s" }
            ),
        );
        return Ok(());
    }
    bail!(
        "{} thing{} wrong, not building",
        report.errors.len(),
        if report.errors.len() == 1 { "" } else { "s" }
    )
}

const SCOPES_NEEDED: &str = "Read Projects, Create Versions and Write Versions";
const BAD_SCOPES: &str = "Sorry, you did not select Read Projects, Create Versions and Write Versions when generating the token, edit it to have it!";
const BAD_TOKEN: &str = "That token was not accepted by modrinth, check you copied the whole thing, the token is shown only once when you make it";
const EXPIRED: &str = "Sorry, your token has expired, please regenerate your token from the settings";

fn modrinth_gave_up(why: &str) -> anyhow::Error {
    anyhow::anyhow!(crate::modrinth::with_details(
        "We couldn't connect to modrinth sorry!",
        why
    ))
}

const PAT_PAGE: &str = "https://modrinth.com/settings/pats";

fn explain_pat() {
    println!("If this is usually your first time publishing congrats, but also, for explainers:");
    println!();
    println!("The Modrinth Token is required for when doing publishing to your modpack, if you");
    println!("don't you simply click on \"No i don't\" which usually opens the page to configure");
    println!("one, when you do make sure you select {SCOPES_NEEDED}");
    println!();
}

fn open_browser(url: &str) -> Result<()> {
    let spawned = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .is_ok()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn().is_ok()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn().is_ok()
    };
    if spawned {
        Ok(())
    } else {
        bail!("could not open a browser, go to {url} yourself")
    }
}

fn ask_token_first_time(explained: &mut bool) -> Result<String> {
    if !*explained {
        explain_pat();
        *explained = true;
    }
    loop {
        let menu = vec!["I have one".to_string(), "No i don't".to_string()];
        let picked = inquire::Select::new("Do you have a Modrinth Token?", menu.clone())
            .with_starting_cursor(0)
            .prompt()
            .map_err(ask_failed)?;
        if picked == menu[1] {
            println!();
            println!(
                "Go make a token at {PAT_PAGE}, select {SCOPES_NEEDED} and then select \"I have one\" when you are done"
            );
            println!();
            ask("press enter to open")?;
            let _ = open_browser(PAT_PAGE);
            println!("opened {PAT_PAGE}");
            println!();
            continue;
        }
        return inquire::Password::new(
            "Type your Modrinth token here (usually after you generated the token below your name): ",
        )
        .prompt()
        .map_err(ask_failed);
    }
}

fn resolve_token(cli: &Cli, given: Option<&str>, explained: &mut bool) -> Result<String> {
    if let Some(t) = given {
        return Ok(t.trim().to_string());
    }
    if secret::has_secret() {
        match secret::load() {
            Ok(Some(t)) => {
                if !crate::modrinth::token_looks_wrong(&t) {
                    detail(cli, "logging in to your account");
                    return Ok(t);
                }
                detail(
                    cli,
                    "the saved token is not a modrinth token, throwing it away",
                );
                forget_secret(cli);
            }
            Ok(None) => {}
            Err(e) => {
                detail(cli, &e.to_string());
                detail(
                    cli,
                    "that saved token CAN NOT be recovered, deleting...",
                );
                forget_secret(cli);
            }
        }
    }
    loop {
        let token = ask_token_first_time(explained)?.trim().to_string();
        if token.is_empty() {
            bail!("this is not a token");
        }
        if crate::modrinth::token_looks_wrong(&token) {
            detail(
                cli,
                "That isn't the right token, modrinth only accepts: mrp_, mra_ or mro_!",
            );
            continue;
        }
        detail(cli, "saving your token");
        secret::save(&token)?;
        return Ok(token);
    }
}

fn forget_secret(cli: &Cli) {
    if let Err(e) = secret::clear() {
        detail(cli, &format!("could not throw the old token away: {e}"));
    }
}

fn resolve_project_id(cli: &Cli, given: Option<&str>, p: &Project) -> Result<String> {
    if let Some(id) = given {
        return Ok(id.trim().to_string());
    }
    if let Some(id) = p
        .manifest
        .modrinth_project_id
        .clone()
        .filter(|s| !s.trim().is_empty())
    {
        detail(cli, &format!("using modrinthProjectId {id} from {MANIFEST_FILE}"));
        return Ok(id);
    }
    let id = ask("what is the modrinth project id? (in your project click the 3 dots and click \"Copy ID\")")?
        .trim()
        .to_string();
    if id.is_empty() {
        bail!("no project id, nothing was published");
    }
    let mut pack = p.manifest.clone();
    pack.modrinth_project_id = Some(id.clone());
    pack.write(&p.dir)?;
    detail(cli, &format!("saved modrinthProjectId {id} to {MANIFEST_FILE}"));
    Ok(id)
}

struct Release {
    minecraft: String,
    files: Vec<(PathBuf, String)>,
    loaders: Vec<String>,
}

fn group_by_mc(targets: &[LockedTarget]) -> Vec<(String, Vec<&LockedTarget>)> {
    let mut out: Vec<(String, Vec<&LockedTarget>)> = Vec::new();
    for t in targets {
        match out.iter_mut().find(|(k, _)| *k == t.minecraft) {
            Some(slot) => slot.1.push(t),
            None => out.push((t.minecraft.clone(), vec![t])),
        }
    }
    out
}

fn env_for(p: &Project) -> Option<String> {
    match (p.manifest.client, p.manifest.server) {
        (true, true) => Some("client_and_server".to_string()),
        (true, false) => Some("client_only".to_string()),
        (false, true) => Some("server_only".to_string()),
        (false, false) => None,
    }
}

async fn publish(
    cli: &Cli,
    api: &str,
    token_arg: Option<&str>,
    project_arg: Option<&str>,
    out: &Path,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
) -> Result<()> {
    let p = load_project(cli)?;
    let version = p.manifest.version.trim().to_string();
    if version.is_empty() {
        bail!("no version to publish, set \"version\" in {MANIFEST_FILE}");
    }
    let top = p
        .manifest
        .top
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();

    step(cli, 1, 5, "locking every mod to a version");
    if !Lock::exists(&p.dir) {
        lock_cmd(cli, api, None, None, false).await?;
    }
    let mut lock = Lock::load(&p.dir)?;
    if lock.manifest_hash != p.manifest.fingerprint() {
        detail(cli, &format!("{MANIFEST_FILE} changed, locking again"));
        lock_cmd(cli, api, None, None, false).await?;
        lock = Lock::load(&p.dir)?;
    }
    if let Some(want) = mc {
        let only = manifest::pick(&p.softwares.targets, Some(want), loader)?;
        lock.targets.retain(|t| t.minecraft == only.minecraft && t.loader.kind == only.loader.kind);
    } else if let Some(l) = loader {
        lock.targets.retain(|t| t.loader.kind == l.as_str());
    }
    if lock.targets.is_empty() {
        bail!("no target left to publish");
    }

    step(cli, 2, 5, "building the mrpacks");
    let sources = override_sources(&p);
    let groups = group_by_mc(&lock.targets);
    let mut releases: Vec<Release> = Vec::new();
    for (mc_ver, targets) in &groups {
        for t in targets {
            detail(
                cli,
                &format!("building for {}, {}", t.minecraft, t.loader.kind),
            );
            let built = build::build(&lock, t, out, &sources)?;
            let name = built
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{}.mrpack", p.manifest.name));
            let size = std::fs::metadata(&built).map(|m| m.len()).unwrap_or(0);
            detail(cli, &format!("  {name} ({size} bytes)"));
            let slot = match releases.iter_mut().find(|r| &r.minecraft == mc_ver) {
                Some(r) => r,
                None => {
                    releases.push(Release {
                        minecraft: mc_ver.clone(),
                        files: Vec::new(),
                        loaders: Vec::new(),
                    });
                    releases.last_mut().expect("just pushed")
                }
            };
            if !slot.loaders.contains(&t.loader.kind) {
                slot.loaders.push(t.loader.kind.clone());
            }
            slot.files.push((built, name));
        }
    }
    if releases.is_empty() {
        bail!("nothing was built");
    }

    step(cli, 2, 4, "checking your token");
    let client = Modrinth::new(api)?;
    let mut explained = false;
    let mut token = resolve_token(cli, token_arg, &mut explained)?;
    if crate::modrinth::token_looks_wrong(&token) {
        bail!("That isn't the right token, modrinth only accepts: mrp_, mra_ or mro_!");
    }
    match client.whoami(&token).await {
        Ok(Ok(u)) => say(cli, &format!("signed in as {u}")),
        _ => detail(cli, "i couldn't read who you are, is Read User Data allowed?"),
    }

    let project_id = resolve_project_id(cli, project_arg, &p)?;

    step(cli, 3, 4, "checking the project");
    if !client.project_exists(&token, &project_id).await? {
        bail!("Sorry, this project no longer exists, either it was deleted privated or simply gone");
    }
    detail(cli, &format!("{project_id} is there"));

    let existing = client.project_versions(&token, &project_id).await?;
    let old_featured: Vec<String> = existing
        .iter()
        .filter(|v| v.featured)
        .map(|v| v.version_number.clone())
        .collect();
    let environment = env_for(&p);
    let changelog = latest_changelog(&p.dir);

    println!();
    say(cli, "about to publish");
    for r in &releases {
        let many = r.loaders.len() > 1;
        let id = if many {
            r.loaders
                .iter()
                .map(|l| format!("v{version}-{}-{}", r.minecraft, l))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            format!("v{version}-{}", r.minecraft)
        };
        let star = if r.minecraft == top { "  (featured)" } else { "" };
        println!("  {}  {}{star}", id, r.loaders.join(", "));
        for (_, n) in &r.files {
            println!("      {n}");
        }
    }
    if !changelog.is_empty() {
        println!("  changelog     {} lines", changelog.lines().count());
    }
    for v in &old_featured {
        println!("  unfeaturing   {v}");
    }
    println!();

    step(cli, 4, 5, "uploading to modrinth");
    let mut made: Vec<(String, String, bool)> = Vec::new();
    let mut created_ids: Vec<String> = Vec::new();
    let mut made_featured = false;
    let total = releases.len();
    for (i, r) in releases.iter().enumerate() {
        let many = r.loaders.len() > 1;
        let version_number = if many {
            format!("v{version}-{}-{}", r.minecraft, r.loaders[0])
        } else {
            format!("v{version}-{}", r.minecraft)
        };
        let want_featured = !made_featured && (r.minecraft == top || top.is_empty());
        if want_featured {
            made_featured = true;
        }
        let body = CreateVersion {
            file_parts: (0..r.files.len()).map(|n| format!("file{n}")).collect(),
            project_id: project_id.clone(),
            name: format!("{} {}", p.manifest.name, version_number),
            version_number: version_number.clone(),
            changelog: changelog.clone(),
            dependencies: Vec::new(),
            game_versions: vec![r.minecraft.clone()],
            version_type: "release".to_string(),
            loaders: r.loaders.clone(),
            environment: environment.clone(),
            featured: want_featured,
        };
        detail(
            cli,
            &format!("[{}/{}] uploading {}", i + 1, total, version_number),
        );
        for (_, n) in &r.files {
            detail(cli, &format!("        {n}"));
        }
        let created = loop {
            match client.create_version(&token, &body, &r.files).await? {
                Ok(v) => break v,
                Err(AuthFailure::BadScopes) => bail!("{BAD_SCOPES}"),
                Err(AuthFailure::Invalid(why)) => {
                    warn(cli, &crate::modrinth::with_details(BAD_TOKEN, &why));
                    if token_arg.is_some() {
                        bail!("the token you passed is not working, so nothing was done");
                    }
                }
                Err(AuthFailure::Expired(why)) => {
                    warn(cli, &crate::modrinth::with_details(EXPIRED, &why));
                    if token_arg.is_some() {
                        bail!("the token you passed is not working, so nothing was done");
                    }
                }
                Err(AuthFailure::Other(e))
                    if e.contains("not found") || e.contains("404") =>
                {
                    bail!(
                        "Sorry, you don't have permissions over {project_id} to publish it, did they gave you the exact permissions?"
                    );
                }
                Err(AuthFailure::Other(e)) => bail!("{}", modrinth_gave_up(&e)),
            }
            let _ = secret::clear();
            token = ask_token_first_time(&mut explained)?.trim().to_string();
            if token.is_empty() {
                bail!("this is not a token");
            }
            if crate::modrinth::token_looks_wrong(&token) {
                bail!("That isn't the right token, modrinth only accepts: mrp_, mra_ or mro_!");
            }
            detail(cli, "saving your token");
            secret::save(&token)?;
            detail(cli, &format!("trying {} again", version_number));
        };
        detail(
            cli,
            &format!("        created {}", created.id),
        );
        created_ids.push(created.id.clone());
        made.push((
            r.minecraft.clone(),
            format!(
                "https://modrinth.com/project/{project_id}/version/{}",
                created.id
            ),
            want_featured,
        ));
    }

    step(cli, 5, 5, "tidying up the featured version");
    let fresh: Vec<String> = existing
        .iter()
        .filter(|v| !created_ids.contains(&v.id))
        .filter(|v| v.featured)
        .map(|v| v.id.clone())
        .collect();
    let mut unfeatured = 0;
    for id in fresh {
        match client.set_featured(&token, &id, false).await? {
            Ok(()) => {
                detail(cli, "unfeatured an old one");
                unfeatured += 1;
            }
            Err(AuthFailure::BadScopes) => bail!("{BAD_SCOPES}"),
            Err(_) => warn(cli, "could not unfeature an old version"),
        }
    }

    println!();
    say(cli, &format!("published {} version(s):", made.len()));
    println!();
    for (mc_ver, url, feat) in &made {
        if *feat {
            println!("- {mc_ver}: {url} (Featured)");
        } else {
            println!("- {mc_ver}: {url}");
        }
    }
    if unfeatured > 0 {
        println!();
        detail(cli, &format!("{unfeatured} old version(s) unfeatured"));
    }
    Ok(())
}

fn latest_changelog(dir: &Path) -> String {
    let raw = match std::fs::read_to_string(dir.join(crate::manifest::CHANGELOG_FILE)) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    let Ok(data) = raw.parse::<toml_edit::DocumentMut>() else {
        return String::new();
    };
    let table = data.as_table();
    let mut dates: Vec<&str> = table.iter().map(|(k, _)| k).collect();
    dates.sort_by(|a, b| b.cmp(a));
    for date in dates {
        let Some(blocks) = table.get(date).and_then(|v| v.as_array_of_tables()) else {
            continue;
        };
        let collected: Vec<&toml_edit::Table> = blocks.iter().collect();
        for block in collected.into_iter().rev() {
            if let Some(text) = block.get("changelogtext").and_then(|v| v.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    String::new()
}

async fn validate_cmd(cli: &Cli) -> Result<()> {
    let p = load_project(cli)?;
    let lock = if Lock::exists(&p.dir) {
        Lock::load(&p.dir)?
    } else {
        say(cli, "nothing is locked yet, running `ehmodpack lock` first");
        lock_cmd(cli, &cli.api.clone().unwrap_or_else(|| DEFAULT_API.to_string()), None, None, false).await?;
        Lock::load(&p.dir)?
    };
    println!();
    print_report(cli, &validate::check(&lock, &p.manifest, &override_sources(&p)))?;
    check_schema(cli, &p);
    Ok(())
}
fn order_target_key(mc: Option<&str>, loader: Option<LoaderKind>) -> Option<String> {
    match (mc, loader) {
        (None, None) => None,
        (Some(mc), None) => Some(mc.to_string()),
        (mc, Some(loader)) => Some(format!("{}+{}", mc?, loader.as_str())),
    }
}

fn parse_position(raw: &str) -> Result<Option<RpPosition>> {
    let want = raw.trim();
    if want.is_empty() {
        return Ok(None);
    }
    if let Ok(n) = want.parse::<u32>() {
        if n == 0 {
            bail!("a position of 0 is not a thing, 1 is the top of the stack");
        }
        return Ok(Some(RpPosition::At(n)));
    }
    match want.to_ascii_lowercase().as_str() {
        "top" => Ok(Some(RpPosition::Named(manifest::RpNamed::Top))),
        "bottom" => Ok(Some(RpPosition::Named(manifest::RpNamed::Bottom))),
        "off" | "none" => Ok(None),
        other => bail!("{other:?} is not a position, use a number like 1, or top or bottom"),
    }
}

fn order_entries(
    p: &Project,
    target: &Target,
    lock: Option<&Lock>,
) -> Vec<(String, Option<RpPosition>)> {
    let mut out: Vec<(String, Option<RpPosition>)> = Vec::new();
    for pkg in &p.manifest.packages {
        if !matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader) {
            continue;
        }
        let position = pkg.position_for(&target.minecraft, &target.loader.kind);
        if position.is_none() && !pkg.is_active() {
            continue;
        }
        let name = lock
            .and_then(|l| {
                l.targets
                    .iter()
                    .find(|t| {
                        t.minecraft == target.minecraft && t.loader.kind == target.loader.kind
                    })
                    .and_then(|t| {
                        t.packages
                            .iter()
                            .find(|lp| lp.project.eq_ignore_ascii_case(&pkg.key()))
                            .map(|lp| {
                                lp.path.rsplit('/').next().unwrap_or(&lp.path).to_string()
                            })
                    })
            })
            .unwrap_or_else(|| pkg.key());
        out.push((name, position));
    }
    for pack in &p.manifest.external_packs {
        let position = pack.position_for(&target.minecraft, &target.loader.kind);
        if position.is_none() && !pack.is_active() {
            continue;
        }
        out.push((pack.entry(), position));
    }
    for (id, _) in manifest::builtin_packs_for(&target.loader.kind) {
        if p
            .manifest
            .external_packs
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(id))
        {
            continue;
        }
        out.push((id.to_string(), None));
    }
    out.sort_by(|a, b| {
        b.1.unwrap_or_default()
            .rank()
            .cmp(&a.1.unwrap_or_default().rank())
    });
    out
}

fn looks_like_position(raw: &str) -> bool {
    let want = raw.trim();
    if want.is_empty() {
        return false;
    }
    want.parse::<u32>().is_ok()
        || matches!(want.to_ascii_lowercase().as_str(), "top" | "bottom" | "off")
}

fn order_specs(packs: &[String], off: bool) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < packs.len() {
        let token = &packs[i];
        if off {
            out.push((token.trim().to_string(), String::new()));
            i += 1;
        } else if let Some((name, value)) = token.rsplit_once('=') {
            out.push((name.trim().to_string(), value.to_string()));
            i += 1;
        } else if i + 1 < packs.len() && looks_like_position(&packs[i + 1]) {
            out.push((token.trim().to_string(), packs[i + 1].clone()));
            i += 2;
        } else {
            bail!("{token:?} needs a position, like {token}=1 or {token} 1");
        }
    }
    Ok(out)
}

const ORDER_SAVE: &str = "save and exit";
const ORDER_CLEAR: &str = "clear every position on these targets";

fn order_pos_label(position: Option<RpPosition>) -> String {
    match position {
        None => "off".to_string(),
        Some(RpPosition::At(n)) => n.to_string(),
        Some(RpPosition::Named(manifest::RpNamed::Bottom)) => "bottom".to_string(),
        Some(RpPosition::Named(manifest::RpNamed::Top)) => "top".to_string(),
    }
}

#[derive(Debug, Clone)]
struct OrderEntry {
    name: String,
    external: bool,
}

fn locked_pack_name(lock: Option<&Lock>, target: &Target, slug: &str) -> Option<String> {
    lock.and_then(|l| {
        l.targets
            .iter()
            .find(|t| t.minecraft == target.minecraft && t.loader.kind == target.loader.kind)
            .and_then(|t| {
                t.packages
                    .iter()
                    .find(|lp| lp.project.eq_ignore_ascii_case(slug))
                    .map(|lp| lp.path.rsplit('/').next().unwrap_or(&lp.path).to_string())
            })
    })
}

fn order_choices(
    p: &Project,
    targets: &[Target],
    lock: Option<&Lock>,
    on_disk: &[String],
) -> Vec<(String, OrderEntry)> {
    let first = targets.first();
    let mut out: Vec<(String, OrderEntry)> = Vec::new();
    for pkg in &p.manifest.packages {
        if !matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader) {
            continue;
        }
        let resolved = first.and_then(|t| pkg.position_for(&t.minecraft, &t.loader.kind));
        let file = first
            .and_then(|t| locked_pack_name(lock, t, &pkg.key()))
            .unwrap_or_else(|| pkg.key());
        let tag = if file.eq_ignore_ascii_case(&pkg.key()) {
            String::new()
        } else {
            format!("   ({})", pkg.key())
        };
        out.push((
            format!("{:<5} {}{tag}", order_pos_label(resolved), file),
            OrderEntry {
                name: pkg.key(),
                external: false,
            },
        ));
    }
    let loader = first.map(|t| t.loader.kind.clone()).unwrap_or_default();
    let builtins = manifest::builtin_packs_for(&loader);
    let label_of = |id: &str| -> String {
        builtins
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(id))
            .map(|(_, l)| (*l).to_string())
            .unwrap_or_else(|| id.to_string())
    };
    for pack in &p.manifest.external_packs {
        let resolved = first.and_then(|t| pack.position_for(&t.minecraft, &t.loader.kind));
        let shown = if pack.is_builtin() {
            label_of(&pack.name)
        } else {
            pack.name.clone()
        };
        out.push((
            format!(
                "{:<5} {}{}",
                order_pos_label(resolved),
                shown,
                match (pack.is_builtin(), pack.is_active()) {
                    (true, true) => "   (built-in pack)",
                    (true, false) => "   (built-in pack, off)",
                    (false, true) => "",
                    (false, false) => "   (off)",
                }
            ),
            OrderEntry {
                name: pack.name.clone(),
                external: true,
            },
        ));
    }
    for (id, label) in &builtins {
        if p
            .manifest
            .external_packs
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(id))
        {
            continue;
        }
        out.push((
            format!("{:<5} {label}   (built-in pack)", order_pos_label(None)),
            OrderEntry {
                name: id.to_string(),
                external: true,
            },
        ));
    }
    for file in on_disk {
        if p
            .manifest
            .external_packs
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(file))
        {
            continue;
        }
        out.push((
            format!("{:<5} {file}   (in the folder, not in the manifest)", order_pos_label(None)),
            OrderEntry {
                name: file.clone(),
                external: true,
            },
        ));
    }
    out
}

fn set_order_on(
    p: &mut Project,
    entry: &OrderEntry,
    scope: &[Option<String>],
    targets: &[Target],
    position: Option<RpPosition>,
    cli: &Cli,
) {
    if !entry.external {
        for pkg in p.manifest.packages.iter_mut() {
            if !pkg.key().eq_ignore_ascii_case(&entry.name) {
                continue;
            }
            if position.is_some() {
                pkg.active = Some(true);
            }
            set_positions(&mut pkg.position, &mut pkg.positions, scope, position);
        }
    } else {
        let found = p
            .manifest
            .external_packs
            .iter_mut()
            .find(|e| e.name.eq_ignore_ascii_case(&entry.name));
        match found {
            Some(pack) => {
                if position.is_some() {
                    pack.active = Some(true);
                }
                set_positions(&mut pack.position, &mut pack.positions, scope, position);
            }
            None => {
                let loader = targets
                    .first()
                    .map(|t| t.loader.kind.clone())
                    .unwrap_or_default();
                let builtin = manifest::builtin_packs_for(&loader)
                    .iter()
                    .any(|(id, _)| id.eq_ignore_ascii_case(&entry.name));
                let mut pack = manifest::ExternalPack {
                    name: entry.name.clone(),
                    active: Some(position.is_some()),
                    builtin: if builtin { Some(true) } else { None },
                    position: None,
                    positions: BTreeMap::new(),
                };
                set_positions(&mut pack.position, &mut pack.positions, scope, position);
                p.manifest.external_packs.push(pack);
            }
        }
    }
    let where_ = if scope.iter().any(|k| k.is_none()) {
        "every target".to_string()
    } else {
        targets
            .iter()
            .map(Targetish::label)
            .collect::<Vec<_>>()
            .join(", ")
    };
    say(
        cli,
        &format!("{} is {} on {}", entry.name, order_pos_label(position), where_),
    );
}

fn order_interactive(cli: &Cli, mc: Option<&str>, loader: Option<LoaderKind>) -> Result<()> {
    let mut p = load_project(cli)?;
    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };
    let on_disk = external_pack_names(&p);

    let labels: Vec<String> = p.softwares.targets.iter().map(Targetish::label).collect();
    let (targets, scope): (Vec<Target>, Vec<Option<String>>) = if mc.is_some() || loader.is_some() {
        let t = manifest::pick(&p.softwares.targets, mc, loader)?.clone();
        let k = Some(order_target_key(mc, loader).unwrap_or_default());
        (vec![t], vec![k])
    } else {
        let options: Vec<String> = labels
            .iter()
            .cloned()
            .chain(std::iter::once("all of them".to_string()))
            .collect();
        let chosen: Vec<Target> = loop {
            let picked = inquire::MultiSelect::new(
                &format!("{} has {} targets, which ones?", MANIFEST_FILE, labels.len()),
                options.clone(),
            )
            .with_help_message("space ticks a target, enter confirms, right arrow picks everything")
            .prompt()
            .map_err(ask_failed)?;
            if picked.iter().any(|o| o == "all of them") {
                break p.softwares.targets.clone();
            }
            let chosen: Vec<Target> = labels
                .iter()
                .enumerate()
                .filter(|(_, l)| picked.iter().any(|x| x == *l))
                .filter_map(|(i, _)| p.softwares.targets.get(i).cloned())
                .collect();
            if !chosen.is_empty() {
                break chosen;
            }
            detail(
                cli,
                "nothing ticked yet, press space on a target then enter, or tick all of them",
            );
        };
        let keys: Vec<Option<String>> = if chosen.len() == p.softwares.targets.len() {
            vec![None]
        } else {
            chosen
                .iter()
                .map(|t| Some(format!("{}+{}", t.minecraft, t.loader.kind)))
                .collect()
        };
        (chosen, keys)
    };

    let scope_label = if scope.iter().any(|k| k.is_none()) {
        "every target".to_string()
    } else {
        targets
            .iter()
            .map(Targetish::label)
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!();
    say(
        cli,
        &format!("ordering resource packs for {scope_label}, 1 is the top of the stack"),
    );
    for target in &targets {
        say(cli, &format!("right now, {}", Targetish::label(target)));
        let entries = order_entries(&p, target, lock.as_ref());
        if entries.is_empty() {
            detail(cli, "nothing is active here yet");
        } else {
            let rows: Vec<Vec<String>> = entries
                .iter()
                .map(|(name, position)| vec![order_pos_label(*position), name.clone()])
                .collect();
            println!(
                "{}",
                table(cli, &["top", "pack"], &rows, &vec![false; rows.len()])
            );
        }
    }
    println!();

    let positions: Vec<String> = ["off".to_string(), "top".to_string(), "bottom".to_string()]
        .into_iter()
        .chain((1..=20).map(|n| n.to_string()))
        .collect();

    loop {
        let choices = order_choices(&p, &targets, lock.as_ref(), &on_disk);
        let mut menu: Vec<String> = choices.iter().map(|(label, _)| label.clone()).collect();
        menu.push(ORDER_CLEAR.to_string());
        menu.push(ORDER_SAVE.to_string());

        let picked = inquire::Select::new("pick a pack to move", menu.clone())
            .with_page_size(16)
            .with_help_message("enter picks it, the number on the left is where it sits now")
            .prompt()
            .map_err(ask_failed)?;
        let Some(i) = menu.iter().position(|o| *o == picked) else {
            continue;
        };

        if picked == ORDER_SAVE {
            break;
        }
        if picked == ORDER_CLEAR {
            let yes = inquire::Confirm::new(&format!("clear every position on {scope_label}?"))
                .with_default(false)
                .prompt()
                .map_err(ask_failed)?;
            if yes {
                let entries: Vec<OrderEntry> = order_choices(&p, &targets, lock.as_ref(), &on_disk)
                    .into_iter()
                    .map(|(_, e)| e)
                    .collect();
                for entry in entries {
                    set_order_on(&mut p, &entry, &scope, &targets, None, cli);
                }
            }
            continue;
        }

        let entry = match choices.get(i) {
            Some((_, e)) => e.clone(),
            None => continue,
        };
        let label = format!("where does {} sit", entry.name);
        let where_ = inquire::Select::new(&label, positions.clone())
            .prompt()
            .map_err(ask_failed)?;
        let position = parse_position(&where_)?;
        set_order_on(&mut p, &entry, &scope, &targets, position, cli);
    }

    p.manifest.write(&p.dir)?;
    say(cli, &format!("wrote {MANIFEST_FILE}"));
    for target in &targets {
        let label = Targetish::label(target);
        println!();
        say(cli, &format!("resource pack order for {label}"));
        let entries = order_entries(&p, target, lock.as_ref());
        if entries.is_empty() {
            detail(cli, "nothing is active here");
            continue;
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|(name, position)| vec![order_pos_label(*position), name.clone()])
            .collect();
        println!(
            "{}",
            table(cli, &["top", "pack"], &rows, &vec![false; rows.len()])
        );
    }
    println!();
    detail(cli, "run `ehmodpack lock` then `ehmodpack build` to see it in the pack");
    Ok(())
}

fn order_cmd(
    cli: &Cli,
    mc: Option<&str>,
    loader: Option<LoaderKind>,
    off: bool,
    packs: Vec<String>,
) -> Result<()> {
    if packs.is_empty() && !off {
        return order_interactive(cli, mc, loader);
    }
    let mut p = load_project(cli)?;
    let key = order_target_key(mc, loader);
    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };
    let targets: Vec<Target> = if key.is_some() {
        vec![manifest::pick(&p.softwares.targets, mc, loader)?.clone()]
    } else {
        p.softwares.targets.clone()
    };

    let on_disk = external_pack_names(&p);

    for (name, raw) in order_specs(&packs, off)? {
        let position = parse_position(&raw)?;
        if position.is_some() && off {
            bail!("--off does not take a position, just the pack name");
        }
        if !off && position.is_none() {
            bail!("{name:?} needs a position, like {name}=1, or --off to clear it");
        }

        let mut hit = false;
        for pkg in p.manifest.packages.iter_mut() {
            let file = lock.as_ref().and_then(|l| {
                targets.iter().find_map(|t| {
                    l.targets
                        .iter()
                        .find(|lt| {
                            lt.minecraft == t.minecraft && lt.loader.kind == t.loader.kind
                        })
                        .and_then(|lt| {
                            lt.packages
                                .iter()
                                .find(|lp| lp.project.eq_ignore_ascii_case(&pkg.key()))
                                .map(|lp| lp.path.rsplit('/').next().unwrap_or(&lp.path).to_string())
                        })
                })
            });
            let known = pkg.key().eq_ignore_ascii_case(&name)
                || file.as_deref() == Some(name.as_str());
            if !known || !matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader) {
                continue;
            }
            if position.is_some() && !pkg.is_active() {
                pkg.active = Some(true);
                say(cli, &format!("{} is active now", pkg.key()));
            }
            set_position(&mut pkg.position, &mut pkg.positions, &key, position);
            say(
                cli,
                &format!(
                    "{} is {}",
                    pkg.key(),
                    match &key {
                        Some(k) => format!("at {} on {k}", show_position(position)),
                        None => format!("{} everywhere", show_position(position)),
                    }
                ),
            );
            hit = true;
        }

        if !hit {
            if let Some(pack) = p
                .manifest
                .external_packs
                .iter_mut()
                .find(|e| e.name.eq_ignore_ascii_case(&name))
            {
                if position.is_some() && !pack.is_active() {
                    pack.active = Some(true);
                    say(cli, &format!("{} is active now", pack.name));
                }
                set_position(&mut pack.position, &mut pack.positions, &key, position);
                say(
                    cli,
                    &format!(
                        "{} is {}",
                        pack.name,
                        match &key {
                            Some(k) => format!("at {} on {k}", show_position(position)),
                            None => format!("{} everywhere", show_position(position)),
                        }
                    ),
                );
                hit = true;
            }
        }

        if !hit && !off && on_disk.iter().any(|f| f.eq_ignore_ascii_case(&name)) {
                let mut pack = manifest::ExternalPack {
                    name: name.clone(),
                    active: Some(position.is_some()),
                    builtin: None,
                    position: None,
                    positions: BTreeMap::new(),
                };
            set_position(&mut pack.position, &mut pack.positions, &key, position);
            p.manifest.external_packs.push(pack);
            say(
                cli,
                &format!("added {name} to external_packs, it lives in the overrides folder"),
            );
            hit = true;
        }

        if !hit {
            let mut known: Vec<String> = p
                .manifest
                .packages
                .iter()
                .filter(|x| matches!(x.kind, PkgType::Resourcepack | PkgType::Shader))
                .map(|x| x.key())
                .collect();
            known.extend(p.manifest.external_packs.iter().map(|x| x.name.clone()));
            known.extend(on_disk.iter().cloned());
            let mut seen = BTreeSet::new();
            known.retain(|k| seen.insert(k.to_ascii_lowercase()));
            bail!("{name:?} is not a resource pack here, try one of: {}", known.join(", "));
        }
    }

    p.manifest.write(&p.dir)?;
    if !packs.is_empty() {
        say(cli, &format!("wrote {MANIFEST_FILE}"));
    detail(
        cli,
        "run ehmodpack lock to refresh the lock and ehmodpack build to apply any changes",
    );
    }

    for target in &targets {
        let label = Targetish::label(target);
        println!();
        say(cli, &format!("resource pack order for {label}"));
        let entries = order_entries(&p, target, lock.as_ref());
        if entries.is_empty() {
            detail(cli, "nothing is active here");
            continue;
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|(name, position)| {
                vec![show_position(*position), name.clone()]
            })
            .collect();
        println!(
            "{}",
            table(cli, &["top", "pack"], &rows, &vec![false; rows.len()])
        );
    }
    println!();
    Ok(())
}

fn show_position(position: Option<RpPosition>) -> String {
    match position {
        None => "off".to_string(),
        Some(RpPosition::At(n)) => n.to_string(),
        Some(RpPosition::Named(manifest::RpNamed::Bottom)) => "bottom".to_string(),
        Some(RpPosition::Named(manifest::RpNamed::Top)) => "top".to_string(),
    }
}

fn set_position(
    position: &mut Option<RpPosition>,
    positions: &mut BTreeMap<String, RpPosition>,
    key: &Option<String>,
    value: Option<RpPosition>,
) {
    for key in std::slice::from_ref(key) {
        match (key, value) {
            (Some(k), Some(v)) => {
                positions.insert(k.clone(), v);
            }
            (Some(k), None) => {
                positions.remove(k);
            }
            (None, Some(v)) => *position = Some(v),
            (None, None) => *position = None,
        }
    }
}

fn set_positions(
    position: &mut Option<RpPosition>,
    positions: &mut BTreeMap<String, RpPosition>,
    scope: &[Option<String>],
    value: Option<RpPosition>,
) {
    for key in scope {
        match (key, value) {
            (Some(k), Some(v)) => {
                positions.insert(k.clone(), v);
            }
            (Some(k), None) => {
                positions.remove(k);
            }
            (None, Some(v)) => *position = Some(v),
            (None, None) => *position = None,
        }
    }
}

fn external_pack_names(p: &Project) -> Vec<String> {
    let mut out = Vec::new();
    for name in [
        manifest::DEFAULT_OVERRIDES,
        manifest::DEFAULT_CLIENT_OVERRIDES,
    ] {
        let dir = p.dir.join(name).join("resourcepacks");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(file) = entry.path().file_name().and_then(|f| f.to_str()) {
                out.push(file.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn check_schema(cli: &Cli, p: &Project) {
    let lock_version = if Lock::exists(&p.dir) {
        Lock::load(&p.dir).map(|l| l.schema_version).unwrap_or(0)
    } else {
        0
    };
    let have = p
        .manifest
        .schema_version
        .max(p.softwares.schema_version)
        .max(lock_version);
    if have < manifest::SCHEMA_VERSION {
        warn(cli, manifest::SCHEMA_STALE);
    }
}

async fn set_release(cli: &Cli, api: &str, version: &str) -> Result<()> {
    let want = version.trim();
    if want.is_empty() {
        bail!("the version cannot be empty");
    }
    let mut p = load_project(cli)?;
    let current = p.manifest.version.trim().to_string();
    if current == want {
        say(cli, &format!("{MANIFEST_FILE} is already on {want}"));
        return Ok(());
    }
    p.manifest.version = want.to_string();
    p.manifest.stamp();
    p.manifest.write(&p.dir)?;
    say(cli, &format!("{MANIFEST_FILE} is now on {want}"));
    lock_cmd(cli, api, None, None, false).await?;
    say(cli, &format!("{LOCK_FILE} refreshed"));
    Ok(())
}

fn update_schema(cli: &Cli) -> Result<()> {
    let mut p = load_project(cli)?;
    p.manifest.stamp();
    p.softwares.stamp();
    p.manifest.write(&p.dir)?;
    p.softwares.write(&p.dir)?;
    if Lock::exists(&p.dir) {
        let mut lock = Lock::load(&p.dir)?;
        lock.schema = Some(manifest::lock_schema_url());
        lock.schema_version = manifest::SCHEMA_VERSION;
        lock.write(&p.dir)?;
        say(cli, &format!("{LOCK_FILE} is on schema v{}", manifest::SCHEMA_VERSION));
    }
    let written = manifest::write_local_schemas(&p.dir)?;
    say(
        cli,
        &format!(
            "{} and {SOFTWARES_FILE} are on schema v{}",
            MANIFEST_FILE,
            manifest::SCHEMA_VERSION
        ),
    );
    for path in written {
        println!("  {}", path.display());
    }
    Ok(())
}

fn color_enabled(cli: &Cli) -> bool {
    if cli.no_color || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
}

fn step(cli: &Cli, n: usize, total: usize, text: &str) {
    if !color_enabled(cli) {
        println!("[{n}/{total}] {text}");
    } else {
        println!("{}", format!("[{n}/{total}] {text}").cyan().bold());
    }
}

fn detail(cli: &Cli, text: &str) {
    if !color_enabled(cli) {
        println!("  {text}");
    } else {
        println!("  {}", text.dimmed());
    }
}

fn notice(cli: &Cli, text: &str) {
    if !color_enabled(cli) {
        println!("{text}");
    } else {
        println!("{}", text.yellow());
    }
}

fn say(cli: &Cli, text: &str) {
    if !color_enabled(cli) {
        println!("{text}");
    } else {
        println!("{}", text.bold().green());
    }
}

fn grey(cli: &Cli, text: &str) -> String {
    if !color_enabled(cli) {
        text.to_string()
    } else {
        text.bright_black().to_string()
    }
}

fn warn(cli: &Cli, text: &str) {
    if !color_enabled(cli) {
        eprintln!("{text}");
    } else {
        eprintln!("{}", text.yellow());
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn render_row(cells: &[String], widths: &[usize]) -> String {
    let mut line = String::new();
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            line.push_str("  ");
        }
        line.push_str(cell);
        if i + 1 < cells.len() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            line.push_str(&" ".repeat(pad));
        }
    }
    line.trim_end().to_string()
}

fn table(cli: &Cli, headers: &[&str], rows: &[Vec<String>], dimmed: &[bool]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let head: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    let mut out = String::new();
    out.push_str(&render_row(&head, &widths));
    out.push('\n');
    out.push_str(
        &widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  "),
    );
    for (i, row) in rows.iter().enumerate() {
        out.push('\n');
        let line = render_row(row, &widths);
        if dimmed.get(i).copied().unwrap_or(false) && color_enabled(cli) {
            out.push_str(&format!("{}", line.dimmed()));
        } else {
            out.push_str(&line);
        }
    }
    out
}

#[cfg(test)]
mod order_tests {
    use super::{
        order_specs, parse_position, set_positions, looks_like_position, RpPosition,
    };
    use crate::manifest::RpNamed;
    use std::collections::BTreeMap;

    #[test]
    fn a_position_lands_on_every_picked_target() {
        let mut position = None;
        let mut positions = BTreeMap::new();
        let scope = vec![
            Some("1.21.10+fabric".to_string()),
            Some("1.21.9+fabric".to_string()),
        ];
        set_positions(
            &mut position,
            &mut positions,
            &scope,
            Some(RpPosition::At(3)),
        );
        assert_eq!(
            positions.get("1.21.10+fabric"),
            Some(&RpPosition::At(3))
        );
        assert_eq!(positions.get("1.21.9+fabric"), Some(&RpPosition::At(3)));
        assert_eq!(positions.len(), 2);
        assert!(position.is_none(), "the plain one stays untouched");
    }

    #[test]
    fn picking_every_target_writes_the_plain_position() {
        let mut position = None;
        let mut positions = BTreeMap::new();
        set_positions(
            &mut position,
            &mut positions,
            &[None],
            Some(RpPosition::At(2)),
        );
        assert_eq!(position, Some(RpPosition::At(2)));
        assert!(positions.is_empty());
    }

    #[test]
    fn clearing_removes_every_picked_target() {
        let mut position = Some(RpPosition::At(9));
        let mut positions = BTreeMap::new();
        let scope = vec![
            Some("1.21.10+fabric".to_string()),
            Some("1.21.9+fabric".to_string()),
        ];
        positions.insert("1.21.10+fabric".to_string(), RpPosition::At(3));
        positions.insert("1.21.9+fabric".to_string(), RpPosition::At(3));
        set_positions(&mut position, &mut positions, &scope, None);
        assert!(positions.is_empty(), "{positions:?}");
        assert_eq!(position, Some(RpPosition::At(9)), "the plain one stays");
    }

    #[test]
    fn off_is_a_position_not_a_mistake() {
        assert_eq!(parse_position("off").unwrap(), None);
        assert_eq!(parse_position("").unwrap(), None);
    }

    #[test]
    fn the_named_and_number_forms_both_read() {
        assert_eq!(
            parse_position("top").unwrap(),
            Some(RpPosition::Named(RpNamed::Top))
        );
        assert_eq!(
            parse_position("bottom").unwrap(),
            Some(RpPosition::Named(RpNamed::Bottom))
        );
        assert_eq!(parse_position("4").unwrap(), Some(RpPosition::At(4)));
    }

    #[test]
    fn a_bad_position_is_refused() {
        assert!(parse_position("0").is_err());
        assert!(parse_position("up there").is_err());
    }

    #[test]
    fn both_argument_forms_pair_up() {
        let joined = vec!["Ginkgo Font.zip=1".to_string(), "icons=2".to_string()];
        assert_eq!(
            order_specs(&joined, false).unwrap(),
            vec![
                ("Ginkgo Font.zip".to_string(), "1".to_string()),
                ("icons".to_string(), "2".to_string())
            ]
        );
        let spaced = vec![
            "Ginkgo Font.zip".to_string(),
            "1".to_string(),
            "icons".to_string(),
            "2".to_string(),
        ];
        assert_eq!(order_specs(&spaced, false).unwrap(), order_specs(&joined, false).unwrap());
    }

    #[test]
    fn a_name_with_no_position_is_refused() {
        assert!(order_specs(&["Ginkgo Font.zip".to_string()], false).is_err());
    }

    #[test]
    fn off_takes_a_bare_name() {
        let bare = vec!["Ginkgo Font.zip".to_string()];
        assert_eq!(
            order_specs(&bare, true).unwrap(),
            vec![("Ginkgo Font.zip".to_string(), String::new())]
        );
    }

    #[test]
    fn a_name_never_reads_as_a_position() {
        assert!(looks_like_position("1"));
        assert!(looks_like_position("off"));
        assert!(!looks_like_position("Ginkgo Font.zip"));
    }
}

#[cfg(test)]
mod version_conflict_tests {
    use crate::modmeta::{game_mc_ok, shares_mc_minor};

    #[test]
    fn the_same_minor_line_is_always_ok() {
        assert!(shares_mc_minor("1.21.10", "1.21.9"));
        assert!(shares_mc_minor("1.21.10", "1.21"));
        assert!(shares_mc_minor("1.21.10", "1.21.10"));
    }

    #[test]
    fn a_different_minor_line_is_not_ok() {
        assert!(!shares_mc_minor("1.21.10", "1.20.6"));
        assert!(!shares_mc_minor("1.21.10", "1.22.0"));
    }

    #[test]
    fn an_exact_tagged_version_wins() {
        let listed = vec!["1.21.10".to_string()];
        assert!(game_mc_ok("1.21.10", &listed));
    }

    #[test]
    fn made_for_the_preceding_patch_still_passes() {
        let listed = vec!["1.21.9".to_string(), "1.21.11".to_string()];
        assert!(game_mc_ok("1.21.10", &listed), "1.21.x builds work across the line");
    }

    #[test]
    fn made_for_a_totally_different_line_is_a_conflict() {
        let listed = vec!["1.20.6".to_string(), "1.20.7".to_string()];
        assert!(!game_mc_ok("1.21.10", &listed));
    }

    #[test]
    fn an_empty_tag_list_is_not_judged() {
        assert!(game_mc_ok("1.21.10", &[]));
    }
}