use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;

use crate::Launcher;
use crate::account;
use crate::build;
use crate::i18n;
use crate::importer;
use crate::loaders::{self, LoaderKind};
use crate::locale;
use crate::lock::{Lock, LockedPackage, LockedTarget};
use crate::manifest::{
    self, Env, LOCK_FILE, LoaderSpec, MANIFEST_FILE, Manifest, Package, PkgType, RpPosition,
    SOFTWARES_FILE, Softwares, Support, Target, Targetish,
};
use crate::minecraft;
use crate::modmeta;
use crate::modrinth::{
    AuthFailure, Category, CreateVersion, DEFAULT_API, Disclosure, Environment, Modrinth, Resolved,
    SearchFilters, SearchHit, Sort, is_modrinth_id,
};
use crate::scaffold::{self, NewPlan};
use crate::secret;
use crate::staging;
use crate::validate;
use crate::{Cli, Cmd, WorkflowKind};
use futures_util::StreamExt;

pub async fn dispatch(cli: Cli) -> Result<()> {
    let api = cli.api.clone().unwrap_or_else(|| DEFAULT_API.to_string());
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
            let sort = sort.or(if browsing {
                Some(Sort::Downloads)
            } else {
                None
            });
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
        Cmd::Java => java_cmd(&cli),
        Cmd::List { mc, loader } => list(&cli, mc.as_deref(), *loader),
        Cmd::Lock { mc, loader, check } => {
            lock_cmd(&cli, &api, mc.as_deref(), *loader, *check).await
        }
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
        } => {
            update_cmd(
                &cli,
                &api,
                mc.as_deref(),
                *loader,
                *dry_run,
                *update_changelogs,
            )
            .await
        }
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
        } => {
            publish(
                &cli,
                &api,
                token.as_deref(),
                project_id.as_deref(),
                out,
                mc.as_deref(),
                *loader,
            )
            .await
        }
        Cmd::Locale { action } => match action {
            crate::LocaleAction::List => {
                locale::list();
                Ok(())
            }
            crate::LocaleAction::Set { locale } => locale::set(locale),
        },
        Cmd::Validate => validate_cmd(&cli).await,
        Cmd::Account {
            login,
            logout,
            show,
            client_id,
        } => account_cmd(&cli, *login, *logout, *show, client_id.as_deref()).await,
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
            install_java_if_no_there,
            vendor,
            java_path,
            msa,
            msa_login,
            client_id,
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
                *install_java_if_no_there,
                vendor.as_deref(),
                java_path.as_deref(),
                *msa,
                *msa_login,
                client_id.as_deref(),
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
    let softwares = Softwares::load(&dir)?.context(i18n::t("load-no-softwares"))?;
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

async fn resolve_one(client: &Modrinth, target: &Target, pkg: &Package) -> Result<Resolved> {
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
        detail_note(&i18n::t_args(
            "resolve-no-pinned",
            &[("key", key.as_str()), ("last", last.as_str())],
        ));
    }
    let found = client
        .resolve(&key, Some(&target.minecraft), &loaders)
        .await
        .with_context(|| {
            i18n::t_args(
                "resolve-nothing",
                &[("key", key.as_str()), ("target", target.label().as_str())],
            )
        })?;
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
        "{}",
        i18n::t_args(
            "resolve-wrong-kind",
            &[
                ("slug", found.project.slug.as_str()),
                ("kind", found.project.project_type.as_str()),
            ]
        )
    )
}

fn detail_note(text: &str) {
    if crate::progress::interactive() {
        println!("\r  {text}");
    }
}

async fn unsupported_note(client: &Modrinth, key: &str, target: &Target) -> String {
    let what = match client.project(key).await {
        Ok(p) => i18n::t_args(
            "resolve-on-versions",
            &[
                ("loaders", &p.loaders.join("/")),
                (
                    "versions",
                    &if p.game_versions.is_empty() {
                        i18n::t("resolve-no-mc-versions")
                    } else {
                        i18n::t_args(
                            "resolve-versions-count",
                            &[("count", &p.game_versions.len().to_string())],
                        )
                    },
                ),
            ],
        ),
        Err(_) => i18n::t("resolve-nothing-visible"),
    };
    i18n::t_args(
        "resolve-unsupported",
        &[
            ("key", key),
            ("what", what.as_str()),
            ("target", target.label().as_str()),
        ],
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

async fn keep_loader(
    client: &Modrinth,
    hits: Vec<SearchHit>,
    loader: LoaderKind,
) -> Vec<SearchHit> {
    let mut kept = Vec::new();
    for hit in hits {
        match client.project(&hit.slug).await {
            Ok(p)
                if p.loaders
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case(loader.as_str())) =>
            {
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
        return i18n::t("unknown");
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
        "{} ({})",
        head.join(", "),
        i18n::t_args("versions-count", &[("count", &versions.len().to_string())])
    )
}

async fn with_details(client: &Modrinth, hits: &[SearchHit]) -> Vec<(String, String)> {
    let jobs = hits.iter().map(|h| {
        let slug = h.slug.clone();
        async move { client.project(&slug).await }
    });
    let projects: Vec<anyhow::Result<crate::modrinth::Project>> = futures_util::stream::iter(jobs)
        .buffer_unordered(12)
        .collect()
        .await;
    projects
        .into_iter()
        .map(|p| match p {
            Ok(p) => (
                if p.loaders.is_empty() {
                    i18n::t("unknown")
                } else {
                    p.loaders.join(", ")
                },
                summarise_versions(&p.game_versions),
            ),
            Err(_) => (i18n::t("unknown"), i18n::t("unknown")),
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
                i18n::t("nothing-came-back")
            } else {
                i18n::t_args(
                    "search-nothing-matched",
                    &[("query", &format!("{:?}", filters.query))],
                )
            },
        );
        return Ok(());
    }
    let added = added_slugs(cli);
    let details = with_details(&client, &found.hits).await;
    println!("{}", hits_table(cli, &found.hits, &added, &details));
    let limit = filters.limit.clamp(1, 100);
    let pages = found.total_hits.div_ceil(limit);
    println!(
        "\n{}",
        i18n::t_args(
            "search-page-status",
            &[
                ("page", &filters.page.to_string()),
                ("pages", &pages.to_string()),
                ("shown", &found.hits.len().to_string()),
                ("total", &found.total_hits.to_string()),
            ]
        )
    );
    let active = filters.active_lines();
    if !active.is_empty() {
        println!(
            "{}",
            i18n::t_args("filters-line", &[("filters", &active.join(" | "))])
        );
    }
    if filters.page < pages {
        println!(
            "{}",
            i18n::t_args(
                "search-next-cmd",
                &[("page", &(filters.page + 1).to_string())]
            )
        );
    }
    println!("{}", i18n::t("search-info-hint"));
    println!("{}", i18n::t("search-add-hint"));
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
        format!("{name}  {}", i18n::t("marked-on"))
    } else {
        format!("{name}  {}", i18n::t("marked-off"))
    }
}

fn back() -> String {
    i18n::t("option-back")
}

fn with_back(mut options: Vec<String>) -> Vec<String> {
    options.push(back());
    options
}

fn went_back(chosen: &[String]) -> bool {
    chosen.iter().any(|c| c == &back())
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
        say(cli, &i18n::t("category-asking"));
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
            i18n::t("category-menu-only"),
            i18n::t("category-menu-hide"),
            back(),
        ];
        let picked = inquire::Select::new(&i18n::t("category-prompt"), menu.clone())
            .prompt()
            .map_err(ask_failed)?;
        let Some(i) = menu.iter().position(|o| *o == picked) else {
            continue;
        };
        match i {
            0 => {
                let current = filters.categories.clone();
                let options: Vec<String> = names.iter().map(|n| marked(&current, n)).collect();
                let chosen = inquire::MultiSelect::new(
                    &i18n::t("category-only-prompt"),
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
                        items
                            .iter()
                            .any(|c| filters.categories.iter().any(|f| f.ends_with(&c.name)))
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
                let picked = inquire::MultiSelect::new(
                    &i18n::t("category-menu-hide"),
                    with_back(base.clone()),
                )
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
            i18n::t("filters-empty-header")
        } else {
            i18n::t_args("filters-header", &[("filters", &active.join(" | "))])
        };
        let options = vec![
            i18n::t("filter-option-open-source"),
            i18n::t("filter-option-supports"),
            i18n::t("filter-option-disclosure-exclusions"),
            i18n::t("filter-option-only-disclosures"),
            i18n::t("filter-option-minecraft-version"),
            i18n::t("filter-option-loader"),
            i18n::t("filter-option-sorted-by"),
            i18n::t("filter-option-clear"),
            i18n::t("filter-option-back"),
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
                    &i18n::t("filter-option-open-source"),
                    with_back(vec![
                        i18n::t("filter-open-any"),
                        i18n::t("filter-open-only-open"),
                        i18n::t("filter-open-only-closed"),
                    ]),
                )
                .prompt()
                .map_err(ask_failed)?;
                if went_back(std::slice::from_ref(&open)) {
                    continue;
                }
                filters.open_source = if open == i18n::t("filter-open-only-open") {
                    Some(true)
                } else if open == i18n::t("filter-open-only-closed") {
                    Some(false)
                } else {
                    None
                };
            }
            1 => {
                let current: Vec<String> = filters.environments.iter().map(|e| e.label()).collect();
                let options: Vec<String> = Environment::all()
                    .iter()
                    .map(|e| marked(&current, &e.label()))
                    .collect();
                let chosen = inquire::MultiSelect::new(
                    &i18n::t("filter-option-supports"),
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
                    .map(|d| d.label())
                    .collect();
                let options: Vec<String> = Disclosure::all()
                    .iter()
                    .map(|d| marked(&current, &d.label()))
                    .collect();
                let chosen = inquire::MultiSelect::new(
                    &i18n::t("filter-hide-disclosures-prompt"),
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
                    .map(|d| d.label())
                    .collect();
                let options: Vec<String> = Disclosure::all()
                    .iter()
                    .map(|d| marked(&current, &d.label()))
                    .collect();
                let chosen = inquire::MultiSelect::new(
                    &i18n::t("filter-option-only-disclosures"),
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
                let raw = ask(&i18n::t_args(
                    "filter-version-prompt",
                    &[("current", &format!("{current:?}"))],
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
                let options: Vec<String> = tags.iter().map(|t| marked(&current, &t.name)).collect();
                let chosen = inquire::MultiSelect::new(
                    &i18n::t("filter-option-loader"),
                    with_back(options.clone()),
                )
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
                let choices: Vec<(String, Sort)> = vec![
                    (i18n::t("sort-relevance"), Sort::Relevance),
                    (i18n::t("sort-follows"), Sort::Follows),
                    (i18n::t("sort-downloads"), Sort::Downloads),
                    (i18n::t("sort-newest"), Sort::Newest),
                    (i18n::t("sort-updated"), Sort::Updated),
                ];
                let mut options: Vec<String> = choices.iter().map(|(n, _)| n.clone()).collect();
                options.push(back());
                let picked = inquire::Select::new(&i18n::t("filter-option-sorted-by"), options)
                    .prompt()
                    .map_err(ask_failed)?;
                if picked == back() {
                    continue;
                }
                filters.sort = choices.iter().find(|(n, _)| *n == picked).map(|(_, s)| *s);
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
                warn(cli, &i18n::t("filters-cleared"));
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
                i18n::t("search-menu-header")
            } else {
                i18n::t_args(
                    "search-menu-header-filtered",
                    &[("filters", &active.join(" | "))],
                )
            };
            let options = vec![
                i18n::t("search-menu-search"),
                i18n::t("search-menu-category"),
                i18n::t("search-menu-filters"),
                i18n::t("search-menu-quit"),
            ];
            let picked = inquire::Select::new(&header, options.clone())
                .prompt()
                .map_err(ask_failed)?;
            let Some(i) = options.iter().position(|o| *o == picked) else {
                continue;
            };
            match i {
                0 => {
                    let raw = ask(&i18n::t("search-prompt"))?;
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
                warn(
                    cli,
                    &i18n::t_args(
                        "search-nothing-matched",
                        &[("query", &format!("{:?}", filters.query))],
                    ),
                );
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
                        format!(" {}", i18n::t("added-mark"))
                    } else {
                        String::new()
                    };
                    let loaders = details.get(i).map(|(l, _)| l.clone()).unwrap_or_default();
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
                options.push(i18n::t_args(
                    "search-next-page",
                    &[
                        ("next", &(page + 1).to_string()),
                        ("pages", &pages.to_string()),
                    ],
                ));
                navs.push(Nav::Next);
            }
            if page > 1 {
                options.push(i18n::t("search-previous-page"));
                navs.push(Nav::Prev);
            }
            options.push(i18n::t("search-menu-filters"));
            navs.push(Nav::EditFilters);
            options.push(i18n::t("search-menu-category"));
            navs.push(Nav::EditCategory);
            options.push(i18n::t("search-again"));
            navs.push(Nav::NewSearch);
            options.push(i18n::t("search-back-to-start"));
            navs.push(Nav::BackToStart);
            options.push(i18n::t("search-menu-quit"));
            navs.push(Nav::Quit);

            let active = filters.active_lines();
            let mut header = i18n::t_args(
                "search-results-header",
                &[
                    ("query", filters.query.as_str()),
                    (
                        "shown",
                        &(found.hits.len() + (page - 1) * limit).to_string(),
                    ),
                    ("total", &found.total_hits.to_string()),
                ],
            );
            if !active.is_empty() {
                header.push_str(&format!(
                    "\n{}",
                    i18n::t_args("filters-line", &[("filters", &active.join(" | "))])
                ));
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
        format!(" {}", i18n::t("added-mark"))
    } else {
        String::new()
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
            .unwrap_or_else(|| (i18n::t("unknown"), i18n::t("unknown")));
        let mut row = vec![
            h.slug.clone(),
            h.project_type.clone(),
            thousands(h.downloads),
            loaders,
            versions,
            truncate(&h.description, 34),
        ];
        if is_added {
            row.push(i18n::t("added-marker"));
        }
        rows.push(row);
        flags.push(is_added);
    }
    let header_text = vec![
        i18n::t("header-slug"),
        i18n::t("header-type"),
        i18n::t("header-downloads"),
        i18n::t("header-loaders"),
        i18n::t("header-minecraft"),
        i18n::t("header-description"),
    ];
    let mut headers: Vec<&str> = header_text.iter().map(String::as_str).collect();
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
    println!("  {:<10} {}", i18n::t("header-slug"), p.slug);
    println!("  {:<10} {}", i18n::t("header-id"), p.id);
    println!("  {:<10} {}", i18n::t("header-type"), p.project_type);
    println!(
        "  {:<10} {}",
        i18n::t("header-by"),
        if people.is_empty() {
            i18n::t("unknown")
        } else {
            people.join(", ")
        }
    );
    println!(
        "  {:<10} {}",
        i18n::t("header-downloads"),
        thousands(p.downloads)
    );
    println!(
        "  {:<10} {}",
        i18n::t("header-followers"),
        thousands(p.followers)
    );
    println!(
        "  {:<10} {}",
        i18n::t("header-updated"),
        short_date(&p.updated)
    );
    println!(
        "  {:<10} {}",
        i18n::t("header-loaders"),
        p.loaders.join(", ")
    );
    println!(
        "  {:<10} {}",
        i18n::t("header-versions"),
        p.game_versions.len()
    );
    if let Some(l) = &p.license {
        println!("  {:<10} {} ({})", i18n::t("header-license"), l.name, l.id);
    }
    for (label, url) in [
        (i18n::t("header-source"), &p.source_url),
        (i18n::t("header-issues"), &p.issues_url),
        (i18n::t("header-wiki"), &p.wiki_url),
        (i18n::t("header-discord"), &p.discord_url),
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
        println!("  {:<10} {label}  {}", i18n::t("header-donate"), d.url);
    }
    if !disclosures.is_empty() {
        println!("  {}", i18n::t("header-disclosures"));
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
        .chain(std::iter::once(i18n::t("option-all-of-them")))
        .collect();
    let picked = inquire::MultiSelect::new(
        &i18n::t_args(
            "targets-prompt",
            &[("file", MANIFEST_FILE), ("count", &all.len().to_string())],
        ),
        options,
    )
    .prompt()
    .map_err(ask_failed)?;
    if picked.iter().any(|o| o == &i18n::t("option-all-of-them")) || picked.is_empty() {
        return Ok(all.clone());
    }
    let chosen: Vec<Target> = labels
        .iter()
        .enumerate()
        .filter(|(_, l)| picked.iter().any(|p| p == *l))
        .filter_map(|(i, _)| all.get(i).cloned())
        .collect();
    if chosen.is_empty() {
        bail!(
            "{}",
            i18n::t_args("nothing-to-install", &[("file", MANIFEST_FILE)])
        );
    }
    Ok(chosen)
}

fn short_date(stamp: &str) -> String {
    stamp.chars().take(10).collect()
}

fn is_addable_kind(project_type: &str) -> bool {
    matches!(project_type, "mod" | "resourcepack" | "shader" | "datapack")
}

async fn after_show(cli: &Cli, slug: &str, project_type: &str, from_info: bool) -> Result<()> {
    let addable = is_addable_kind(project_type);
    let leave = if from_info {
        i18n::t("after-exit")
    } else {
        i18n::t("after-go-back")
    };
    if !addable {
        say(
            cli,
            &i18n::t_args(
                "after-not-addable",
                &[("slug", slug), ("kind", project_type)],
            ),
        );
        println!(
            "{}",
            grey(
                cli,
                &i18n::t_args("after-this-is", &[("kind", project_type)])
            )
        );
    }
    let add_label = if addable {
        i18n::t("after-add-this")
    } else {
        grey(cli, &i18n::t("after-add-this"))
    };
    let menu = vec![add_label, leave];

    loop {
        let picked = inquire::Select::new(&i18n::t("after-prompt"), menu.clone())
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
            say(
                cli,
                &i18n::t_args(
                    "after-not-addable-no",
                    &[("slug", slug), ("kind", project_type)],
                ),
            );
            continue;
        }
        let Ok(p) = load_project(cli) else {
            println!("{}", i18n::t_args("after-add-cmd", &[("slug", slug)]));
            return Ok(());
        };
        if added_slugs(cli).contains(&slug.to_ascii_lowercase()) {
            say(
                cli,
                &i18n::t_args(
                    "after-already-added",
                    &[("slug", slug), ("file", MANIFEST_FILE)],
                ),
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
            say(cli, &i18n::t("nothing-came-back"));
            return Ok(());
        }
        let limit = filters.limit.clamp(1, 100);
        let pages = found.total_hits.div_ceil(limit);
        let mut options: Vec<String> = found.hits.iter().map(|h| hit_line(h, &added)).collect();
        let first_nav = options.len();
        let mut navs: Vec<Nav> = Vec::new();
        if filters.page < pages {
            options.push(i18n::t_args(
                "search-next-page",
                &[
                    ("next", &(filters.page + 1).to_string()),
                    ("pages", &pages.to_string()),
                ],
            ));
            navs.push(Nav::Next);
        }
        if filters.page > 1 {
            options.push(i18n::t("search-previous-page"));
            navs.push(Nav::Prev);
        }
        options.push(i18n::t("search-menu-filters"));
        navs.push(Nav::EditFilters);
        options.push(i18n::t("search-menu-quit"));
        navs.push(Nav::Quit);

        let active = filters.active_lines();
        let mut header = i18n::t_args(
            "browse-header",
            &[
                ("sort", sort.as_str()),
                (
                    "shown",
                    &(found.hits.len() + (filters.page - 1) * limit).to_string(),
                ),
                ("total", &found.total_hits.to_string()),
            ],
        );
        if !active.is_empty() {
            header.push_str(&format!(
                "\n{}",
                i18n::t_args("filters-line", &[("filters", &active.join(" | "))])
            ));
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
    step(
        cli,
        1,
        3,
        &i18n::t_args("add-finding", &[("project", project)]),
    );
    add_project(cli, &p, project, version, targets).await?;
    step(cli, 3, 3, &i18n::t("step-done"));
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
            Some(v) => {
                client
                    .resolve_pinned(project, Some(&target.minecraft), &loaders, v)
                    .await
            }
            None => {
                client
                    .resolve(project, Some(&target.minecraft), &loaders)
                    .await
            }
        };
        match found {
            Ok(r) => {
                if r.project.project_type == "modpack" {
                    bail!(
                        "{}",
                        i18n::t_args("add-modpack-rejected", &[("slug", r.project.slug.as_str())])
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
                return Err(e).context(i18n::t("add-nothing-added"));
            }
            Err(e) => warn(
                cli,
                &i18n::t_args(
                    "add-skipped",
                    &[
                        ("project", project),
                        ("target", target.label().as_str()),
                        ("error", &format!("{e:#}")),
                    ],
                ),
            ),
        }
    }

    if versions.is_empty() {
        bail!("{}", i18n::t("add-nothing-added"));
    }
    if p.manifest
        .packages
        .iter()
        .any(|x| x.key().eq_ignore_ascii_case(&slug))
    {
        bail!(
            "{}",
            i18n::t_args(
                "add-already",
                &[("slug", slug.as_str()), ("file", MANIFEST_FILE)]
            )
        );
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
        say(
            cli,
            &i18n::t_args(
                "add-learned",
                &[("slug", slug.as_str()), ("list", &list.join(", "))],
            ),
        );
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
        &i18n::t_args(
            "add-added",
            &[
                ("slug", slug.as_str()),
                ("file", MANIFEST_FILE),
                ("count", &count.to_string()),
                ("total", &targets.len().to_string()),
            ],
        ),
    );
    let last = pack.packages.last().expect("just pushed");
    if let Some(v) = &last.version {
        println!("  {:<24} {v}", i18n::t("header-default"));
    }
    for (k, v) in &last.versions {
        println!("  {k:<24} {v}");
    }
    if kind == PkgType::Resourcepack {
        println!("  {}", i18n::t("add-inactive-hint"));
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
        detail(
            cli,
            &i18n::t_args(
                "dep-required",
                &[("slug", slug.as_str()), ("id", id.as_str())],
            ),
        );
        let fresh = load_project(cli)?;
        match add_with_deps(cli, &fresh, &id, None, targets.clone(), seen).await {
            Ok(()) => pulled.push(id),
            Err(e) => warn(
                cli,
                &i18n::t_args(
                    "dep-add-failed",
                    &[("id", id.as_str()), ("error", &format!("{e:#}"))],
                ),
            ),
        }
    }
    if !pulled.is_empty() {
        say(
            cli,
            &i18n::t_args(
                "dep-installed",
                &[("slug", slug.as_str()), ("list", &pulled.join(", "))],
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
        bail!(
            "{}",
            i18n::t_args(
                "remove-missing",
                &[("project", project), ("file", MANIFEST_FILE)]
            )
        );
    }
    p.manifest.write(&p.dir)?;
    say(
        cli,
        &i18n::t_args(
            "remove-done",
            &[("project", project), ("file", MANIFEST_FILE)],
        ),
    );
    Ok(())
}

fn lock_project_has(lock: Option<&Lock>, package_key: &str, query: &str) -> bool {
    let Some(lock) = lock else {
        return false;
    };
    lock.targets
        .iter()
        .flat_map(|t| t.packages.iter())
        .any(|lp| {
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
        bail!("{}", i18n::t("find-query-empty"));
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
        } else if pkg
            .ids
            .as_deref()
            .is_some_and(|ids| ids.iter().any(|x| x.eq_ignore_ascii_case(q)))
        {
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
                pkg.versions
                    .values()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
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
        say(
            cli,
            &i18n::t_args("find-nothing", &[("query", &format!("{q:?}"))]),
        );
        return Ok(());
    }
    if !rows.is_empty() {
        say(
            cli,
            &i18n::t_args(
                "find-packages-count",
                &[
                    ("count", &rows.len().to_string()),
                    ("suffix", if rows.len() == 1 { "" } else { "s" }),
                    ("file", MANIFEST_FILE),
                ],
            ),
        );
        println!(
            "{}",
            table(
                cli,
                &[
                    i18n::t("header-index"),
                    i18n::t("header-type"),
                    i18n::t("header-project"),
                    i18n::t("header-version-s"),
                    i18n::t("header-matched-by"),
                ],
                &rows,
                &vec![false; rows.len()]
            )
        );
    }
    if !hit_targets.is_empty() {
        say(
            cli,
            &i18n::t_args(
                "find-targets-count",
                &[
                    ("count", &hit_targets.len().to_string()),
                    ("suffix", if hit_targets.len() == 1 { "" } else { "s" }),
                    ("file", SOFTWARES_FILE),
                ],
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
        say(cli, &i18n::t("find-remove-hint"));
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("{}", i18n::t("find-needs-terminal"));
    }
    if hit_pkgs.is_empty() && hit_targets.is_empty() {
        return Ok(());
    }
    let yes = inquire::Confirm::new(&i18n::t_args(
        "find-confirm",
        &[
            ("packages", &hit_pkgs.len().to_string()),
            ("package_suffix", if hit_pkgs.len() == 1 { "" } else { "s" }),
            ("targets", &hit_targets.len().to_string()),
            (
                "target_suffix",
                if hit_targets.len() == 1 { "" } else { "s" },
            ),
        ],
    ))
    .with_default(false)
    .prompt()
    .map_err(ask_failed)?;
    if !yes {
        say(cli, &i18n::t("find-left-alone"));
        return Ok(());
    }

    if !hit_pkgs.is_empty() {
        let mut p = load_project(cli)?;
        for idx in hit_pkgs.iter().rev() {
            let name = p.manifest.packages[*idx].key();
            p.manifest.packages.remove(*idx);
            say(
                cli,
                &i18n::t_args(
                    "find-removed-package",
                    &[("name", name.as_str()), ("file", MANIFEST_FILE)],
                ),
            );
        }
        p.manifest.write(&p.dir)?;
    }
    if !hit_targets.is_empty() {
        let mut p = load_project(cli)?;
        let mut gone = 0usize;
        for idx in hit_targets.iter().rev() {
            let label = p.softwares.targets[*idx].label();
            p.softwares.targets.remove(*idx);
            say(
                cli,
                &i18n::t_args(
                    "find-dropped-target",
                    &[("label", label.as_str()), ("file", SOFTWARES_FILE)],
                ),
            );
            gone += 1;
        }
        p.softwares.write(&p.dir)?;
        if gone > 0 {
            say(
                cli,
                &i18n::t_args("wrote-file", &[("file", SOFTWARES_FILE)]),
            );
        }
    }
    say(cli, &i18n::t("find-lock-hint"));
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
        say(cli, &i18n::t("list-empty"));
        return Ok(());
    }
    let all_targets: Vec<LockedTarget> =
        lock.as_ref().map(|l| l.targets.clone()).unwrap_or_default();
    let targets: Vec<LockedTarget> = if mc.is_some() || loader.is_some() {
        vec![manifest::pick(&all_targets, mc, loader)?.clone()]
    } else {
        all_targets
    };

    let mut headers: Vec<String> = vec![
        i18n::t("list-header-type"),
        i18n::t("list-header-package"),
        i18n::t("list-header-wanted"),
    ];
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
                    l.targets
                        .iter()
                        .find(|lt| lt.minecraft == t.minecraft && lt.loader.kind == t.loader.kind)
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
        println!("\n{}", i18n::t("nothing-locked-hint"));
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
        p.softwares.targets.iter().map(Targetish::label).collect()
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
                    &i18n::t_args("lock-skipped", &[("key", pkg.key().as_str())]),
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
        let pulled = pull_deps(
            cli,
            &client,
            &mut p.manifest,
            target,
            single,
            &mut packages,
            deps,
        )
        .await?;
        steps.finish();
        if packages.is_empty() {
            bail!(
                "{}",
                i18n::t_args(
                    "lock-nothing-resolves",
                    &[("label", target.label().as_str())]
                )
            );
        }
        for slug in &pulled {
            if !pulled_all.iter().any(|x| x == slug) {
                pulled_all.push(slug.clone());
            }
        }
        say(
            cli,
            &i18n::t_args(
                "lock-summary",
                &[
                    ("count", &packages.len().to_string()),
                    ("label", target.label().as_str()),
                    (
                        "missing",
                        &if missing.is_empty() {
                            String::new()
                        } else {
                            i18n::t_args(
                                "lock-missing-tail",
                                &[
                                    ("count", &missing.len().to_string()),
                                    ("list", &missing.join(", ")),
                                ],
                            )
                        },
                    ),
                ],
            ),
        );
        fresh.insert(
            Targetish::label(target),
            LockedTarget {
                minecraft: target.minecraft.clone(),
                java: target.java.clone(),
                loader: target.loader.clone(),
                packages,
                external_packs: Vec::new(),
            },
        );
    }
    if !pulled_all.is_empty() {
        say(
            cli,
            &i18n::t_args(
                "lock-pulled-deps",
                &[
                    ("count", &pulled_all.len().to_string()),
                    ("suffix", if pulled_all.len() == 1 { "" } else { "s" }),
                    ("list", &pulled_all.join(", ")),
                ],
            ),
        );
        p.manifest.write(&p.dir)?;
        say(cli, &i18n::t_args("wrote-file", &[("file", MANIFEST_FILE)]));
    }

    let (renamed_packs, learned_shas) = reconcile_external_packs(cli, p, &fresh);
    if renamed_packs > 0 {
        say(
            cli,
            &i18n::t_args(
                "lock-renamed-packs",
                &[
                    ("count", &renamed_packs.to_string()),
                    ("suffix", if renamed_packs == 1 { "" } else { "s" }),
                ],
            ),
        );
    }
    if renamed_packs > 0 || learned_shas > 0 {
        p.manifest.write(&p.dir)?;
        say(cli, &i18n::t_args("wrote-file", &[("file", MANIFEST_FILE)]));
    }
    for (_, t) in fresh.iter_mut() {
        let minecraft = t.minecraft.clone();
        let loader = t.loader.kind.clone();
        t.external_packs = p
            .manifest
            .external_packs
            .iter()
            .map(|e| crate::lock::LockedExternal {
                name: e.name.clone(),
                active: e.is_active(),
                builtin: e.is_builtin(),
                position: e.position_for(&minecraft, &loader),
            })
            .collect();
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
    say(cli, &i18n::t_args("wrote-file", &[("file", LOCK_FILE)]));
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
        version: if single {
            Some(version.to_string())
        } else {
            None
        },
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
                &i18n::t_args(
                    "dep-not-required",
                    &[("id", pid.as_str()), ("kind", dep.dependency_type.as_str())],
                ),
            );
            continue;
        }
        if visited.contains(&low) || modmeta::is_platform(&low) {
            detail(
                cli,
                &i18n::t_args("dep-already-seen", &[("id", pid.as_str())]),
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
                            warn(
                                cli,
                                &i18n::t_args(
                                    "dep-unusable",
                                    &[
                                        ("id", pid.as_str()),
                                        ("version", vid.as_str()),
                                        ("error", &format!("{e:#}")),
                                    ],
                                ),
                            );
                            continue;
                        }
                    }
                }
                _ => {
                    warn(
                        cli,
                        &i18n::t_args(
                            "dep-unfetchable",
                            &[("id", pid.as_str()), ("version", vid.as_str())],
                        ),
                    );
                    continue;
                }
            }
        } else {
            match client
                .resolve(&pid, Some(&target.minecraft), &loaders)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    warn(
                        cli,
                        &i18n::t_args(
                            "dep-no-build",
                            &[("id", pid.as_str()), ("error", &format!("{e:#}"))],
                        ),
                    );
                    continue;
                }
            }
        };
        let slug = resolved.project.slug.clone();
        if resolved.project.project_type != "mod" {
            warn(
                cli,
                &i18n::t_args(
                    "dep-wrong-kind",
                    &[
                        ("id", pid.as_str()),
                        ("kind", resolved.project.project_type.as_str()),
                    ],
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
                    &i18n::t_args("dep-skipped-by-config", &[("slug", slug.as_str())]),
                );
                continue;
            }
        }
        if packages
            .iter()
            .any(|lp| lp.project.eq_ignore_ascii_case(&slug))
            || visited.contains(&slug.to_ascii_lowercase())
        {
            detail(
                cli,
                &i18n::t_args("dep-have-already", &[("slug", slug.as_str())]),
            );
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
            version: if single {
                Some(version_number.clone())
            } else {
                None
            },
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
        bail!(
            "{}",
            i18n::t_args(
                "lock-manifest-changed",
                &[("manifest", MANIFEST_FILE), ("lock", LOCK_FILE)]
            )
        );
    }
    let wanted: Vec<String> = p.softwares.targets.iter().map(Targetish::label).collect();
    let have = lock.labels();
    if wanted != have {
        bail!(
            "{}",
            i18n::t_args(
                "lock-targets-changed",
                &[
                    ("softwares", SOFTWARES_FILE),
                    ("wanted", &wanted.join(", ")),
                    ("lock", LOCK_FILE),
                    ("have", &have.join(", ")),
                ]
            )
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
                Ok(r) => stale.push(i18n::t_args(
                    "lock-replaced",
                    &[
                        ("project", pkg.project.as_str()),
                        ("was", pkg.version_number.as_str()),
                        ("now", r.version.version_number.as_str()),
                    ],
                )),
                Err(e) => stale.push(i18n::t_args(
                    "lock-gone",
                    &[
                        ("project", pkg.project.as_str()),
                        ("version", pkg.version_number.as_str()),
                        ("error", &format!("{e:#}")),
                    ],
                )),
            }
        }
    }
    if stale.is_empty() {
        say(
            cli,
            &i18n::t_args("lock-fresh", &[("count", &lock.targets.len().to_string())]),
        );
        return Ok(());
    }
    for line in &stale {
        warn(cli, line);
    }
    bail!("{}", i18n::t_args("lock-stale", &[("lock", LOCK_FILE)]));
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
            bail!(
                "{}",
                i18n::t_args(
                    "build-manifest-changed",
                    &[("manifest", MANIFEST_FILE), ("lock", LOCK_FILE)]
                )
            );
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
    say(cli, &i18n::t("build-checking"));
    print_report(cli, &validate::check(&lock, &p.manifest, &sources))?;
    print_report(cli, &validate::check_sources(&sources))?;
    check_schema(cli, &p);

    for target in &chosen {
        let active = target.active_packs();
        if !active.is_empty() {
            println!();
            say(
                cli,
                &i18n::t_args("build-active-packs", &[("label", target.label().as_str())]),
            );
            for pkg in &active {
                println!("  {}", pkg.path);
            }
        }
    }
    println!();

    if build::stamp_fresh(&out_dir, &lock, &sources) {
        say(cli, &i18n::t("build-fresh"));
        check_schema(cli, &p);
        return Ok(());
    }
    let mut built_paths: Vec<PathBuf> = Vec::new();

    let client = Modrinth::new(api)?;
    for target in chosen {
        say(
            cli,
            &i18n::t_args(
                "publish-building-for",
                &[
                    ("mc", target.minecraft.as_str()),
                    ("loader", target.loader.kind.as_str()),
                ],
            ),
        );
        let on_disk_packs = build::pack_paths_on_disk(&sources);
        for pkg in target.active_packs() {
            let fname = pkg.path.rsplit('/').next().unwrap_or(&pkg.path);
            if !pkg.downloads.is_empty() {
                detail(
                    cli,
                    &i18n::t_args(
                        "build-from-modrinth",
                        &[("project", pkg.project.as_str()), ("file", fname)],
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
                            &i18n::t_args(
                                "build-sha-mismatch",
                                &[("project", pkg.project.as_str())],
                            ),
                        );
                    }
                }
                None => warn(
                    cli,
                    &i18n::t_args(
                        "build-not-in-overrides",
                        &[("project", pkg.project.as_str()), ("file", fname)],
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
                    i18n::t_args("build-verify-failed", &[("label", target.label().as_str())])
                });
            }
        };
        detail(
            cli,
            &i18n::t_args(
                "build-shas",
                &[
                    ("files", &verified.downloaded.to_string()),
                    ("shas", &verified.shas.to_string()),
                ],
            ),
        );
        let path = build::build(&lock, target, &out_dir, &sources)?;
        built_paths.push(path.clone());
        let _ = staging::wipe(&staged);
        say(
            cli,
            &i18n::t_args("build-built", &[("path", &path.display().to_string())]),
        );
        println!(
            "  {}",
            i18n::t_args(
                "build-runtime",
                &[
                    ("mc", target.minecraft.as_str()),
                    ("loader", target.loader.kind.as_str()),
                    ("version", target.loader.version.as_str()),
                    ("java", target.java.as_deref().unwrap_or("-")),
                    ("count", &target.packages.len().to_string()),
                ]
            )
        );
        for pkg in &target.packages {
            if let Some(position) = pkg.position {
                println!(
                    "  {}",
                    i18n::t_args(
                        "build-active",
                        &[
                            ("project", pkg.project.as_str()),
                            ("position", &format!("{position:?}")),
                        ]
                    )
                );
            }
        }
    }
    check_schema(cli, &p);
    build::write_stamp(&out_dir, &lock, &sources, &built_paths)?;
    Ok(())
}

fn closing_lines(cli: &Cli, dir: &Path, targets: &[Target], committed: Option<bool>) {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string());
    say(
        cli,
        &i18n::t_args("closing-done", &[("name", name.as_str())]),
    );
    say(cli, &i18n::t("closing-readme"));
    println!();
    println!("{}", i18n::t("closing-view-projects"));
    println!();
    println!("{}", i18n::t("closing-search"));
    println!("{}", i18n::t("closing-browse"));
    println!();
    for t in targets {
        println!(
            "{}",
            i18n::t_args(
                "closing-created",
                &[
                    ("mc", t.minecraft.as_str()),
                    ("loader", t.loader.kind.as_str()),
                    ("loader2", t.loader.kind.as_str()),
                    ("version", t.loader.version.as_str()),
                ]
            )
        );
    }
    match committed {
        Some(true) => println!("  {}", i18n::t("closing-git-commit")),
        Some(false) => warn(cli, &i18n::t("closing-git-failed")),
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
        if git_flag || committed {
            Some(committed)
        } else {
            None
        },
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
    notice(cli, &importer::license_warning());
    println!();

    step(cli, 1, 6, &i18n::t("import-reading-archive"));
    let index = importer::read_index(mrpack)?;
    let pack_name = name
        .map(str::to_string)
        .unwrap_or_else(|| importer::name_of(&index));
    let dir = cli
        .dir
        .join(folder.map_or_else(|| scaffold::slugify(&pack_name), |f| scaffold::slugify(f)));
    importer::ensure_empty_dir(&dir, force)?;

    let minecraft = importer::minecraft_of(&index)?;
    let loader_pairs = importer::loaders_of(&index);
    if loader_pairs.is_empty() {
        bail!("{}", i18n::t("import-no-loader"));
    }

    let files = importer::files_of(&index)?;
    step(
        cli,
        2,
        6,
        &i18n::t_args("import-looking-up", &[("count", &files.len().to_string())]),
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
        &i18n::t_args(
            "import-versions-api",
            &[
                ("sent", &version_ids.len().to_string()),
                ("got", &versions.len().to_string()),
            ],
        ),
    );
    detail(
        cli,
        &i18n::t_args(
            "import-projects-api",
            &[
                ("sent", &project_ids.len().to_string()),
                ("got", &projects.len().to_string()),
            ],
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

    step(cli, 3, 6, &i18n::t("import-writing-packages"));
    let mut packages = Vec::new();
    for f in &files {
        let Some(v) = by_version.get(f.version_id.as_str()) else {
            detail(
                cli,
                &format!(
                    "{}",
                    i18n::t_args("import-missing", &[("path", f.path.as_str())])
                ),
            );
            warn(
                cli,
                &i18n::t_args("import-gone", &[("path", f.path.as_str())]),
            );
            continue;
        };
        let Some(pj) = by_project.get(v.project_id.as_str()) else {
            detail(
                cli,
                &i18n::t_args("import-no-project", &[("path", f.path.as_str())]),
            );
            warn(
                cli,
                &i18n::t_args("import-no-project-warn", &[("path", f.path.as_str())]),
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

    let java =
        importer::java_of(&index).or_else(|| Some(loaders::java_for(&minecraft).to_string()));
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

    step(cli, 4, 6, &i18n::t("import-pulling-overrides"));
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    let written = importer::extract_overrides(mrpack, &dir, include_configs, &mut sources)?;
    if include_configs {
        say(
            cli,
            &format!(
                "  {}",
                i18n::t_args(
                    "import-overrides-with-configs",
                    &[("count", &written.to_string())]
                )
            ),
        );
    } else {
        say(
            cli,
            &format!(
                "  {}",
                i18n::t_args(
                    "import-overrides-without-configs",
                    &[("count", &written.to_string())]
                )
            ),
        );
    }

    step(cli, 5, 6, &i18n::t("import-lock-original"));
    let mut project = Project {
        dir: dir.clone(),
        manifest: pack,
        softwares,
    };
    do_lock(cli, api, &mut project, None, None).await?;

    step(cli, 6, 6, &i18n::t("import-wrapping-up"));
    let committed = if git {
        Some(scaffold::init_repo(&dir)?)
    } else {
        None
    };
    println!();
    closing_lines(cli, &dir, &project.softwares.targets, committed);
    Ok(())
}

async fn add_ver(cli: &Cli, api: &str, version: &str, loaders: &[LoaderKind]) -> Result<()> {
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
        bail!("{}", i18n::t("addver-no-loader"));
    }

    let mut fresh = Vec::new();
    for k in &kinds {
        if p.softwares
            .targets
            .iter()
            .any(|t| t.minecraft == version && t.loader.kind == k.as_str())
        {
            bail!(
                "{}",
                i18n::t_args(
                    "addver-already",
                    &[("version", version), ("loader", k.as_str())]
                )
            );
        }
        let lv = loaders::resolve(*k, version).await.with_context(|| {
            i18n::t_args(
                "addver-resolve-failed",
                &[("loader", &k.to_string()), ("version", version)],
            )
        })?;
        fresh.push(Target::new(version, *k, &lv));
    }
    for t in &fresh {
        say(
            cli,
            &i18n::t_args("addver-adding", &[("label", t.label().as_str())]),
        );
    }

    let lock = if Lock::exists(&p.dir) {
        Some(Lock::load(&p.dir)?)
    } else {
        None
    };
    let client = Modrinth::new(api)?;
    p.softwares.targets.extend(fresh);

    step(cli, 1, 2, &i18n::t("step-versions-every-target"));
    let total = p.manifest.packages.len() * p.softwares.targets.len();
    let mut steps = crate::progress::Steps::new(total);
    let mut maps: Vec<BTreeMap<String, String>> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for pkg in &p.manifest.packages {
        let mut map = BTreeMap::new();
        for t in &p.softwares.targets {
            steps.at(
                map.len() + maps.iter().map(|m| m.len()).sum::<usize>() + 1,
                &pkg.key(),
            );
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

    step(cli, 2, 2, &i18n::t("step-locking"));
    do_lock(cli, api, &mut p, None, None).await?;
    say(
        cli,
        &i18n::t_args(
            "addver-now-building",
            &[(
                "list",
                &p.softwares
                    .targets
                    .iter()
                    .map(Targetish::label)
                    .collect::<Vec<_>>()
                    .join(", "),
            )],
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
                &i18n::t_args(
                    "identity-jar-failed",
                    &[("slug", slug), ("error", &format!("{e:#}"))],
                ),
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
        "{}",
        i18n::t_args(
            "identity-changed",
            &[
                ("slug", slug),
                ("id", id.as_str()),
                ("known", &known.join(", ")),
            ]
        )
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

    step(cli, 1, 2, &i18n::t("step-newest-every-target"));
    let total: usize = p.manifest.packages.len() * targets.len();
    let mut steps = crate::progress::Steps::new(total);
    let mut n = 0usize;
    let mut changes: Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    )> = Vec::new();
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
                detail(
                    cli,
                    &i18n::t_args("update-locked", &[("slug", slug.as_str())]),
                );
                continue;
            }
            if p.manifest.packages[idx].is_skipped(&target.minecraft, &target.loader.kind) {
                skipped.push((
                    slug.clone(),
                    target.label(),
                    i18n::t("update-skipped-reason"),
                ));
                continue;
            }
            let kind = p.manifest.packages[idx].kind;
            let known = p.manifest.packages[idx].ids.clone().unwrap_or_default();
            let pinned =
                p.manifest.packages[idx].version_for(&target.minecraft, &target.loader.kind);
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
                            .map(|lp| (lp.version_number.clone(), lp.version_id.clone()))
                    })
            });
            let was = locked.as_ref().map(|(v, _)| v.clone()).or(pinned);
            let old_vid = locked
                .as_ref()
                .map(|(_, id)| id.clone())
                .unwrap_or_default();

            let Ok(project) = client.project(&slug).await else {
                skipped.push((
                    slug.clone(),
                    target.label(),
                    i18n::t("update-reason-not-modrinth"),
                ));
                continue;
            };
            let pid = project.id.clone();
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
                    i18n::t("update-reason-no-versions"),
                ));
                continue;
            };
            if list.is_empty() {
                warn(
                    cli,
                    &i18n::t_args(
                        "update-nothing-for-target",
                        &[("slug", slug.as_str()), ("target", target.label().as_str())],
                    ),
                );
                skipped.push((
                    slug.clone(),
                    target.label(),
                    i18n::t_args(
                        "update-reason-no-build",
                        &[
                            ("loader", target.loader.kind.as_str()),
                            ("mc", target.minecraft.as_str()),
                        ],
                    ),
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
            let Some(version) = list.into_iter().find(|v| v.version_number == best) else {
                continue;
            };
            let new_vid = version.id.clone();
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
                pid,
                new_vid,
                old_vid,
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
        say(cli, &i18n::t("update-newest"));
    } else {
        let rows: Vec<Vec<String>> = changes
            .iter()
            .map(|(s, _t, k, w, n, _p, _nv, _ov)| vec![s.clone(), k.clone(), w.clone(), n.clone()])
            .collect();
        if !rows.is_empty() {
            say(
                cli,
                &i18n::t_args(
                    "update-ready",
                    &[
                        ("count", &rows.len().to_string()),
                        ("suffix", if rows.len() == 1 { "" } else { "s" }),
                    ],
                ),
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
            &i18n::t_args(
                "update-targets-failed",
                &[("count", &skipped.len().to_string())],
            ),
        );
        for (s, t, why) in &skipped {
            detail(
                cli,
                &i18n::t_args(
                    "update-skip-row",
                    &[
                        ("slug", s.as_str()),
                        ("target", t.as_str()),
                        ("why", why.as_str()),
                    ],
                ),
            );
        }
    }
    if dry_run {
        say(cli, &i18n::t("update-nothing-written"));
        return Ok(());
    }
    if !bad.is_empty() {
        for line in &bad {
            error_line(cli, line);
        }
        bail!("{}", i18n::t("update-aborted"));
    }
    if changes.is_empty() {
        return Ok(());
    }
    step(cli, 2, 2, &i18n::t("update-writing"));
    for (slug, _title, key, _was, now, _pid, _nv, _ov) in &changes {
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
    say(cli, &i18n::t_args("wrote-file", &[("file", MANIFEST_FILE)]));
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
        for (_slug, title, _key, was, now, pid, new_vid, old_vid) in &changes {
            let project_url = format!("https://modrinth.com/project/{pid}");
            let old_disp = if was == "*" {
                "*".to_string()
            } else {
                was.clone()
            };
            let old_link = if old_vid.is_empty() {
                old_disp
            } else {
                format!("[{old_disp}]({project_url}/version/{old_vid})")
            };
            let new_link = if new_vid.is_empty() {
                now.clone()
            } else {
                format!("[{now}]({project_url}/version/{new_vid})")
            };
            lines.push(format!(
                "- [{title}]({project_url}) has been updated from {old_link} to {new_link}"
            ));
        }
        if let Some(lock) = &lock {
            for t in &p.softwares.targets {
                let old = lock
                    .targets
                    .iter()
                    .find(|lt| lt.minecraft == t.minecraft && lt.loader.kind == t.loader.kind)
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
        write_changelog_toml(cli, &p.dir, &lines)?;
    }
    say(cli, &i18n::t("lock-fresh-simple"));
    Ok(())
}

fn changelog_block_date(line: &str) -> Option<&str> {
    let line = line.trim();
    let inner = line.strip_prefix("[[")?.strip_suffix("]]")?;
    if inner.is_empty() { None } else { Some(inner) }
}

fn split_changelog(raw: &str) -> (String, Vec<(String, String)>) {
    let mut preamble = String::new();
    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in raw.split_inclusive('\n') {
        if let Some(date) = changelog_block_date(line) {
            if let Some(previous) = current.take() {
                blocks.push(previous);
            }
            current = Some((date.to_string(), line.to_string()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
        } else {
            preamble.push_str(line);
        }
    }
    if let Some(last) = current {
        blocks.push(last);
    }
    (preamble, blocks)
}

fn render_changelog(preamble: &str, blocks: &[(String, String)]) -> String {
    let mut out = preamble.trim_end_matches('\n').to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    let rendered: Vec<&str> = blocks
        .iter()
        .map(|(_, block)| block.trim_end_matches('\n'))
        .collect();
    out.push_str(&rendered.join("\n\n"));
    out.push('\n');
    out
}

fn write_changelog_toml(cli: &Cli, dir: &Path, lines: &[String]) -> Result<()> {
    let now = time::OffsetDateTime::now_utc();
    let date = format!(
        "{}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    );
    let block = format!(
        "[[{date}]]\nchangelogtext = \"\"\"\n{}\n\"\"\"\n",
        lines.join("\n")
    );
    let raw =
        std::fs::read_to_string(dir.join(crate::manifest::CHANGELOG_FILE)).unwrap_or_default();
    let (preamble, mut blocks) = split_changelog(&raw);
    let insert_at = blocks
        .iter()
        .rposition(|(existing, _)| existing == &date)
        .map(|i| i + 1)
        .unwrap_or(blocks.len());
    blocks.insert(insert_at, (date, block));
    blocks.sort_by(|a, b| b.0.cmp(&a.0));
    let out = render_changelog(&preamble, &blocks);
    crate::manifest::write_file(&dir.join(crate::manifest::CHANGELOG_FILE), &out)?;
    say(
        cli,
        &i18n::t_args("wrote-file", &[("file", crate::manifest::CHANGELOG_FILE)]),
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
            "{}",
            i18n::t_args(
                "onever-no-target",
                &[
                    ("version", version),
                    (
                        "list",
                        &p.softwares
                            .targets
                            .iter()
                            .map(Targetish::label)
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ]
            )
        );
    }
    if kept.len() == p.softwares.targets.len() {
        say(
            cli,
            &i18n::t_args("onever-already-only", &[("version", version)]),
        );
    } else {
        let dropped = p.softwares.targets.len() - kept.len();
        say(
            cli,
            &i18n::t_args(
                "onever-dropping",
                &[
                    ("count", &dropped.to_string()),
                    ("suffix", if dropped == 1 { "" } else { "s" }),
                ],
            ),
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
        say(
            cli,
            &i18n::t_args("onever-keeping", &[("label", t.label().as_str())]),
        );
    }
    Ok(())
}

fn pick_workflow(cli: &Cli, given: Option<WorkflowKind>) -> Result<WorkflowKind> {
    if let Some(w) = given {
        return Ok(w);
    }
    if !std::io::stdin().is_terminal() {
        say(cli, &i18n::t("workflow-default-source"));
        return Ok(WorkflowKind::Source);
    }
    let picked = inquire::Select::new(
        &i18n::t("workflow-prompt"),
        vec![i18n::t("workflow-source"), i18n::t("workflow-binaries")],
    )
    .prompt()
    .map_err(ask_failed)?;
    Ok(if picked == i18n::t("workflow-binaries") {
        WorkflowKind::Binaries
    } else {
        WorkflowKind::Source
    })
}

async fn ask_missing(cli: &Cli, api: &str, mut plan: NewPlan, git_flag: bool) -> Result<NewPlan> {
    if !std::io::stdin().is_terminal() {
        if plan.name.is_none() || plan.minecraft.is_empty() || plan.loaders.is_empty() {
            bail!("{}", i18n::t("needs-terminal"));
        }
        if git_flag {
            plan.git = Some(true);
        }
        plan.workflow = Some(pick_workflow(cli, plan.workflow)?);
        return Ok(plan);
    }
    if plan.name.is_none() {
        plan.name = Some(ask(&i18n::t("ask-pack-name"))?);
    }
    if plan.minecraft.is_empty() {
        plan.minecraft = ask_versions(api).await?;
    }
    if plan.loaders.is_empty() {
        let names: Vec<String> = LoaderKind::all()
            .iter()
            .map(|k| k.as_str().to_string())
            .collect();
        let picked = inquire::Select::new(&i18n::t("ask-loader"), names)
            .prompt()
            .map_err(ask_failed)?;
        plan.loaders = vec![LoaderKind::parse(&picked).unwrap_or(LoaderKind::Fabric)];
    }
    if plan.workflow.is_none() {
        plan.workflow = Some(pick_workflow(cli, None)?);
    }
    if plan.git.is_none() {
        plan.git = Some(git_flag || ask_yes_no(&i18n::t("ask-git-repo")));
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
            !n.is_empty() && n.starts_with('1') && n.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .collect();
    if releases.is_empty() {
        let raw = ask(&i18n::t("ask-versions-fallback"))?;
        let picked = split_versions(&raw);
        if picked.is_empty() {
            bail!("{}", i18n::t("pick-one-version"));
        }
        return Ok(picked);
    }
    let picked = inquire::MultiSelect::new(&i18n::t("ask-versions"), releases)
        .with_page_size(15)
        .prompt()
        .map_err(ask_failed)?;
    if picked.is_empty() {
        bail!("{}", i18n::t("pick-one-version"));
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

async fn account_cmd(
    cli: &Cli,
    login: bool,
    logout: bool,
    show: bool,
    client_id: Option<&str>,
) -> Result<()> {
    if login && logout {
        bail!("{}", i18n::t("account-both-flags"));
    }
    if login {
        let id = account::client_id(client_id)?;
        let account = account::login(&id, |line| device_code_notice(cli, line)).await?;
        say(
            cli,
            &i18n::t_args(
                "account-signed-in",
                &[
                    ("name", account.name.as_str()),
                    ("uuid", account.uuid.as_str()),
                ],
            ),
        );
        detail(cli, &i18n::t("account-refresh-keychain"));
        return Ok(());
    }
    if logout {
        if account::logout()? {
            say(cli, &i18n::t("account-removed"));
        } else {
            say(cli, &i18n::t("account-none-saved"));
        }
        return Ok(());
    }
    let _ = show;
    match account::saved()? {
        Some(saved) => {
            say(
                cli,
                &i18n::t_args(
                    "account-signed-in",
                    &[("name", saved.name.as_str()), ("uuid", saved.uuid.as_str())],
                ),
            );
            detail(
                cli,
                &i18n::t_args("account-client-id", &[("id", saved.client_id.as_str())]),
            );
            detail(cli, &i18n::t("account-refresh-hidden"));
        }
        None => say(cli, &i18n::t("account-no-login")),
    }
    Ok(())
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
    install_java_if_no_there: bool,
    vendor: Option<&str>,
    java_path: Option<&str>,
    msa: bool,
    msa_login: bool,
    client_id: Option<&str>,
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
        vec![
            lock.targets
                .first()
                .context(i18n::t("test-no-targets"))?
                .clone(),
        ]
    };

    let sources = override_sources(&p);
    let client = Modrinth::new(api)?;

    let session = if msa_login {
        let id = account::client_id(client_id)?;
        let signed = account::login(&id, |line| device_code_notice(cli, line)).await?;
        detail(
            cli,
            &i18n::t_args("test-signed-in", &[("name", signed.name.as_str())]),
        );
        Some(account::session(|line| device_code_notice(cli, line)).await?)
    } else if msa {
        Some(
            account::session(|line| device_code_notice(cli, line))
                .await
                .context(i18n::t("test-msa-failed"))?,
        )
    } else {
        None
    };
    if session.is_some() {
        detail(cli, &i18n::t("test-using-msa"));
    }

    for (index, target) in chosen.iter().enumerate() {
        let required_java = minecraft::required_java(&target.minecraft, target.java.as_deref());
        if let Some(declared) = target.java.as_deref().and_then(|s| s.parse::<u32>().ok())
            && let Some(real) = minecraft::java_for_minecraft(&target.minecraft)
            && declared != real
        {
            detail(
                cli,
                &i18n::t_args(
                    "java-mismatch",
                    &[
                        ("declared", &declared.to_string()),
                        ("target", target.minecraft.as_str()),
                        ("real", &real.to_string()),
                    ],
                ),
            );
        }
        let mut java_home: Option<PathBuf> = None;
        if minecraft::java_of(&minecraft::java_candidates(), required_java).is_err() {
            let found = minecraft::all_javas();
            let wrong = if found.is_empty() {
                i18n::t("java-missing")
            } else {
                let mut majors: Vec<String> = Vec::new();
                for j in &found {
                    let m = j.major.to_string();
                    if !majors.contains(&m) {
                        majors.push(m);
                    }
                }
                i18n::t_args(
                    "java-none-found",
                    &[
                        ("required", &required_java.to_string()),
                        ("found", &majors.join(", ")),
                    ],
                )
            };
            println!();
            println!(
                "{}",
                i18n::t_args(
                    "java-required",
                    &[
                        ("required", &required_java.to_string()),
                        ("wrong", wrong.as_str()),
                    ]
                )
            );
            println!();
            let ask = install_java_if_no_there
                || (crate::progress::interactive()
                    && inquire::Confirm::new(&i18n::t("java-install-prompt"))
                        .with_default(true)
                        .prompt()
                        .map_err(ask_failed)?);
            if !ask {
                bail!(
                    "{}",
                    i18n::t_args("java-needed", &[("required", &required_java.to_string())])
                );
            }
            let vendor_id = match vendor {
                Some(v) => crate::java_install::normalise_vendor(v)?,
                None if install_java_if_no_there => crate::java_install::default_vendor(),
                None => {
                    let labels: Vec<String> = crate::java_install::VENDORS
                        .iter()
                        .map(|(_, l)| l.to_string())
                        .collect();
                    let picked = inquire::Select::new(&i18n::t("java-vendor-select"), labels)
                        .prompt()
                        .map_err(ask_failed)?;
                    crate::java_install::VENDORS
                        .iter()
                        .find(|(_, l)| *l == picked)
                        .map(|(id, _)| *id)
                        .unwrap_or_else(crate::java_install::default_vendor)
                }
            };
            let add_path = match java_path {
                Some(v) => v == "yes",
                None => {
                    crate::progress::interactive()
                        && inquire::Confirm::new(&i18n::t("java-home-prompt"))
                            .with_default(false)
                            .prompt()
                            .map_err(ask_failed)?
                }
            };
            let fetch = crate::http::Http::new()?;
            let home = crate::java_install::install(&fetch, vendor_id, required_java, |line| {
                detail(cli, line)
            })
            .await?;
            match crate::java_install::set_java_home(&home, add_path) {
                Ok(true) => detail(cli, &i18n::t("java-home-added")),
                Ok(false) => detail(cli, &i18n::t("java-home-set")),
                Err(e) => warn(
                    cli,
                    &i18n::t_args("java-env-failed", &[("error", &format!("{e:#}"))]),
                ),
            }
            java_home = Some(home);
        }

        let total = 6;
        let dir = staging::fresh_dir(&lock.name, target)?;
        step(
            cli,
            index as usize + 1,
            total * chosen.len(),
            &i18n::t_args("test-staging", &[("label", target.label().as_str())]),
        );

        let outcome: Result<()> = async {
            let copied = staging::copy_overrides(&sources, &dir)?;
            detail(
                cli,
                &i18n::t_args("test-override-files", &[("count", &copied.to_string())]),
            );

            step(
                cli,
                index as usize + 2,
                total * chosen.len(),
                &i18n::t("test-downloading"),
            );
            let staged = staging::download(&client, target, &dir, |line| detail(cli, line)).await?;
            detail(
                cli,
                &i18n::t_args(
                    "test-files-checked",
                    &[
                        ("count", &staged.downloaded.to_string()),
                        ("bytes", staging::human(staged.bytes).as_str()),
                    ],
                ),
            );

            step(
                cli,
                index as usize + 3,
                total * chosen.len(),
                &i18n::t("test-conflicts"),
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
                detail(
                    cli,
                    &i18n::t_args(
                        "test-soft-notes",
                        &[("count", &conflicts.warnings.len().to_string())],
                    ),
                );
            }
            if conflicts.errors.is_empty() {
                detail(cli, &i18n::t("test-no-conflicts"));
            } else {
                println!();
                say(
                    cli,
                    &i18n::t_args(
                        if conflicts.errors.len() == 1 {
                            "test-conflict-one"
                        } else {
                            "test-conflict-many"
                        },
                        &[("count", &conflicts.errors.len().to_string())],
                    ),
                );
                for c in &conflicts.errors {
                    error_line(
                        cli,
                        &format!(
                            "  {}",
                            i18n::t_args(
                                "test-conflict-row",
                                &[
                                    ("offender", c.offender.as_str()),
                                    ("kind", c.kind),
                                    ("other", c.other.as_str()),
                                    ("wanted", c.wanted.as_str()),
                                    ("found", c.found.as_str()),
                                ],
                            )
                        ),
                    );
                }
                let go = inquire::Confirm::new(&i18n::t("test-launch-anyway"))
                    .with_default(false)
                    .prompt()
                    .map_err(ask_failed)?;
                if !go {
                    say(cli, &i18n::t("test-stopping"));
                    if keep {
                        say(
                            cli,
                            &i18n::t_args("test-kept", &[("path", &dir.display().to_string())]),
                        );
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
                &i18n::t("test-install-loader"),
            );
            let http = crate::http::Http::new()?;
            let base = dir.join(".minecraft-ehmodpack");
            let natives =
                minecraft::install(&http, &base, target, java_home, |line| detail(cli, line))
                    .await?;
            detail(
                cli,
                &i18n::t_args(
                    "test-runtime",
                    &[
                        ("mc", target.minecraft.as_str()),
                        ("loader", target.loader.kind.as_str()),
                        ("version", target.loader.version.as_str()),
                        ("java", &natives.java_major.to_string()),
                        ("libs", &natives.libraries.to_string()),
                        ("assets", &natives.assets.to_string()),
                    ],
                ),
            );

            step(
                cli,
                index as usize + 4,
                total * chosen.len(),
                &i18n::t("test-start-game"),
            );
            let mut child = minecraft::spawn(&natives, &dir, session.as_ref(), &target.minecraft)?;
            detail(
                cli,
                &i18n::t_args("test-pid", &[("pid", &child.id().to_string())]),
            );

            match how {
                Launcher::None => {
                    say(
                        cli,
                        &i18n::t_args("test-instance-at", &[("path", &dir.display().to_string())]),
                    );
                    staging::wait_for_enter(&format!("{} ", i18n::t("test-enter-stop")));
                }
                _ => {
                    let _ = child.wait();
                }
            }
            let _ = (launch, profile);
            Ok(())
        }
        .await;

        if let Err(error) = outcome {
            if keep {
                warn(
                    cli,
                    &i18n::t_args(
                        "test-kept-failure",
                        &[
                            ("path", &dir.display().to_string()),
                            ("error", &error.to_string()),
                        ],
                    ),
                );
            } else if let Err(wipe_error) = staging::wipe(&dir) {
                warn(
                    cli,
                    &i18n::t_args(
                        "test-delete-failed",
                        &[
                            ("path", &dir.display().to_string()),
                            ("error", &wipe_error.to_string()),
                        ],
                    ),
                );
            } else {
                detail(cli, &i18n::t("test-cleaned-failure"));
            }
            return Err(error);
        }

        step(
            cli,
            index as usize + 5,
            total * chosen.len(),
            &i18n::t("test-cleanup"),
        );
        if keep {
            say(
                cli,
                &i18n::t_args("test-kept", &[("path", &dir.display().to_string())]),
            );
        } else if let Err(e) = staging::wipe(&dir) {
            warn(
                cli,
                &i18n::t_args(
                    "test-delete-failed",
                    &[
                        ("path", &dir.display().to_string()),
                        ("error", &e.to_string()),
                    ],
                ),
            );
        } else {
            detail(cli, &i18n::t("test-deleted"));
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
        .with_context(|| i18n::t_args("fetch-no-url", &[("project", pkg.project.as_str())]))?;
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

fn matches_entry(entry: &str, candidates: &[(String, String, String, String)]) -> Option<usize> {
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
            rows.push(vec![slug.clone(), entry.clone(), i18n::t("sync-on-name")]);
            continue;
        }
        let here = on_disk.iter().find(|(n, _)| *n == bare);
        let sha = here.and_then(|(_, path)| build::sha1_of(path));
        if let Some(h) = &sha {
            if let Some(i) = candidates.iter().position(|(_, _, _, locked)| locked == h) {
                let (_, _, slug, _) = &candidates[i];
                on.insert(slug.clone());
                matched_files.insert(bare);
                rows.push(vec![slug.clone(), entry.clone(), i18n::t("sync-on-sha")]);
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
                    i18n::t("sync-on-hash"),
                ]);
            }
            None => rows.push(vec![
                i18n::t("sync-not-on-modrinth"),
                entry.clone(),
                if here.is_some() {
                    i18n::t("sync-on-local")
                } else {
                    i18n::t("sync-on-unchecked")
                },
            ]),
        }
    }

    if !no_download {
        step(cli, 1, 2, &i18n::t("sync-step-download"));
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
                    bad.push(i18n::t_args(
                        "sync-fetch-failed",
                        &[
                            ("project", pkg.project.as_str()),
                            ("file", file.as_str()),
                            ("error", &format!("{e:#}")),
                        ],
                    ));
                    continue;
                }
                if pkg.kind == PkgType::Resourcepack {
                    if let Some(format) = build::pack_format_for(&target.minecraft) {
                        let ok = std::fs::read(&out)
                            .map(|b| crate::mcmeta::zip_supports(&b, format))
                            .unwrap_or(false);
                        if !ok {
                            bad.push(i18n::t_args(
                                "sync-no-verify",
                                &[
                                    ("project", pkg.project.as_str()),
                                    ("mc", target.minecraft.as_str()),
                                ],
                            ));
                            continue;
                        }
                    }
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
            say(
                cli,
                &i18n::t_args("sync-fetched", &[("count", &n.to_string())]),
            );
        }
    }

    for (name, _) in &on_disk {
        if !matched_files.contains(name) {
            rows.push(vec![
                i18n::t("sync-not-on-modrinth"),
                name.clone(),
                i18n::t("sync-off"),
            ]);
        }
    }
    for (_, file, slug, _) in &candidates {
        if !matched_files.contains(file) {
            rows.push(vec![
                slug.clone(),
                file.clone(),
                i18n::t("sync-off-unlisted"),
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
            let next = if want {
                Some(RpPosition::default())
            } else {
                None
            };
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

    let mut generated: Vec<String> = on_disk.iter().map(|(n, _)| n.clone()).collect();
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
        &i18n::t_args(
            "sync-read-options",
            &[("path", &options_path.display().to_string())],
        ),
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
        &i18n::t_args(
            "sync-changed",
            &[
                ("count", &changed.to_string()),
                ("suffix", if changed == 1 { "e" } else { "es" }),
            ],
        ),
    );
    say(
        cli,
        &i18n::t_args(
            "sync-on-list",
            &[(
                "list",
                &if on.is_empty() {
                    i18n::t("sync-nothing")
                } else {
                    on.iter().cloned().collect::<Vec<_>>().join(", ")
                },
            )],
        ),
    );
    say(
        cli,
        &i18n::t_args(
            "sync-up-to-date",
            &[("manifest", MANIFEST_FILE), ("lock", LOCK_FILE)],
        ),
    );
    say(
        cli,
        &i18n::t_args(
            "sync-wrote-options",
            &[("path", &options_path.display().to_string())],
        ),
    );
    for line in canonical
        .lines()
        .filter(|l| l.starts_with("resourcePacks:"))
    {
        println!("  {line}");
    }
    say(cli, &i18n::t("sync-rewrites"));
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
        say(cli, &i18n::t("fix-no-lock"));
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
            let pinned =
                p.manifest.packages[idx].version_for(&target.minecraft, &target.loader.kind);
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
                    i18n::t("fix-skipped"),
                    i18n::t("fix-not-target"),
                ]);
                continue;
            }
            let Ok(project) = client.project(&slug).await else {
                rows.push(vec![slug, "?".to_string(), i18n::t("fix-not-modrinth")]);
                continue;
            };
            let is_mod = p.manifest.packages[idx].kind == PkgType::Mod;
            let want = if is_mod { loaders.clone() } else { Vec::new() };
            let Ok(list) = client
                .versions(&project, Some(&target.minecraft), &want)
                .await
            else {
                rows.push(vec![
                    slug,
                    "?".to_string(),
                    i18n::t("update-reason-no-versions"),
                ]);
                continue;
            };
            if list.is_empty() {
                rows.push(vec![
                    slug,
                    locked
                        .clone()
                        .or(pinned.clone())
                        .unwrap_or_else(|| "*".to_string()),
                    if is_mod {
                        i18n::t_args(
                            "update-reason-no-build",
                            &[
                                ("loader", target.loader.kind.as_str()),
                                ("mc", target.minecraft.as_str()),
                            ],
                        )
                    } else {
                        i18n::t_args("fix-no-build", &[("mc", target.minecraft.as_str())])
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
        &i18n::t_args(
            "fix-versions-count",
            &[
                ("count", &changed.to_string()),
                ("suffix", if changed == 1 { "" } else { "s" }),
            ],
        ),
    );
    if rows.is_empty() {
        say(cli, &i18n::t("fix-matches-loader"));
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
        say(cli, &i18n::t("update-nothing-written"));
        return Ok(());
    }
    if changed == 0 {
        return Ok(());
    }
    p.manifest.write(&p.dir)?;
    say(cli, &i18n::t_args("wrote-file", &[("file", MANIFEST_FILE)]));
    let go = inquire::Confirm::new(&i18n::t("fix-lock-confirm"))
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
        say(cli, &i18n::t("verify-no-lock-first"));
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
                        i18n::t("verify-no-url"),
                    ]);
                    bad.push(i18n::t_args(
                        "verify-has-no-url",
                        &[("project", pkg.project.as_str())],
                    ));
                    continue;
                }
            };
            let body = match client.get_bytes(&url).await {
                Ok(b) => b,
                Err(e) => {
                    rows.push(vec![
                        pkg.project.clone(),
                        pkg.version_number.clone(),
                        i18n::t_args("verify-download-failed", &[("error", &e.to_string())]),
                    ]);
                    bad.push(i18n::t_args(
                        "verify-could-not-download",
                        &[("project", pkg.project.as_str()), ("error", &e.to_string())],
                    ));
                    if !keep_going {
                        steps.finish();
                        error_line(cli, &format!("  {e}"));
                        bail!("{}", i18n::t("verify-stop-first"));
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
                        let known = p
                            .manifest
                            .packages
                            .iter()
                            .find(|m| m.key().eq_ignore_ascii_case(&pkg.project));
                        if let Some(pkg_known) = known {
                            if let Some(list) = &pkg_known.ids {
                                if !list.is_empty()
                                    && !list.iter().any(|k| k.eq_ignore_ascii_case(&meta.id))
                                {
                                    wrong_id.push(i18n::t_args(
                                        "verify-wrong-id",
                                        &[
                                            ("project", pkg.project.as_str()),
                                            ("id", meta.id.as_str()),
                                            ("known", &list.join(", ")),
                                        ],
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
                            detail: i18n::t_args(
                                "verify-wrong-loader",
                                &[
                                    ("project", pkg.project.as_str()),
                                    ("version", pkg.version_number.as_str()),
                                    ("loaders", &loaders.join("/")),
                                    ("loader", target.loader.kind.as_str()),
                                ],
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
                            detail: i18n::t_args(
                                "verify-wrong-mc",
                                &[
                                    ("project", pkg.project.as_str()),
                                    ("version", pkg.version_number.as_str()),
                                    ("mc", target.minecraft.as_str()),
                                ],
                            ),
                        });
                    }
                }
            }
            if pkg.kind == PkgType::Resourcepack {
                if let Some(format) = build::pack_format_for(&target.minecraft) {
                    if !crate::mcmeta::zip_supports(&body, format) {
                        let msg = i18n::t_args(
                            "sync-no-verify",
                            &[
                                ("project", pkg.project.as_str()),
                                ("mc", target.minecraft.as_str()),
                            ],
                        );
                        rows.push(vec![
                            pkg.project.clone(),
                            pkg.version_number.clone(),
                            msg.clone(),
                        ]);
                        bad.push(msg.clone());
                        if !keep_going {
                            steps.finish();
                            error_line(cli, &msg);
                            bail!("{}", i18n::t("verify-stop-first"));
                        }
                        continue;
                    }
                }
            }
            match staging::verify(&pkg.project, &body, &pkg.hashes.sha1, &pkg.hashes.sha512) {
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
                        error_line(cli, &format!("  {e}"));
                        bail!("{}", i18n::t("verify-stop-first"));
                    }
                }
            }
        }
    }
    steps.finish();

    println!();
    say(cli, &i18n::t("verify-checked-hashes"));
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
            &i18n::t_args(
                "verify-identity-count",
                &[
                    ("count", &wrong_id.len().to_string()),
                    ("suffix", if wrong_id.len() == 1 { "" } else { "s" }),
                ],
            ),
        );
        for line in &wrong_id {
            error_line(cli, line);
        }
        bail!("{}", i18n::t("verify-identity-bail"));
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
            &i18n::t_args(
                "verify-soft-conflicts",
                &[
                    ("count", &warnings.len().to_string()),
                    ("suffix", if warnings.len() == 1 { "" } else { "s" }),
                ],
            ),
        );
        for (label, c) in &warnings {
            println!(
                "  {}",
                i18n::t_args(
                    "verify-soft-row",
                    &[
                        ("label", label.as_str()),
                        ("offender", c.offender.as_str()),
                        ("other", c.other.as_str()),
                        ("wanted", c.wanted.as_str()),
                        ("found", c.found.as_str()),
                    ],
                )
            );
        }
    }

    if !errors.is_empty() {
        println!();
        say(
            cli,
            &i18n::t_args(
                "verify-real-conflicts",
                &[
                    ("count", &errors.len().to_string()),
                    ("suffix", if errors.len() == 1 { "" } else { "s" }),
                ],
            ),
        );
        for (label, c) in &errors {
            error_line(
                cli,
                &format!(
                    "  {}",
                    i18n::t_args(
                        "verify-real-row",
                        &[
                            ("label", label.as_str()),
                            ("offender", c.offender.as_str()),
                            ("kind", c.kind),
                            ("other", c.other.as_str()),
                            ("wanted", c.wanted.as_str()),
                            ("found", c.found.as_str()),
                        ],
                    )
                ),
            );
        }
    }

    if !version_conflicts.is_empty() {
        println!();
        say(
            cli,
            &i18n::t_args(
                "verify-version-conflicts",
                &[
                    ("count", &version_conflicts.len().to_string()),
                    (
                        "suffix",
                        if version_conflicts.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                    ),
                ],
            ),
        );
        for v in &version_conflicts {
            error_line(
                cli,
                &format!(
                    "  {}",
                    i18n::t_args(
                        "verify-version-row",
                        &[("label", v.label.as_str()), ("detail", v.detail.as_str())]
                    )
                ),
            );
        }
    }

    if bad.is_empty() && errors.is_empty() && warnings.is_empty() && version_conflicts.is_empty() {
        println!();
        say(
            cli,
            &i18n::t_args("verify-all-ok", &[("count", &n.to_string())]),
        );
        return Ok(());
    }

    println!();
    if !bad.is_empty() {
        for line in &bad {
            error_line(cli, &format!("  {line}"));
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

    if !std::io::stdin().is_terminal() {
        say(cli, &i18n::t("verify-no-terminal"));
        let hard = bad.len() + errors.len() + version_conflicts.len();
        if hard > 0 {
            bail!(
                "{}",
                i18n::t_args("verify-hard-no-terminal", &[("count", &hard.to_string())])
            );
        }
        return Ok(());
    }

    let fix = inquire::Confirm::new(&i18n::t("fix-confirm"))
        .with_default(true)
        .prompt()
        .map_err(ask_failed)?;
    if !fix {
        say(
            cli,
            &i18n::t_args(
                "verify-skip-conflicts",
                &[
                    ("version", &version_conflicts.len().to_string()),
                    ("real", &errors.len().to_string()),
                    ("soft", &warnings.len().to_string()),
                    ("bad", &bad.len().to_string()),
                ],
            ),
        );
        let hard = bad.len() + errors.len() + version_conflicts.len();
        if hard > 0 {
            bail!(
                "{}",
                i18n::t_args("verify-hard-fail", &[("count", &hard.to_string())])
            );
        }
        return Ok(());
    }

    let label = targets.first().map(|t| t.label()).unwrap_or_default();
    let fix_target = targets
        .first()
        .map(|t| (t.minecraft.clone(), t.loader.kind.clone()));
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
                    warn(
                        cli,
                        &i18n::t_args("verify-locked-slug", &[("slug", slug.as_str())]),
                    );
                } else {
                    set_pin(&mut p.manifest, &slug, &key, &version, targets.len() == 1);
                    fixes.push(i18n::t_args(
                        "verify-fix-row",
                        &[
                            ("label", err_label.as_str()),
                            ("other", c.other.as_str()),
                            ("version", version.as_str()),
                            ("key", key.as_str()),
                            ("range", range),
                        ],
                    ));
                }
            }
            Err(e) => warn(
                cli,
                &i18n::t_args(
                    "verify-could-not-satisfy",
                    &[
                        ("other", c.other.as_str()),
                        ("wanted", c.wanted.as_str()),
                        ("error", &format!("{e:#}")),
                    ],
                ),
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
                    warn(
                        cli,
                        &i18n::t_args("verify-locked-slug", &[("slug", slug.as_str())]),
                    );
                } else {
                    set_pin(&mut p.manifest, &slug, &key, &version, targets.len() == 1);
                    warn_fixes.push(i18n::t_args(
                        "verify-warn-fix-row",
                        &[
                            ("label", warn_label.as_str()),
                            ("slug", slug.as_str()),
                            ("version", version.as_str()),
                            ("key", key.as_str()),
                            ("found", c.found.as_str()),
                            ("wanted", c.wanted.as_str()),
                        ],
                    ));
                }
            }
            Err(e) => warn(
                cli,
                &i18n::t_args(
                    "verify-could-not-step",
                    &[
                        ("other", c.other.as_str()),
                        ("wanted", c.wanted.as_str()),
                        ("error", &format!("{e:#}")),
                    ],
                ),
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
            warn(
                cli,
                &i18n::t_args(
                    "verify-fix-not-modrinth",
                    &[("project", v.project.as_str())],
                ),
            );
            continue;
        };
        let Ok(list) = client
            .versions(&project, Some(&t.minecraft), &loaders)
            .await
        else {
            warn(
                cli,
                &i18n::t_args(
                    "verify-fix-version-list-failed",
                    &[("project", v.project.as_str())],
                ),
            );
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
                &i18n::t_args("verify-locked-disabled", &[("project", v.project.as_str())]),
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
            vc_fixes.push(i18n::t_args(
                "verify-vc-same",
                &[
                    ("label", v.label.as_str()),
                    ("project", v.project.as_str()),
                    ("loader", v.loader.as_str()),
                ],
            ));
        } else {
            vc_fixes.push(i18n::t_args(
                "verify-vc-different",
                &[
                    ("label", v.label.as_str()),
                    ("loader", v.loader.as_str()),
                    ("version", chosen.version_number.as_str()),
                ],
            ));
        }
    }
    if !fixes.is_empty() || !warn_fixes.is_empty() || !vc_fixes.is_empty() {
        p.manifest.write(&p.dir)?;
        if !fixes.is_empty() {
            let count = fixes.len();
            say(
                cli,
                &i18n::t_args("verify-packages-adjusted", &[("count", &count.to_string())]),
            );
            for line in &fixes {
                println!("  {line}");
            }
        }
        if !warn_fixes.is_empty() {
            let count = warn_fixes.len();
            say(
                cli,
                &i18n::t_args("verify-soft-fixed", &[("count", &count.to_string())]),
            );
            for line in &warn_fixes {
                println!("  {line}");
            }
        }
        if !vc_fixes.is_empty() {
            let count = vc_fixes.len();
            say(
                cli,
                &i18n::t_args("verify-version-fixed", &[("count", &count.to_string())]),
            );
            for line in &vc_fixes {
                println!("  {line}");
            }
        }
    }
    say(
        cli,
        &i18n::t_args("verify-locking-again", &[("label", label.as_str())]),
    );
    match &fix_target {
        Some((mc, kind)) => lock_cmd(cli, api, Some(mc), LoaderKind::parse(kind), false).await,
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
            &i18n::t_args(
                "candidate-wrong-id",
                &[("slug", slug.as_str()), ("mod_id", mod_id)],
            ),
        );
    }
    let any_mc = i18n::t("candidate-any-mc");
    let mc_label = mc.unwrap_or(&any_mc);
    if seen_candidate {
        bail!(
            "{}",
            i18n::t_args(
                "candidate-no-build",
                &[("mod_id", mod_id), ("mc", mc_label)]
            )
        );
    }
    bail!(
        "{}",
        i18n::t_args(
            "candidate-no-project",
            &[("mod_id", mod_id), ("range", range), ("mc", mc_label),]
        )
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
            &i18n::t_args(
                "candidate-wrong-id",
                &[("slug", slug.as_str()), ("mod_id", mod_id)],
            ),
        );
    }
    if seen_candidate {
        bail!(
            "{}",
            i18n::t_args(
                "candidate-no-build-outside",
                &[("mod_id", mod_id), ("range", range)]
            )
        );
    }
    let any_mc = i18n::t("candidate-any-mc");
    let mc_label = mc.unwrap_or(&any_mc);
    bail!(
        "{}",
        i18n::t_args(
            "candidate-no-version-outside",
            &[("mod_id", mod_id), ("range", range), ("mc", mc_label),]
        )
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
        version: if single {
            Some(version.to_string())
        } else {
            None
        },
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
        error_line(cli, &format!("  {line}"));
    }
    if report.ok() {
        say(
            cli,
            &i18n::t_args(
                "report-ok",
                &[
                    ("count", &report.warnings.len().to_string()),
                    ("suffix", if report.warnings.len() == 1 { "" } else { "s" }),
                ],
            ),
        );
        return Ok(());
    }
    bail!(
        "{}",
        i18n::t_args(
            "report-failed",
            &[
                ("count", &report.errors.len().to_string()),
                ("suffix", if report.errors.len() == 1 { "" } else { "s" }),
            ]
        )
    )
}

fn modrinth_gave_up(why: &str) -> anyhow::Error {
    anyhow::anyhow!(crate::modrinth::with_details(
        &i18n::t("modrinth-gave-up"),
        why
    ))
}

const PAT_PAGE: &str = "https://modrinth.com/settings/pats";

fn explain_pat() {
    println!("{}", i18n::t("explain-pat-intro"));
    println!();
    println!("{}", i18n::t("explain-pat-required"));
    println!("{}", i18n::t("explain-pat-no-token"));
    println!(
        "{}",
        i18n::t_args(
            "explain-pat-scopes",
            &[("scopes", &i18n::t("token-scopes-needed"))]
        )
    );
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
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .is_ok()
    };
    if spawned {
        Ok(())
    } else {
        bail!("{}", i18n::t_args("open-browser-failed", &[("url", url)]))
    }
}

fn ask_token_first_time(explained: &mut bool) -> Result<String> {
    if !*explained {
        explain_pat();
        *explained = true;
    }
    loop {
        let menu = vec![i18n::t("token-menu-have"), i18n::t("token-menu-no")];
        let picked = inquire::Select::new(&i18n::t("token-have-prompt"), menu.clone())
            .with_starting_cursor(0)
            .prompt()
            .map_err(ask_failed)?;
        if picked == menu[1] {
            println!();
            println!(
                "{}",
                i18n::t_args(
                    "token-make",
                    &[
                        ("url", PAT_PAGE),
                        ("scopes", &i18n::t("token-scopes-needed"))
                    ]
                )
            );
            println!();
            ask(&i18n::t("token-press-enter"))?;
            let _ = open_browser(PAT_PAGE);
            println!("{}", i18n::t_args("token-opened", &[("url", PAT_PAGE)]));
            println!();
            continue;
        }
        return inquire::Password::new(&format!("{} ", i18n::t("token-password-prompt")))
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
                    detail(cli, &i18n::t("token-logging-in"));
                    return Ok(t);
                }
                detail(cli, &i18n::t("token-saved-invalid"));
                forget_secret(cli);
            }
            Ok(None) => {}
            Err(e) => {
                detail(cli, &e.to_string());
                detail(cli, &i18n::t("token-unrecoverable"));
                forget_secret(cli);
            }
        }
    }
    loop {
        let token = ask_token_first_time(explained)?.trim().to_string();
        if token.is_empty() {
            bail!("{}", i18n::t("token-not-a-token"));
        }
        if crate::modrinth::token_looks_wrong(&token) {
            detail(cli, &i18n::t("token-wrong-prefix"));
            continue;
        }
        detail(cli, &i18n::t("token-saving"));
        secret::save(&token)?;
        return Ok(token);
    }
}

fn forget_secret(cli: &Cli) {
    if let Err(e) = secret::clear() {
        detail(
            cli,
            &i18n::t_args("token-clear-failed", &[("error", &e.to_string())]),
        );
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
        detail(
            cli,
            &i18n::t_args(
                "project-id-using",
                &[("id", id.as_str()), ("file", MANIFEST_FILE)],
            ),
        );
        return Ok(id);
    }
    let id = ask(&i18n::t("project-id-prompt"))?.trim().to_string();
    if id.is_empty() {
        bail!("{}", i18n::t("project-id-empty"));
    }
    let mut pack = p.manifest.clone();
    pack.modrinth_project_id = Some(id.clone());
    pack.write(&p.dir)?;
    detail(
        cli,
        &i18n::t_args(
            "project-id-saved",
            &[("id", id.as_str()), ("file", MANIFEST_FILE)],
        ),
    );
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
        bail!(
            "{}",
            i18n::t_args("publish-no-version", &[("file", MANIFEST_FILE)])
        );
    }
    let top = p
        .manifest
        .top
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();

    step(cli, 1, 5, &i18n::t("publish-step-lock"));
    if !Lock::exists(&p.dir) {
        lock_cmd(cli, api, None, None, false).await?;
    }
    let mut lock = Lock::load(&p.dir)?;
    if lock.manifest_hash != p.manifest.fingerprint() {
        detail(
            cli,
            &i18n::t_args("publish-manifest-changed", &[("file", MANIFEST_FILE)]),
        );
        lock_cmd(cli, api, None, None, false).await?;
        lock = Lock::load(&p.dir)?;
    }
    if let Some(want) = mc {
        let only = manifest::pick(&p.softwares.targets, Some(want), loader)?;
        lock.targets
            .retain(|t| t.minecraft == only.minecraft && t.loader.kind == only.loader.kind);
    } else if let Some(l) = loader {
        lock.targets.retain(|t| t.loader.kind == l.as_str());
    }
    if lock.targets.is_empty() {
        bail!("{}", i18n::t("publish-no-targets"));
    }

    step(cli, 2, 5, &i18n::t("publish-step-build"));
    let sources = override_sources(&p);
    let reused = build::stamp_fresh(out, &lock, &sources);
    let mut built_paths: Vec<PathBuf> = Vec::new();
    if reused {
        detail(cli, &i18n::t("publish-reuse"));
    }
    let groups = group_by_mc(&lock.targets);
    let mut releases: Vec<Release> = Vec::new();
    for (mc_ver, targets) in &groups {
        for t in targets {
            let built = if reused {
                out.join(format!("{}.mrpack", build::output_name(&lock, t)))
            } else {
                detail(
                    cli,
                    &i18n::t_args(
                        "publish-building-for",
                        &[
                            ("mc", t.minecraft.as_str()),
                            ("loader", t.loader.kind.as_str()),
                        ],
                    ),
                );
                let path = build::build(&lock, t, out, &sources)?;
                built_paths.push(path.clone());
                path
            };
            let name = built
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{}.mrpack", p.manifest.name));
            let size = std::fs::metadata(&built).map(|m| m.len()).unwrap_or(0);
            detail(
                cli,
                &format!(
                    "  {}",
                    i18n::t_args(
                        "publish-built-file",
                        &[("name", name.as_str()), ("size", &size.to_string())]
                    )
                ),
            );
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
    if !reused {
        if let Err(e) = build::write_stamp(out, &lock, &sources, &built_paths) {
            detail(
                cli,
                &i18n::t_args("publish-stamp-failed", &[("error", &format!("{e:#}"))]),
            );
        }
    }
    if releases.is_empty() {
        bail!("{}", i18n::t("publish-nothing-built"));
    }

    step(cli, 2, 4, &i18n::t("publish-step-token"));
    let client = Modrinth::new(api)?;
    let mut explained = false;
    let mut token = resolve_token(cli, token_arg, &mut explained)?;
    if crate::modrinth::token_looks_wrong(&token) {
        bail!("{}", i18n::t("token-wrong-prefix"));
    }
    match client.whoami(&token).await {
        Ok(Ok(u)) => say(
            cli,
            &i18n::t_args("publish-signed-in", &[("user", u.as_str())]),
        ),
        _ => detail(cli, &i18n::t("publish-whoami-failed")),
    }

    let project_id = resolve_project_id(cli, project_arg, &p)?;

    step(cli, 3, 4, &i18n::t("publish-step-project"));
    if !client.project_exists(&token, &project_id).await? {
        bail!("{}", i18n::t("publish-project-gone"));
    }
    detail(
        cli,
        &i18n::t_args("publish-project-exists", &[("id", project_id.as_str())]),
    );

    let existing = client.project_versions(&token, &project_id).await?;
    let old_featured: Vec<String> = existing
        .iter()
        .filter(|v| v.featured)
        .map(|v| v.version_number.clone())
        .collect();
    let environment = env_for(&p);
    let changelog = latest_changelog(&p.dir);

    println!();
    say(cli, &i18n::t("publish-about-to"));
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
        let star = if r.minecraft == top {
            format!("  {}", i18n::t("publish-featured-tag"))
        } else {
            String::new()
        };
        println!("  {}  {}{star}", id, r.loaders.join(", "));
        for (_, n) in &r.files {
            println!("      {n}");
        }
    }
    if !changelog.is_empty() {
        println!(
            "  {}",
            i18n::t_args(
                "publish-changelog-lines",
                &[("count", &changelog.lines().count().to_string())]
            )
        );
    }
    for v in &old_featured {
        println!(
            "  {}",
            i18n::t_args("publish-unfeaturing", &[("id", v.as_str())])
        );
    }
    println!();

    for pack in &p.manifest.external_packs {
        if pack.is_builtin() || !pack.is_active() {
            continue;
        }
        warn(
            cli,
            &i18n::t_args(
                "publish-pack-withheld-warning",
                &[("name", pack.name.as_str())],
            ),
        );
    }

    step(cli, 4, 5, &i18n::t("publish-step-upload"));
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
            &i18n::t_args(
                "publish-upload-progress",
                &[
                    ("current", &(i + 1).to_string()),
                    ("total", &total.to_string()),
                    ("version", version_number.as_str()),
                ],
            ),
        );
        for (_, n) in &r.files {
            detail(cli, &format!("        {n}"));
        }
        let created = loop {
            match client.create_version(&token, &body, &r.files).await? {
                Ok(v) => break v,
                Err(AuthFailure::BadScopes) => bail!("{}", i18n::t("publish-bad-scopes")),
                Err(AuthFailure::Invalid(why)) => {
                    warn(
                        cli,
                        &crate::modrinth::with_details(&i18n::t("publish-bad-token"), &why),
                    );
                    if token_arg.is_some() {
                        bail!("{}", i18n::t("publish-token-failed"));
                    }
                }
                Err(AuthFailure::Expired(why)) => {
                    warn(
                        cli,
                        &crate::modrinth::with_details(&i18n::t("publish-expired-token"), &why),
                    );
                    if token_arg.is_some() {
                        bail!("{}", i18n::t("publish-token-failed"));
                    }
                }
                Err(AuthFailure::Other(e)) if e.contains("not found") || e.contains("404") => {
                    bail!(
                        "{}",
                        i18n::t_args("publish-no-permission", &[("id", project_id.as_str())])
                    );
                }
                Err(AuthFailure::Other(e)) => bail!("{}", modrinth_gave_up(&e)),
            }
            let _ = secret::clear();
            token = ask_token_first_time(&mut explained)?.trim().to_string();
            if token.is_empty() {
                bail!("{}", i18n::t("token-not-a-token"));
            }
            if crate::modrinth::token_looks_wrong(&token) {
                bail!("{}", i18n::t("token-wrong-prefix"));
            }
            detail(cli, &i18n::t("token-saving"));
            secret::save(&token)?;
            detail(
                cli,
                &i18n::t_args(
                    "publish-retry-version",
                    &[("version", version_number.as_str())],
                ),
            );
        };
        detail(
            cli,
            &format!(
                "        {}",
                i18n::t_args("publish-created", &[("id", created.id.as_str())])
            ),
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

    step(cli, 5, 5, &i18n::t("publish-step-featured"));
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
                detail(cli, &i18n::t("publish-unfeatured"));
                unfeatured += 1;
            }
            Err(AuthFailure::BadScopes) => bail!("{}", i18n::t("publish-bad-scopes")),
            Err(_) => warn(cli, &i18n::t("publish-unfeature-failed")),
        }
    }

    println!();
    say(
        cli,
        &i18n::t_args("publish-summary", &[("count", &made.len().to_string())]),
    );
    println!();
    for (mc_ver, url, feat) in &made {
        if *feat {
            println!(
                "{}",
                i18n::t_args(
                    "publish-summary-row",
                    &[("mc", mc_ver.as_str()), ("url", url.as_str())]
                )
            );
        } else {
            println!(
                "{}",
                i18n::t_args(
                    "publish-summary-row-plain",
                    &[("mc", mc_ver.as_str()), ("url", url.as_str())]
                )
            );
        }
    }
    if unfeatured > 0 {
        println!();
        detail(
            cli,
            &i18n::t_args(
                "publish-unfeatured-count",
                &[("count", &unfeatured.to_string())],
            ),
        );
    }
    if !created_ids.is_empty() {
        println!();
        let perms_link = format!(
            "https://modrinth.com/project/{}/settings/permissions",
            project_id
        );
        match client.project_versions_v3(&token, &project_id).await {
            Ok(listed) => {
                let mut withheld: Vec<String> = Vec::new();
                for id in &created_ids {
                    match listed.iter().find(|v| &v.id == id) {
                        Some(v) if !v.files_missing_attribution.is_empty() => {
                            withheld.push(if v.version_number.is_empty() {
                                v.id.clone()
                            } else {
                                v.version_number.clone()
                            })
                        }
                        None => withheld.push(id.clone()),
                        _ => {}
                    }
                }
                if withheld.len() == created_ids.len() {
                    warn(
                        cli,
                        &i18n::t_args("publish-all-withheld", &[("url", perms_link.as_str())]),
                    );
                } else if !withheld.is_empty() {
                    warn(
                        cli,
                        &i18n::t_args(
                            "publish-some-withheld",
                            &[("list", &withheld.join(", ")), ("url", perms_link.as_str())],
                        ),
                    );
                }
            }
            Err(e) => {
                detail(
                    cli,
                    &i18n::t_args(
                        "publish-withheld-check-failed",
                        &[("error", &format!("{e:#}"))],
                    ),
                );
            }
        }
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
        say(cli, &i18n::t("order-no-lock-first"));
        lock_cmd(
            cli,
            &cli.api.clone().unwrap_or_else(|| DEFAULT_API.to_string()),
            None,
            None,
            false,
        )
        .await?;
        Lock::load(&p.dir)?
    };
    println!();
    print_report(
        cli,
        &validate::check(&lock, &p.manifest, &override_sources(&p)),
    )?;
    check_schema(cli, &p);
    Ok(())
}

fn java_cmd(cli: &Cli) -> Result<()> {
    println!();
    let javas = crate::minecraft::all_javas();
    if javas.is_empty() {
        say(cli, &i18n::t("java-none-anywhere"));
        detail(cli, &i18n::t("java-looked-at"));
        return Ok(());
    }
    say(
        cli,
        &i18n::t_args(
            "java-found-count",
            &[
                ("count", &javas.len().to_string()),
                ("suffix", if javas.len() == 1 { "" } else { "s" }),
            ],
        ),
    );
    println!();
    let rows: Vec<Vec<String>> = javas
        .iter()
        .map(|j| {
            vec![
                format!("java {}", j.version),
                j.vendor.clone(),
                j.exe.display().to_string(),
            ]
        })
        .collect();
    let dimmed = vec![false; rows.len()];
    println!(
        "{}",
        table(
            cli,
            &[
                i18n::t("header-version"),
                i18n::t("header-vendor"),
                i18n::t("header-path"),
            ],
            &rows,
            &dimmed,
        )
    );
    if let Ok(p) = load_project(cli) {
        println!();
        say(cli, &i18n::t("java-target-wants"));
        for target in &p.softwares.targets {
            let want = crate::minecraft::required_java(&target.minecraft, target.java.as_deref());
            match javas.iter().find(|j| j.major >= want) {
                Some(j) => detail(
                    cli,
                    &i18n::t_args(
                        "java-target-ok",
                        &[
                            ("target", target.label().as_str()),
                            ("want", &want.to_string()),
                            ("version", j.version.as_str()),
                            ("path", &j.exe.display().to_string()),
                        ],
                    ),
                ),
                None => warn(
                    cli,
                    &i18n::t_args(
                        "java-target-none",
                        &[
                            ("target", target.label().as_str()),
                            ("want", &want.to_string()),
                        ],
                    ),
                ),
            }
        }
    }
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
            bail!("{}", i18n::t("order-zero-invalid"));
        }
        return Ok(Some(RpPosition::At(n)));
    }
    match want.to_ascii_lowercase().as_str() {
        "top" => Ok(Some(RpPosition::Named(manifest::RpNamed::Top))),
        "bottom" => Ok(Some(RpPosition::Named(manifest::RpNamed::Bottom))),
        "off" | "none" => Ok(None),
        other => bail!(
            "{}",
            i18n::t_args(
                "order-position-invalid",
                &[("other", &format!("{other:?}"))]
            )
        ),
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
                            .map(|lp| lp.path.rsplit('/').next().unwrap_or(&lp.path).to_string())
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
        if p.manifest
            .external_packs
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(id))
        {
            continue;
        }
        out.push((id.to_string(), None));
    }
    sort_by_stack_order(&mut out);
    out
}

fn sort_by_stack_order(entries: &mut [(String, Option<RpPosition>)]) {
    entries.sort_by_key(|(_, position)| match position {
        Some(sitting) => (0u8, sitting.rank()),
        None => (1, u32::MAX),
    });
}

fn print_order(cli: &Cli, p: &Project, lock: Option<&Lock>, targets: &[Target]) {
    for target in targets {
        say(
            cli,
            &i18n::t_args(
                "order-resource-pack-order",
                &[("label", Targetish::label(target).as_str())],
            ),
        );
        let entries = order_entries(p, target, lock);
        if entries.is_empty() {
            detail(cli, &i18n::t("order-nothing-active"));
            continue;
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|(name, position)| vec![order_pos_label(*position), name.clone()])
            .collect();
        println!(
            "{}",
            table(
                cli,
                &[i18n::t("header-top"), i18n::t("header-pack")],
                &rows,
                &vec![false; rows.len()],
            )
        );
    }
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
            bail!(
                "{}",
                i18n::t_args(
                    "order-pos-needed-token",
                    &[("token", &format!("{token:?}"))]
                )
            );
        }
    }
    Ok(out)
}

fn order_save() -> String {
    i18n::t("order-save")
}

fn order_clear() -> String {
    i18n::t("order-clear-menu")
}

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
                    (true, true) => format!("   {}", i18n::t("order-builtin")),
                    (true, false) => format!("   {}", i18n::t("order-builtin-off")),
                    (false, true) => String::new(),
                    (false, false) => format!("   {}", i18n::t("order-off")),
                }
            ),
            OrderEntry {
                name: pack.name.clone(),
                external: true,
            },
        ));
    }
    for (id, label) in &builtins {
        if p.manifest
            .external_packs
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(id))
        {
            continue;
        }
        out.push((
            format!(
                "{:<5} {label}   {}",
                order_pos_label(None),
                i18n::t("order-builtin")
            ),
            OrderEntry {
                name: id.to_string(),
                external: true,
            },
        ));
    }
    for file in on_disk {
        if p.manifest
            .external_packs
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(file))
        {
            continue;
        }
        out.push((
            format!(
                "{:<5} {file}   {}",
                order_pos_label(None),
                i18n::t("order-in-folder")
            ),
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
    shas: &BTreeMap<String, String>,
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
                if pack.sha1.is_none() && !pack.is_builtin() {
                    if let Some(h) = shas.get(&entry.name) {
                        pack.sha1 = Some(h.clone());
                    }
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
                    sha1: if builtin {
                        None
                    } else {
                        shas.get(&entry.name).cloned()
                    },
                };
                set_positions(&mut pack.position, &mut pack.positions, scope, position);
                p.manifest.external_packs.push(pack);
            }
        }
    }
    let where_ = if scope.iter().any(|k| k.is_none()) {
        i18n::t("order-every-target")
    } else {
        targets
            .iter()
            .map(Targetish::label)
            .collect::<Vec<_>>()
            .join(", ")
    };
    say(
        cli,
        &format!(
            "{} is {} on {}",
            entry.name,
            order_pos_label(position),
            where_
        ),
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
    let pack_shas = record_pack_shas(&p);

    if !std::io::stdin().is_terminal() {
        say(cli, &i18n::t("order-no-terminal"));
        let targets: Vec<Target> = match (mc, loader) {
            (None, None) => p.softwares.targets.clone(),
            _ => vec![manifest::pick(&p.softwares.targets, mc, loader)?.clone()],
        };
        print_order(cli, &p, lock.as_ref(), &targets);
        return Ok(());
    }

    let labels: Vec<String> = p.softwares.targets.iter().map(Targetish::label).collect();
    let (targets, scope): (Vec<Target>, Vec<Option<String>>) = if mc.is_some() || loader.is_some() {
        let t = manifest::pick(&p.softwares.targets, mc, loader)?.clone();
        let k = Some(order_target_key(mc, loader).unwrap_or_default());
        (vec![t], vec![k])
    } else {
        let options: Vec<String> = labels
            .iter()
            .cloned()
            .chain(std::iter::once(i18n::t("option-all-of-them")))
            .collect();
        let chosen: Vec<Target> = loop {
            let picked = inquire::MultiSelect::new(
                &i18n::t_args(
                    "order-targets-prompt",
                    &[
                        ("file", MANIFEST_FILE),
                        ("count", &labels.len().to_string()),
                    ],
                ),
                options.clone(),
            )
            .with_help_message(&i18n::t("order-targets-hint"))
            .prompt()
            .map_err(ask_failed)?;
            if picked.iter().any(|o| o == &i18n::t("option-all-of-them")) {
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
            detail(cli, &i18n::t("order-nothing-ticked"));
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
        i18n::t("order-every-target")
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
        &i18n::t_args("order-header", &[("scope", scope_label.as_str())]),
    );
    for target in &targets {
        say(
            cli,
            &i18n::t_args(
                "order-current",
                &[("value", Targetish::label(target).as_str())],
            ),
        );
        let entries = order_entries(&p, target, lock.as_ref());
        if entries.is_empty() {
            detail(cli, &i18n::t("order-nothing-active-yet"));
        } else {
            let rows: Vec<Vec<String>> = entries
                .iter()
                .map(|(name, position)| vec![order_pos_label(*position), name.clone()])
                .collect();
            println!(
                "{}",
                table(
                    cli,
                    &[i18n::t("header-top"), i18n::t("header-pack")],
                    &rows,
                    &vec![false; rows.len()],
                )
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
        menu.push(order_clear());
        menu.push(order_save());

        let picked = inquire::Select::new(&i18n::t("order-pick-pack"), menu.clone())
            .with_page_size(16)
            .with_help_message(&i18n::t("order-pick-pack-hint"))
            .prompt()
            .map_err(ask_failed)?;
        let Some(i) = menu.iter().position(|o| *o == picked) else {
            continue;
        };

        if picked == order_save() {
            break;
        }
        if picked == order_clear() {
            let yes = inquire::Confirm::new(&i18n::t_args(
                "order-clear-confirm",
                &[("scope", scope_label.as_str())],
            ))
            .with_default(false)
            .prompt()
            .map_err(ask_failed)?;
            if yes {
                let entries: Vec<OrderEntry> = order_choices(&p, &targets, lock.as_ref(), &on_disk)
                    .into_iter()
                    .map(|(_, e)| e)
                    .collect();
                for entry in entries {
                    set_order_on(&mut p, &entry, &scope, &targets, None, cli, &pack_shas);
                }
            }
            continue;
        }

        let entry = match choices.get(i) {
            Some((_, e)) => e.clone(),
            None => continue,
        };
        let label = i18n::t_args("order-position-prompt", &[("name", entry.name.as_str())]);
        let where_ = inquire::Select::new(&label, positions.clone())
            .prompt()
            .map_err(ask_failed)?;
        let position = parse_position(&where_)?;
        set_order_on(&mut p, &entry, &scope, &targets, position, cli, &pack_shas);
    }

    p.manifest.write(&p.dir)?;
    say(cli, &i18n::t_args("wrote-file", &[("file", MANIFEST_FILE)]));
    for target in &targets {
        let label = Targetish::label(target);
        println!();
        say(
            cli,
            &i18n::t_args("order-resource-pack-order", &[("label", label.as_str())]),
        );
        let entries = order_entries(&p, target, lock.as_ref());
        if entries.is_empty() {
            detail(cli, &i18n::t("order-nothing-active"));
            continue;
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|(name, position)| vec![order_pos_label(*position), name.clone()])
            .collect();
        println!(
            "{}",
            table(
                cli,
                &[i18n::t("header-top"), i18n::t("header-pack")],
                &rows,
                &vec![false; rows.len()],
            )
        );
    }
    println!();
    detail(cli, &i18n::t("order-see-in-pack"));
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
    let pack_shas = record_pack_shas(&p);

    for (name, raw) in order_specs(&packs, off)? {
        let position = parse_position(&raw)?;
        if position.is_some() && off {
            bail!("{}", i18n::t("order-off-no-position"));
        }
        if !off && position.is_none() {
            bail!(
                "{}",
                i18n::t_args("order-pos-needed-name", &[("name", &format!("{name:?}"))])
            );
        }

        let mut hit = false;
        for pkg in p.manifest.packages.iter_mut() {
            let file = lock.as_ref().and_then(|l| {
                targets.iter().find_map(|t| {
                    l.targets
                        .iter()
                        .find(|lt| lt.minecraft == t.minecraft && lt.loader.kind == t.loader.kind)
                        .and_then(|lt| {
                            lt.packages
                                .iter()
                                .find(|lp| lp.project.eq_ignore_ascii_case(&pkg.key()))
                                .map(|lp| {
                                    lp.path.rsplit('/').next().unwrap_or(&lp.path).to_string()
                                })
                        })
                })
            });
            let known =
                pkg.key().eq_ignore_ascii_case(&name) || file.as_deref() == Some(name.as_str());
            if !known || !matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader) {
                continue;
            }
            if position.is_some() && !pkg.is_active() {
                pkg.active = Some(true);
                say(
                    cli,
                    &i18n::t_args("order-active-now", &[("name", pkg.key().as_str())]),
                );
            }
            set_position(&mut pkg.position, &mut pkg.positions, &key, position);
            say(
                cli,
                &match &key {
                    Some(k) => i18n::t_args(
                        "order-is-at",
                        &[
                            ("name", pkg.key().as_str()),
                            ("position", show_position(position).as_str()),
                            ("key", k.as_str()),
                        ],
                    ),
                    None => i18n::t_args(
                        "order-is-everywhere",
                        &[
                            ("name", pkg.key().as_str()),
                            ("position", show_position(position).as_str()),
                        ],
                    ),
                },
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
                    say(
                        cli,
                        &i18n::t_args("order-active-now", &[("name", pack.name.as_str())]),
                    );
                }
                if pack.sha1.is_none() && !pack.is_builtin() {
                    if let Some(h) = pack_shas.get(&name) {
                        pack.sha1 = Some(h.clone());
                    }
                }
                set_position(&mut pack.position, &mut pack.positions, &key, position);
                say(
                    cli,
                    &match &key {
                        Some(k) => i18n::t_args(
                            "order-is-at",
                            &[
                                ("name", pack.name.as_str()),
                                ("position", show_position(position).as_str()),
                                ("key", k.as_str()),
                            ],
                        ),
                        None => i18n::t_args(
                            "order-is-everywhere",
                            &[
                                ("name", pack.name.as_str()),
                                ("position", show_position(position).as_str()),
                            ],
                        ),
                    },
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
                sha1: pack_shas.get(&name).cloned(),
            };
            set_position(&mut pack.position, &mut pack.positions, &key, position);
            p.manifest.external_packs.push(pack);
            say(
                cli,
                &i18n::t_args("order-added-external", &[("name", name.as_str())]),
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
            bail!(
                "{}",
                i18n::t_args(
                    "order-not-a-pack",
                    &[("name", &format!("{name:?}")), ("list", &known.join(", ")),]
                )
            );
        }
    }

    p.manifest.write(&p.dir)?;
    if !packs.is_empty() {
        say(cli, &i18n::t_args("wrote-file", &[("file", MANIFEST_FILE)]));
        detail(cli, &i18n::t("order-lock-build-hint"));
    }

    for target in &targets {
        let label = Targetish::label(target);
        println!();
        say(
            cli,
            &i18n::t_args("order-resource-pack-order", &[("label", label.as_str())]),
        );
        let entries = order_entries(&p, target, lock.as_ref());
        if entries.is_empty() {
            detail(cli, &i18n::t("order-nothing-active"));
            continue;
        }
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|(name, position)| vec![show_position(*position), name.clone()])
            .collect();
        println!(
            "{}",
            table(
                cli,
                &[i18n::t("header-top"), i18n::t("header-pack")],
                &rows,
                &vec![false; rows.len()],
            )
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

fn record_pack_shas(p: &Project) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, path) in build::pack_paths_on_disk(&override_sources(p)) {
        if path.is_file() {
            if let Some(h) = build::sha1_of(&path) {
                out.insert(name, h);
            }
        }
    }
    out
}

fn reconcile_external_packs(
    cli: &Cli,
    p: &mut Project,
    fresh: &BTreeMap<String, LockedTarget>,
) -> (usize, usize) {
    let shas = record_pack_shas(p);
    let mut modrinth_by_sha: BTreeMap<String, String> = BTreeMap::new();
    for target in fresh.values() {
        for pkg in &target.packages {
            if !matches!(pkg.kind, PkgType::Resourcepack | PkgType::Shader)
                || pkg.hashes.sha1.is_empty()
            {
                continue;
            }
            let file = pkg.path.rsplit('/').next().unwrap_or(&pkg.path).to_string();
            modrinth_by_sha
                .entry(pkg.hashes.sha1.clone())
                .or_insert_with(|| file);
        }
    }
    let mut renamed = 0usize;
    let mut learned = 0usize;
    for pack in p.manifest.external_packs.iter_mut() {
        if pack.is_builtin() {
            continue;
        }
        if let Some(sha) = shas.get(&pack.name) {
            if pack.sha1.is_none() {
                pack.sha1 = Some(sha.clone());
                learned += 1;
            }
            continue;
        }
        let Some(sha) = pack.sha1.clone() else {
            continue;
        };
        let same_name = shas
            .iter()
            .find(|(_, h)| *h == &sha)
            .map(|(n, _)| n.clone());
        let candidate = same_name.or_else(|| modrinth_by_sha.get(&sha).cloned());
        if let Some(new_name) = candidate {
            if new_name != pack.name {
                detail(
                    cli,
                    &format!("\"{}\" renamed to \"{}\", same pack", pack.name, new_name),
                );
                pack.name = new_name;
                renamed += 1;
            }
        }
    }
    (renamed, learned)
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
        bail!("{}", i18n::t("setver-empty"));
    }
    let mut p = load_project(cli)?;
    let current = p.manifest.version.trim().to_string();
    if current == want {
        say(
            cli,
            &i18n::t_args(
                "setver-already",
                &[("file", MANIFEST_FILE), ("version", want)],
            ),
        );
        return Ok(());
    }
    p.manifest.version = want.to_string();
    p.manifest.stamp();
    p.manifest.write(&p.dir)?;
    say(
        cli,
        &i18n::t_args("setver-now", &[("file", MANIFEST_FILE), ("version", want)]),
    );
    lock_cmd(cli, api, None, None, false).await?;
    say(
        cli,
        &i18n::t_args("setver-refreshed", &[("file", LOCK_FILE)]),
    );
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
        say(
            cli,
            &i18n::t_args(
                "setver-lock-schema",
                &[
                    ("file", LOCK_FILE),
                    ("version", &manifest::SCHEMA_VERSION.to_string()),
                ],
            ),
        );
    }
    let written = manifest::write_local_schemas(&p.dir)?;
    say(
        cli,
        &i18n::t_args(
            "setver-schemas-schema",
            &[
                ("file", MANIFEST_FILE),
                ("other", SOFTWARES_FILE),
                ("version", &manifest::SCHEMA_VERSION.to_string()),
            ],
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

fn error_line(cli: &Cli, text: &str) {
    if color_enabled(cli) {
        eprintln!("{}", text.red().bold());
    } else {
        eprintln!("{text}");
    }
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

fn device_code_notice(cli: &Cli, text: &str) {
    if text.starts_with("To sign in, use a web browser") {
        if color_enabled(cli) {
            println!("  {}", text.yellow());
        } else {
            println!("  {text}");
        }
        return;
    }
    if text == i18n::t("login-waiting") || text == "Waiting for sign-in..." || text.ends_with("...")
    {
        if color_enabled(cli) {
            println!("  {}", text.cyan());
        } else {
            println!("  {text}");
        }
        return;
    }
    let probe = i18n::t_args("login-go-to", &[("url", "\u{0}"), ("code", "\u{0}")]);
    let go_to = probe.split_once('\u{0}').and_then(|(prefix, rest)| {
        rest.split_once('\u{0}')
            .map(|(mid, suffix)| (prefix, mid, suffix))
    });
    if let Some((prefix, mid, suffix)) = go_to
        && let Some(rest) = text.strip_prefix(prefix)
        && let Some(without_suffix) = rest.strip_suffix(suffix)
        && let Some((url, code)) = without_suffix.split_once(mid)
        && url.contains("://")
        && !code.is_empty()
    {
        if !color_enabled(cli) {
            println!("  {text}");
        } else {
            println!(
                "  {} {} {} {}",
                prefix.trim().yellow().bold(),
                url.blue().underline(),
                mid.trim().yellow(),
                code.bold().cyan()
            );
        }
        return;
    }
    detail(cli, text);
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

fn table<S: AsRef<str>>(cli: &Cli, headers: &[S], rows: &[Vec<String>], dimmed: &[bool]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.as_ref().chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let head: Vec<String> = headers.iter().map(|h| h.as_ref().to_string()).collect();
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
        RpPosition, looks_like_position, order_specs, parse_position, set_positions,
        sort_by_stack_order,
    };
    use crate::manifest::RpNamed;
    use std::collections::BTreeMap;

    #[test]
    fn the_table_lists_the_top_of_the_stack_first() {
        let mut entries = vec![
            (
                "floor.zip".to_string(),
                Some(RpPosition::Named(RpNamed::Bottom)),
            ),
            ("top.zip".to_string(), Some(RpPosition::At(1))),
            ("off.zip".to_string(), None),
            ("second.zip".to_string(), Some(RpPosition::At(2))),
        ];
        sort_by_stack_order(&mut entries);
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["top.zip", "second.zip", "floor.zip", "off.zip"]);
    }

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
        assert_eq!(positions.get("1.21.10+fabric"), Some(&RpPosition::At(3)));
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
        assert_eq!(
            order_specs(&spaced, false).unwrap(),
            order_specs(&joined, false).unwrap()
        );
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
mod changelog_tests {
    use super::{render_changelog, split_changelog};

    #[test]
    fn changelog_blocks_are_newest_first() {
        let raw = "[[2026-09-29]]\nchangelogtext = \"\"\"\nold\n\"\"\"\n\n";
        let (preamble, mut blocks) = split_changelog(raw);
        blocks.push((
            "2026-11-22".to_string(),
            "[[2026-11-22]]\nchangelogtext = \"\"\"\nnew\n\"\"\"\n".to_string(),
        ));
        blocks.sort_by(|a, b| b.0.cmp(&a.0));
        let out = render_changelog(&preamble, &blocks);
        let old = out.find("[[2026-09-29]]").expect("old block");
        let new = out.find("[[2026-11-22]]").expect("new block");
        assert!(new < old);
    }

    #[test]
    fn newest_entry_wins_for_the_same_date() {
        let raw = "[[2026-11-22]]\nchangelogtext = \"\"\"\nfirst\n\"\"\"\n";
        let (preamble, mut blocks) = split_changelog(raw);
        blocks.push((
            "2026-11-22".to_string(),
            "[[2026-11-22]]\nchangelogtext = \"\"\"\nsecond\n\"\"\"\n".to_string(),
        ));
        let out = render_changelog(&preamble, &blocks);
        assert!(
            out.rfind("second").expect("second entry") > out.rfind("first").expect("first entry")
        );
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
        assert!(
            game_mc_ok("1.21.10", &listed),
            "1.21.x builds work across the line"
        );
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