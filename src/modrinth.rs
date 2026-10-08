use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::useragent::USER_AGENT;

pub const DEFAULT_API: &str = "https://api.modrinth.com/v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Sort {
    Relevance,
    Follows,
    Downloads,
    Newest,
    Updated,
}

impl Sort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Follows => "follows",
            Self::Downloads => "downloads",
            Self::Newest => "newest",
            Self::Updated => "updated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Disclosure {
    AiContent,
    AiFunctionality,
    Advertisements,
    EpilepsyTriggers,
    SystemInteractions,
    Telemetry,
    DerivativeWork,
    PaidFeatures,
    Archived,
}

impl Disclosure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AiContent => "ai_content",
            Self::AiFunctionality => "ai_functionality",
            Self::Advertisements => "advertisements",
            Self::EpilepsyTriggers => "epilepsy_triggers",
            Self::SystemInteractions => "system_interactions",
            Self::Telemetry => "telemetry",
            Self::DerivativeWork => "derivative_work",
            Self::PaidFeatures => "paid_features",
            Self::Archived => "archived",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::AiContent,
            Self::AiFunctionality,
            Self::Advertisements,
            Self::EpilepsyTriggers,
            Self::SystemInteractions,
            Self::Telemetry,
            Self::DerivativeWork,
            Self::PaidFeatures,
            Self::Archived,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AiContent => "AI-generated content (code / assets / text)",
            Self::AiFunctionality => "Uses AI functionality (chatbot)",
            Self::Advertisements => "advertisements",
            Self::EpilepsyTriggers => "photosensitivity triggers",
            Self::SystemInteractions => "talks to external systems",
            Self::Telemetry => "telemetry (opt-in / opt-out / always on)",
            Self::DerivativeWork => "derivative work (fork)",
            Self::PaidFeatures => "paid features",
            Self::Archived => "archived",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::AiContent => "AI-generated content",
            Self::AiFunctionality => "Uses AI functionality (chatbot)",
            Self::Advertisements => "advertisements",
            Self::EpilepsyTriggers => "photosensitivity triggers",
            Self::SystemInteractions => "talks to external systems",
            Self::Telemetry => "telemetry",
            Self::DerivativeWork => "derivative work",
            Self::PaidFeatures => "paid features",
            Self::Archived => "archived",
        }
    }

    pub fn tokens(self) -> &'static [&'static str] {
        match self {
            Self::AiContent => &[
                "ai_content",
                "ai_content_code",
                "ai_content_assets",
                "ai_content_text",
            ],
            Self::AiFunctionality => &["ai_functionality"],
            Self::Advertisements => &["advertisements"],
            Self::EpilepsyTriggers => &["epilepsy_triggers"],
            Self::SystemInteractions => &["system_interactions"],
            Self::Telemetry => &[
                "telemetry",
                "telemetry_opt_in",
                "telemetry_opt_out",
                "telemetry_always_active",
            ],
            Self::DerivativeWork => &["derivative_work"],
            Self::PaidFeatures => &["paid_features"],
            Self::Archived => &["archived"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Environment {
    ClientAndServer,
    ClientOnly,
    ClientOnlyServerOptional,
    SingleplayerOnly,
    ServerOnly,
    ServerOnlyClientOptional,
    DedicatedServerOnly,
    ClientOrServer,
    ClientOrServerPrefersBoth,
    Unknown,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientAndServer => "client_and_server",
            Self::ClientOnly => "client_only",
            Self::ClientOnlyServerOptional => "client_only_server_optional",
            Self::SingleplayerOnly => "singleplayer_only",
            Self::ServerOnly => "server_only",
            Self::ServerOnlyClientOptional => "server_only_client_optional",
            Self::DedicatedServerOnly => "dedicated_server_only",
            Self::ClientOrServer => "client_or_server",
            Self::ClientOrServerPrefersBoth => "client_or_server_prefers_both",
            Self::Unknown => "unknown",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::ClientAndServer,
            Self::ClientOnly,
            Self::ClientOnlyServerOptional,
            Self::SingleplayerOnly,
            Self::ServerOnly,
            Self::ServerOnlyClientOptional,
            Self::DedicatedServerOnly,
            Self::ClientOrServer,
            Self::ClientOrServerPrefersBoth,
            Self::Unknown,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClientAndServer => "client and server",
            Self::ClientOnly => "client only",
            Self::ClientOnlyServerOptional => "client only, server optional",
            Self::SingleplayerOnly => "singleplayer only",
            Self::ServerOnly => "server only",
            Self::ServerOnlyClientOptional => "server only, client optional",
            Self::DedicatedServerOnly => "dedicated server only",
            Self::ClientOrServer => "client or server",
            Self::ClientOrServerPrefersBoth => "client or server, prefers both",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub name: String,
    #[serde(default)]
    pub project_type: String,
    #[serde(default)]
    pub header: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderTag {
    pub name: String,
    #[serde(default)]
    pub supported_project_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateVersion {
    pub file_parts: Vec<String>,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub changelog: String,
    pub dependencies: Vec<String>,
    pub game_versions: Vec<String>,
    pub version_type: String,
    pub loaders: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    pub featured: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModrinthError {
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthedVersion {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectStub {
    pub id: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionStub {
    pub id: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub featured: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttributableVersion {
    pub id: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub files_missing_attribution: Vec<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum AuthFailure {
    Expired(String),
    BadScopes,
    Invalid(String),
    Other(String),
}

pub fn with_details(message: &str, details: &str) -> String {
    if details.trim().is_empty() {
        return message.to_string();
    }
    format!("{message}\n\nTechnical details:\n{details}")
}

pub fn token_looks_wrong(token: &str) -> bool {
    let t = token.trim();
    if t.is_empty() {
        return true;
    }
    !matches!(t.split_once('_'), Some(("mrp", _)) | Some(("mra", _)) | Some(("mro", _)))
}

impl Modrinth {
    fn authed(&self, _token: &str) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder().user_agent(USER_AGENT).build()?)
    }

    async fn classify(&self, res: reqwest::Response) -> AuthFailure {
        let status = res.status();
        let body: ModrinthError = res.json().await.unwrap_or_default();
        let mut blob = body.description.clone();
        for d in &body.details {
            blob.push(' ');
            blob.push_str(d);
        }
        let lower = blob.to_ascii_lowercase();
        if lower.contains("scope") {
            return AuthFailure::BadScopes;
        }
        if lower.contains("auth method") {
            return AuthFailure::Invalid(blob);
        }
        if lower.contains("credential")
            || lower.contains("expire")
            || lower.contains("revoked")
        {
            return AuthFailure::Expired(blob);
        }
        if status.as_u16() == 401 || lower.contains("auth") {
            return AuthFailure::Invalid(blob);
        }
        AuthFailure::Other(format!(
            "modrinth said {}: {}",
            status.as_u16(),
            if blob.trim().is_empty() {
                body.error
            } else {
                blob
            }
        ))
    }

    pub async fn whoami(&self, token: &str) -> Result<Result<String, AuthFailure>> {
        let res = self
            .authed(token)?
            .get(format!("{}/user", self.base))
            .header("Authorization", token)
            .send()
            .await
            .context("could not reach modrinth to check your token")?;
        if !res.status().is_success() {
            return Ok(Err(self.classify(res).await));
        }
        let body: serde_json::Value = res.json().await?;
        let who = body
            .get("username")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("id").and_then(|v| v.as_str()))
            .unwrap_or("unknown")
            .to_string();
        Ok(Ok(who))
    }

    pub async fn project_exists(&self, token: &str, id: &str) -> Result<bool> {
        let res = self
            .authed(token)?
            .get(format!("{}/project/{}", self.base, encode(id)))
            .header("Authorization", token)
            .send()
            .await
            .context("could not reach modrinth")?;
        Ok(res.status().is_success())
    }

    pub async fn owned_projects(&self, token: &str) -> Result<Result<Vec<ProjectStub>, AuthFailure>> {
        let res = self
            .authed(token)?
            .get(format!("{}/projects", self.base))
            .header("Authorization", token)
            .send()
            .await
            .context("could not reach modrinth to list your projects")?;
        if !res.status().is_success() {
            return Ok(Err(self.classify(res).await));
        }
        Ok(Ok(res.json().await?))
    }

    pub async fn project_versions(&self, token: &str, id: &str) -> Result<Vec<VersionStub>> {
        let res = self
            .authed(token)?
            .get(format!(
                "{}/project/{}/version",
                self.base,
                encode(id)
            ))
            .header("Authorization", token)
            .send()
            .await
            .context("could not ask modrinth for the versions on that project")?;
        if !res.status().is_success() {
            return Ok(Vec::new());
        }
        Ok(res.json().await?)
    }

    pub async fn project_versions_v3(
        &self,
        token: &str,
        id: &str,
    ) -> Result<Vec<AttributableVersion>> {
        let base = self.base.replace("/v2", "/v3");
        let res = self
            .authed(token)?
            .get(format!("{base}/project/{}/version?limit=100", encode(id)))
            .header("Authorization", token)
            .send()
            .await
            .context("could not ask modrinth for the versions on that project")?;
        if !res.status().is_success() {
            anyhow::bail!(
                "modrinth answered {} for the version list",
                res.status().as_u16()
            );
        }
        Ok(res.json().await?)
    }

    pub async fn create_version(
        &self,
        token: &str,
        body: &CreateVersion,
        files: &[(PathBuf, String)],
    ) -> Result<Result<AuthedVersion, AuthFailure>> {
        let mut form = reqwest::multipart::Form::new().text("data", serde_json::to_string(body)?);
        for (i, (path, name)) in files.iter().enumerate() {
            let bytes = std::fs::read(path)
                .with_context(|| format!("could not read {}", path.display()))?;
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(name.clone())
                .mime_str("application/octet-stream")?;
            form = form.part(format!("file{i}"), part);
        }
        let res = self
            .authed(token)?
            .post(format!("{}/version", self.base))
            .header("Authorization", token)
            .multipart(form)
            .send()
            .await
            .context("the upload to modrinth did not go through")?;
        if !res.status().is_success() {
            return Ok(Err(self.classify(res).await));
        }
        Ok(Ok(res.json().await?))
    }

    pub async fn set_featured(
        &self,
        token: &str,
        version_id: &str,
        featured: bool,
    ) -> Result<Result<(), AuthFailure>> {
        let res = self
            .authed(token)?
            .patch(format!("{}/version/{}", self.base, encode(version_id)))
            .header("Authorization", token)
            .json(&serde_json::json!({ "featured": featured }))
            .send()
            .await
            .context("could not reach modrinth to change the featured version")?;
        if !res.status().is_success() {
            return Ok(Err(self.classify(res).await));
        }
        Ok(Ok(()))
    }
}


#[derive(Debug, Clone, Deserialize)]
pub struct DerivativeSource {
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisclosureDetail {
    pub r#type: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub consent: Option<String>,
    #[serde(default)]
    pub uses: Vec<String>,
    #[serde(default)]
    pub data_collected: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub interactions: Vec<String>,
    #[serde(default)]
    pub sources: Vec<DerivativeSource>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisclosuresResponse {
    #[serde(default)]
    pub disclosures: Vec<DisclosureDetail>,
}

impl DisclosureDetail {
    pub fn kind(&self) -> Option<Disclosure> {
        Disclosure::all()
            .iter()
            .copied()
            .find(|d| d.as_str() == self.r#type)
    }

    pub fn heading(&self) -> String {
        let kind = match self.kind() {
            Some(d) => d.short().to_string(),
            None => self.r#type.clone(),
        };
        match self.consent.as_deref() {
            Some("opt_in") => format!("{kind} (opt-in)"),
            Some("opt_out") => format!("{kind} (opt-out)"),
            Some("always_active") => format!("{kind} (always active)"),
            Some(other) => format!("{kind} ({other})"),
            None => kind,
        }
    }

    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |s: &str| {
            let t = s.trim();
            if !t.is_empty() && !out.iter().any(|o| o == t) {
                out.push(t.to_string());
            }
        };
        if let Some(n) = &self.note {
            push(n);
        }
        if !self.uses.is_empty() {
            push(&format!("applies to: {}", self.uses.join(", ")));
        }
        for d in self.data_collected.iter().chain(&self.features).chain(&self.interactions) {
            push(d);
        }
        for s in &self.sources {
            if let Some(n) = &s.note {
                push(n);
            }
            if !s.label.trim().is_empty() {
                push(&s.label);
            }
            if let Some(l) = &s.link {
                push(l);
            }
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct SearchFilters {
    pub query: String,
    pub project_types: Vec<String>,
    pub categories: Vec<String>,
    pub exclude_categories: Vec<String>,
    pub versions: Vec<String>,
    pub loaders: Vec<String>,
    pub environments: Vec<Environment>,
    pub open_source: Option<bool>,
    pub include_disclosures: Vec<Disclosure>,
    pub exclude_disclosures: Vec<Disclosure>,
    pub sort: Option<Sort>,
    pub limit: usize,
    pub page: usize,
}

impl Default for SearchFilters {
    fn default() -> Self {
        Self {
            query: String::new(),
            project_types: Vec::new(),
            categories: Vec::new(),
            exclude_categories: Vec::new(),
            versions: Vec::new(),
            loaders: Vec::new(),
            environments: Vec::new(),
            open_source: None,
            include_disclosures: Vec::new(),
            exclude_disclosures: Vec::new(),
            sort: None,
            limit: 10,
            page: 1,
        }
    }
}

impl SearchFilters {
    pub fn facets(&self) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = Vec::new();

        let types: Vec<String> = self
            .project_types
            .iter()
            .map(|t| format!("project_type:{t}"))
            .collect();
        if !types.is_empty() {
            out.push(types);
        }

        let mut cats: Vec<String> = self
            .categories
            .iter()
            .map(|c| format!("categories:{c}"))
            .collect();
        cats.extend(self.loaders.iter().map(|l| format!("categories:{l}")));
        if !cats.is_empty() {
            out.push(cats);
        }
        for c in &self.exclude_categories {
            out.push(vec![format!("categories!={c}")]);
        }

        let versions: Vec<String> = self
            .versions
            .iter()
            .map(|v| format!("versions:{v}"))
            .collect();
        if !versions.is_empty() {
            out.push(versions);
        }

        let envs: Vec<String> = self
            .environments
            .iter()
            .map(|e| format!("environment:{}", e.as_str()))
            .collect();
        if !envs.is_empty() {
            out.push(envs);
        }

        if let Some(only) = self.open_source {
            out.push(vec![format!("open_source:{only}")]);
        }

        for d in &self.include_disclosures {
            out.push(vec![format!("disclosure_types:{}", d.as_str())]);
        }
        for d in &self.exclude_disclosures {
            out.push(vec![format!("disclosure_types!={}", d.as_str())]);
        }

        out
    }

    pub fn active_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.project_types.is_empty() {
            out.push(format!(
                "type: {}",
                self.project_types.join(", ")
            ));
        }
        if !self.categories.is_empty() {
            out.push(format!("category: {}", self.categories.join(", ")));
        }
        if !self.exclude_categories.is_empty() {
            out.push(format!(
                "not category: {}",
                self.exclude_categories.join(", ")
            ));
        }
        if !self.versions.is_empty() {
            out.push(format!("minecraft: {}", self.versions.join(", ")));
        }
        if !self.loaders.is_empty() {
            out.push(format!("loader: {}", self.loaders.join(", ")));
        }
        if !self.environments.is_empty() {
            out.push(format!(
                "environment: {}",
                self.environments
                    .iter()
                    .map(|e| e.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        match self.open_source {
            Some(true) => out.push("open source only".to_string()),
            Some(false) => out.push("closed source only".to_string()),
            None => {}
        }
        if !self.include_disclosures.is_empty() {
            out.push(format!(
                "disclosed: {}",
                self.include_disclosures
                    .iter()
                    .map(|d| d.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for d in &self.exclude_disclosures {
            out.push(format!("excluded: {}", d.label()));
        }
        if let Some(sort) = self.sort {
            out.push(format!("sorted by {}", sort.as_str()));
        }
        out
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hashes {
    pub sha1: String,
    pub sha512: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub follows: u64,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub project_type: String,
    #[serde(default)]
    pub display_categories: Vec<String>,
    #[serde(default)]
    pub disclosure_types: Vec<String>,
    #[serde(default)]
    pub environment: Vec<String>,
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub hits: Vec<SearchHit>,
    #[serde(default)]
    pub total_hits: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TagEntry {
    pub name: String,
    #[serde(default)]
    pub major: Option<String>,
    #[serde(default)]
    pub version_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct License {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamMember {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub user: Option<TeamUser>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamUser {
    #[serde(default)]
    pub id: Option<String>,
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Team {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub members: Vec<TeamMember>,
}

impl Team {
    pub fn people(&self) -> Vec<String> {
        self.members
            .iter()
            .filter_map(|m| {
                m.user
                    .as_ref()
                    .map(|u| u.display_name.clone().unwrap_or_else(|| u.username.clone()))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Donation {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub project_type: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub team: Option<serde_json::Value>,
    #[serde(default)]
    pub organization: Option<serde_json::Value>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub followers: u64,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub issues_url: Option<String>,
    #[serde(default)]
    pub wiki_url: Option<String>,
    #[serde(default)]
    pub discord_url: Option<String>,
    #[serde(default)]
    pub donation_urls: Vec<Donation>,
    #[serde(default)]
    pub license: Option<License>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MFile {
    pub filename: String,
    pub url: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    pub hashes: Hashes,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionDependency {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub dependency_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MVersion {
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub date_published: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<VersionDependency>,
    #[serde(default)]
    pub files: Vec<MFile>,
}

impl MVersion {
    pub fn primary_file(&self) -> Option<&MFile> {
        self.files.iter().find(|f| f.primary).or_else(|| self.files.first())
    }

    pub fn rank(&self) -> u8 {
        match self.version_type.as_str() {
            "release" => 0,
            "beta" => 1,
            "alpha" => 2,
            _ => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub project: Project,
    pub version: MVersion,
    pub file: MFile,
}

pub fn resolve_from_project(project: Project, version: MVersion) -> Result<Resolved> {
    let file = version
        .primary_file()
        .cloned()
        .context(format!(
            "{} {} has no downloadable file",
            project.slug, version.version_number
        ))?;
    Ok(Resolved {
        project,
        version,
        file,
    })
}

pub fn best_of(mut fits: Vec<MVersion>) -> String {
    fits.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then_with(|| b.date_published.cmp(&a.date_published))
    });
    fits.first()
        .map(|v| v.version_number.clone())
        .unwrap_or_default()
}

fn pick_best(mut fits: Vec<MVersion>) -> MVersion {
    fits.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then_with(|| b.date_published.cmp(&a.date_published))
    });
    fits.remove(0)
}

pub struct Modrinth {
    http: reqwest::Client,
    base: String,
    cache: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Vec<u8>>>>,
}

impl Modrinth {
    pub fn new(base: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .context("could not build the http client")?;
        Ok(Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    pub fn client(&self) -> crate::http::Http {
        crate::http::Http::from_client(self.http.clone())
    }

    pub async fn download(
        &self,
        url: &str,
        dest: &std::path::Path,
        label: &str,
    ) -> Result<u64> {
        self.client().download(url, dest, label).await
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn send(&self, url: &str) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            let res = self
                .http
                .get(url)
                .send()
                .await
                .with_context(|| format!("GET {url} failed"))?;
            if !(res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                || res.status().is_server_error())
                || attempt >= 6
            {
                return Ok(res);
            }
            attempt += 1;
            let wait = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(std::time::Duration::from_secs)
                .unwrap_or_else(|| {
                    std::time::Duration::from_millis(500 * u64::from(attempt))
                });
            tokio::time::sleep(wait).await;
        }
    }

    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let res = self.send(url).await?;
        let ctype = crate::http::content_type(&res);
        let status = res.status();
        let body = res
            .bytes()
            .await
            .with_context(|| format!("could not read the body from {url}"))?
            .to_vec();
        crate::blockpage::check(url, &ctype, &body)?;
        if !status.is_success() {
            bail!("GET {url} returned an error");
        }
        Ok(body)
    }

    pub async fn game_versions(&self) -> Result<Vec<TagEntry>> {
        self.get(&format!("{}/tag/game_version", self.base)).await
    }

    pub async fn versions_by_ids(&self, ids: &[String]) -> Result<Vec<MVersion>> {
        let ids: Vec<String> = ids
            .iter()
            .filter(|id| is_modrinth_id(id))
            .cloned()
            .collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = serde_json::to_string(&ids)?;
        self.get(&format!("{}/versions?ids={}", self.base, encode(&encoded)))
            .await
    }

    pub async fn resolve_in_range(
        &self,
        id_or_slug: &str,
        mc: Option<&str>,
        loaders: &[String],
        range: &str,
    ) -> Result<Resolved> {
        let project = self.project(id_or_slug).await?;
        self.check_loaders(&project, loaders)?;
        let want = if project.project_type == "mod" {
            loaders
        } else {
            &[]
        };
        let versions = self.versions(&project, mc, want).await?;
        let fits: Vec<MVersion> = versions
            .into_iter()
            .filter(|v| {
                v.files.iter().any(|f| f.primary)
                    && crate::modmeta::satisfied(range, &v.version_number)
            })
            .collect();
        if fits.is_empty() {
            bail!(
                "{} has no version matching {range} for {}",
                project.slug,
                mc.unwrap_or("this minecraft version")
            );
        }
        let version = pick_best(fits);
        let file = version
            .primary_file()
            .cloned()
            .context("version has no downloadable file")?;
        Ok(Resolved {
            project,
            version,
            file,
        })
    }

    pub async fn resolve_outside(
        &self,
        id_or_slug: &str,
        mc: Option<&str>,
        loaders: &[String],
        range: &str,
    ) -> Result<Resolved> {
        let project = self.project(id_or_slug).await?;
        self.check_loaders(&project, loaders)?;
        let want = if project.project_type == "mod" {
            loaders
        } else {
            &[]
        };
        let versions = self.versions(&project, mc, want).await?;
        let fits: Vec<MVersion> = versions
            .into_iter()
            .filter(|v| {
                v.files.iter().any(|f| f.primary)
                    && !crate::modmeta::satisfied(range, &v.version_number)
            })
            .collect();
        if fits.is_empty() {
            bail!(
                "{} has no version out of {range} for {}",
                project.slug,
                mc.unwrap_or("this minecraft version")
            );
        }
        let version = pick_best(fits);
        let file = version
            .primary_file()
            .cloned()
            .context("version has no downloadable file")?;
        Ok(Resolved {
            project,
            version,
            file,
        })
    }

    pub async fn version_file(&self, hash: &str) -> Result<Option<MVersion>> {
        let url = format!("{}/version_file/{}", self.base, encode(hash));
        let response = self.send(&url).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = response
            .error_for_status()
            .with_context(|| format!("GET {url} returned an error"))?
            .bytes()
            .await
            .with_context(|| format!("could not read the body from {url}"))?;
        Ok(Some(serde_json::from_slice(&body)?))
    }

    pub async fn version(&self, id: &str) -> Result<MVersion> {
        self.get(&format!("{}/version/{}", self.base, encode(id)))
            .await
    }

    pub async fn projects_by_ids(&self, ids: &[String]) -> Result<Vec<Project>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = serde_json::to_string(ids)?;
        self.get(&format!("{}/projects?ids={}", self.base, encode(&encoded)))
            .await
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let body = self.cached_json(url).await?;
        serde_json::from_slice(&body)
            .with_context(|| format!("could not decode the response from {url}"))
    }

    async fn cached_json(&self, url: &str) -> Result<std::sync::Arc<Vec<u8>>> {
        if let Some(hit) = {
            let cache = self.cache.lock().expect("json cache poisoned");
            cache.get(url).cloned()
        } {
            return Ok(hit);
        }
        let res = self.send(url).await?;
        let ctype = crate::http::content_type(&res);
        let status = res.status();
        let body = res
            .bytes()
            .await
            .with_context(|| format!("could not read the body from {url}"))?
            .to_vec();
        crate::blockpage::check(url, &ctype, &body)?;
        if !status.is_success() {
            bail!("GET {url} returned an error");
        }
        let body = std::sync::Arc::new(body);
        let mut cache = self.cache.lock().expect("json cache poisoned");
        if cache.len() < 4096 {
            cache.insert(url.to_string(), body.clone());
        }
        Ok(body)
    }

    pub async fn team(&self, id: &str) -> Result<Team> {
        self.get(&format!("{}/team/{}", self.base, encode(id)))
            .await
    }

    pub async fn people_of(&self, project: &Project) -> Vec<String> {
        if !project.author.is_empty() {
            return vec![project.author.clone()];
        }
        match self
            .get::<Vec<TeamMember>>(&format!(
                "{}/project/{}/members",
                self.base, project.id
            ))
            .await
        {
            Ok(members) => members
                .into_iter()
                .filter_map(|m| m.user)
                .map(|u| u.display_name.unwrap_or(u.username))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub async fn search_page(&self, filters: &SearchFilters) -> Result<SearchResponse> {
        let mut url = format!("{}/search?query={}", self.base, encode(&filters.query));
        let facets = filters.facets();
        if !facets.is_empty() {
            let groups = facets
                .iter()
                .map(|group| {
                    let inner = group
                        .iter()
                        .map(|f| format!("\"{f}\""))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("[{inner}]")
                })
                .collect::<Vec<_>>()
                .join(",");
            url.push_str(&format!("&facets={}", encode(&format!("[{groups}]"))));
        }
        let limit = filters.limit.clamp(1, 100);
        let offset = limit.saturating_mul(filters.page.saturating_sub(1));
        url.push_str(&format!("&limit={limit}&offset={offset}"));
        if let Some(index) = filters.sort {
            url.push_str(&format!("&index={}", index.as_str()));
        }
        self.get(&url).await
    }

    pub async fn categories(&self) -> Result<Vec<Category>> {
        self.get(&format!("{}/tag/category", self.base)).await
    }

    pub async fn loaders(&self) -> Result<Vec<LoaderTag>> {
        self.get(&format!("{}/tag/loader", self.base)).await
    }

    pub async fn disclosures_of(&self, project: &Project) -> Result<Vec<DisclosureDetail>> {
        let root = self
            .base
            .strip_suffix("/v2")
            .map(str::to_string)
            .unwrap_or_else(|| self.base.clone());
        let got: DisclosuresResponse = self
            .get(&format!(
                "{root}/v3/project/{}/disclosures",
                encode(&project.id)
            ))
            .await?;
        Ok(got.disclosures)
    }

    pub async fn project(&self, id_or_slug: &str) -> Result<Project> {
        self.get(&format!("{}/project/{}", self.base, encode(id_or_slug)))
            .await
    }

    pub async fn versions(
        &self,
        project: &Project,
        mc: Option<&str>,
        want: &[String],
    ) -> Result<Vec<MVersion>> {
        let loaders: Vec<String> = if want.is_empty() {
            project.loaders.clone()
        } else {
            want.to_vec()
        };
        let mut url = format!(
            "{}/project/{}/version?loaders={}",
            self.base,
            project.id,
            encode(&serde_json::to_string(&loaders)?)
        );
        let exact_mc = if project.project_type == "mod" {
            mc
        } else {
            None
        };
        if let Some(mc) = exact_mc {
            url.push_str(&format!(
                "&game_versions={}",
                encode(&serde_json::to_string(&[mc])?)
            ));
        }
        let mut versions: Vec<MVersion> = self.get(&url).await?;
        if !want.is_empty() {
            versions.retain(|v| {
                v.loaders
                    .iter()
                    .any(|l| want.iter().any(|w| l.eq_ignore_ascii_case(w)))
            });
        }
        if project.project_type != "mod" {
            if let Some(mc) = mc {
                versions.retain(|v| crate::modmeta::game_mc_ok(mc, &v.game_versions));
            }
        }
        Ok(versions)
    }

    pub async fn resolve(
        &self,
        id_or_slug: &str,
        mc: Option<&str>,
        loaders: &[String],
    ) -> Result<Resolved> {
        let project = self.project(id_or_slug).await?;
        self.check_loaders(&project, loaders)?;
        let want = if project.project_type == "mod" {
            loaders
        } else {
            &[]
        };
        let mut versions = self.versions(&project, mc, want).await?;
        if versions.is_empty() {
            anyhow::bail!(
                "{} has no version for {}",
                project.title,
                mc.unwrap_or("any Minecraft version")
            );
        }
        versions.retain(|v| v.primary_file().is_some());
        let version = pick_best(versions);
        let file = version
            .primary_file()
            .cloned()
            .context("version has no downloadable file")?;
        Ok(Resolved {
            project,
            version,
            file,
        })
    }

    pub async fn resolve_pinned(
        &self,
        id_or_slug: &str,
        mc: Option<&str>,
        loaders: &[String],
        wanted: &str,
    ) -> Result<Resolved> {
        if wanted == "*" {
            return self.resolve(id_or_slug, mc, loaders).await;
        }
        let project = self.project(id_or_slug).await?;
        self.check_loaders(&project, loaders)?;
        let want = if project.project_type == "mod" {
            loaders
        } else {
            &[]
        };
        let versions = self.versions(&project, mc, want).await?;
        let version = versions
            .into_iter()
            .find(|v| v.version_number == wanted)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{} has no version {} for {}",
                    project.title,
                    wanted,
                    mc.unwrap_or("any Minecraft version")
                )
            })?;
        let file = version
            .primary_file()
            .cloned()
            .context("version has no downloadable file")?;
        Ok(Resolved {
            project,
            version,
            file,
        })
    }

    fn check_loaders(&self, project: &Project, loaders: &[String]) -> Result<()> {
        if loaders.is_empty() || project.project_type != "mod" {
            return Ok(());
        }
        let ok = project
            .loaders
            .iter()
            .any(|have| loaders.iter().any(|want| have.eq_ignore_ascii_case(want)));
        if !ok {
            anyhow::bail!(
                "{} ({}) supports {:?}, not {:?}",
                project.title,
                project.slug,
                project.loaders,
                loaders
            );
        }
        Ok(())
    }
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn is_modrinth_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}