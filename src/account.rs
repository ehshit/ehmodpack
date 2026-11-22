use std::time::Duration;

use anyhow::{Context, Result, bail};
use keyring::Entry;

use crate::i18n;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;

const SERVICE: &str = "ehmodpack";
const ACCOUNT_USER: &str = "msa-account";
const TOKEN_ENV: &str = "EHMODPACK_MSA_CLIENT_ID";
const SCOPES: &str = "XboxLive.signin offline_access";

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccount {
    pub client_id: String,
    pub refresh_token: String,
    pub name: String,
    pub uuid: String,
    #[serde(default)]
    pub xuid: Option<String>,
}

#[derive(Clone)]
pub struct Session {
    pub name: String,
    pub uuid: String,
    pub access_token: String,
    pub xuid: Option<String>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("name", &self.name)
            .field("uuid", &self.uuid)
            .field("access_token", &"[redacted]")
            .field("xuid", &self.xuid)
            .finish()
    }
}

#[derive(Deserialize)]
struct DeviceCode {
    user_code: String,
    verification_uri: String,
    device_code: String,
    #[serde(default)]
    interval: u64,
    #[serde(default = "default_expires")]
    expires_in: u64,
    #[serde(default)]
    message: Option<String>,
}

fn default_expires() -> u64 {
    900
}

#[derive(Deserialize)]
struct MsToken {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct McLogin {
    access_token: String,
}

#[derive(Deserialize)]
struct McProfile {
    id: String,
    name: String,
}

enum PollAction {
    Pending,
    Slow,
    Fatal(String),
    Other,
}

fn classify_oauth_error(text: &str) -> PollAction {
    let lower = text.to_ascii_lowercase();
    if lower.contains("authorization_pending") || lower.contains("aadsts70016") {
        PollAction::Pending
    } else if lower.contains("slow_down") {
        PollAction::Slow
    } else if lower.contains("expired_token") || lower.contains("aadsts70019") {
        PollAction::Fatal(i18n::t("login-code-expired"))
    } else if lower.contains("bad_verification_code") || lower.contains("aadsts70018") {
        PollAction::Fatal(i18n::t("login-code-wrong"))
    } else if lower.contains("access_denied")
        || lower.contains("authorization_declined")
        || lower.contains("aadsts65004")
    {
        PollAction::Fatal(i18n::t("login-declined"))
    } else {
        PollAction::Other
    }
}

fn entry() -> Result<Entry> {
    Entry::new(SERVICE, ACCOUNT_USER).map_err(anyhow::Error::from)
}

fn parse<T: DeserializeOwned>(value: &Value, what: &str) -> Result<T> {
    serde_json::from_value(value.clone())
        .with_context(|| i18n::t_args("parse-failed", &[("what", what)]))
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(crate::useragent::USER_AGENT)
        .build()
        .context(i18n::t("http-client-failed"))
}

fn error_message(value: &Value) -> String {
    value
        .get("error_description")
        .or_else(|| value.get("error"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| i18n::t("unknown-error"))
}

async fn post_form(http: &reqwest::Client, url: &str, form: &[(&str, &str)]) -> Result<Value> {
    let response = http
        .post(url)
        .form(form)
        .send()
        .await
        .with_context(|| i18n::t_args("post-failed", &[("url", url)]))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| i18n::t_args("reply-read-failed", &[("url", url)]))?;
    let value: Value = serde_json::from_str(&body).with_context(|| {
        i18n::t_args(
            "not-json",
            &[("url", url), ("status", &status.as_u16().to_string())],
        )
    })?;
    if !status.is_success() {
        bail!(
            "{}",
            i18n::t_args(
                "http-status",
                &[
                    ("url", url),
                    ("status", &status.as_u16().to_string()),
                    ("message", &error_message(&value)),
                ]
            )
        );
    }
    Ok(value)
}

async fn post_json(http: &reqwest::Client, url: &str, body: Value) -> Result<Value> {
    let response = http
        .post(url)
        .json(&body)
        .send()
        .await
        .with_context(|| i18n::t_args("post-failed", &[("url", url)]))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| i18n::t_args("reply-read-failed", &[("url", url)]))?;
    let value: Value = serde_json::from_str(&text).with_context(|| {
        i18n::t_args(
            "not-json",
            &[("url", url), ("status", &status.as_u16().to_string())],
        )
    })?;
    if !status.is_success() {
        if let Some(code) = value.get("XErr").and_then(|v| v.as_i64()) {
            bail!(
                "{}",
                i18n::t_args(
                    "xbox-error",
                    &[
                        ("code", &code.to_string()),
                        ("message", &xerr_message(code))
                    ]
                )
            );
        }
        bail!(
            "{}",
            i18n::t_args(
                "http-status",
                &[
                    ("url", url),
                    ("status", &status.as_u16().to_string()),
                    ("message", &error_message(&value)),
                ]
            )
        );
    }
    Ok(value)
}

async fn get_with_token(http: &reqwest::Client, url: &str, token: &str) -> Result<Value> {
    let response = http
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| i18n::t_args("get-failed", &[("url", url)]))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| i18n::t_args("reply-read-failed", &[("url", url)]))?;
    let value: Value = serde_json::from_str(&text).with_context(|| {
        i18n::t_args(
            "not-json",
            &[("url", url), ("status", &status.as_u16().to_string())],
        )
    })?;
    if !status.is_success() {
        bail!(
            "{}",
            i18n::t_args(
                "http-status",
                &[
                    ("url", url),
                    ("status", &status.as_u16().to_string()),
                    ("message", &error_message(&value)),
                ]
            )
        );
    }
    Ok(value)
}

fn xerr_message(code: i64) -> String {
    let key = match code {
        2148916222 => "xerr-uk-age",
        2148916227 => "xerr-suspended",
        2148916229 => "xerr-online-restricted",
        2148916233 => "xerr-no-profile",
        2148916234 => "xerr-terms",
        2148916235 => "xerr-region",
        2148916236 | 2148916237 => "xerr-adult",
        2148916238 => "xerr-family",
        _ => "xerr-refused",
    };
    i18n::t(key)
}

struct AuthHint {
    keys: &'static [&'static str],
    headline: &'static str,
    what_happened: &'static str,
    hint: Option<&'static str>,
}

static AUTH_HINTS: &[AuthHint] = &[
    AuthHint {
        keys: &["2148916222"],
        headline: "auth-age-check-headline",
        what_happened: "auth-age-check-what",
        hint: Some("auth-age-check-hint"),
    },
    AuthHint {
        keys: &["2148916233"],
        headline: "auth-no-xbox-profile-headline",
        what_happened: "auth-no-xbox-profile-what",
        hint: Some("auth-no-xbox-profile-hint"),
    },
    AuthHint {
        keys: &["2148916235"],
        headline: "auth-xbox-region-headline",
        what_happened: "auth-xbox-region-what",
        hint: Some("auth-xbox-region-hint"),
    },
    AuthHint {
        keys: &["2148916236", "2148916237"],
        headline: "auth-adult-verification-headline",
        what_happened: "auth-adult-verification-what",
        hint: Some("auth-adult-verification-hint"),
    },
    AuthHint {
        keys: &["2148916238"],
        headline: "auth-family-group-headline",
        what_happened: "auth-family-group-what",
        hint: Some("auth-family-group-hint"),
    },
    AuthHint {
        keys: &["2148916227"],
        headline: "auth-xbox-suspended-headline",
        what_happened: "auth-xbox-suspended-what",
        hint: None,
    },
    AuthHint {
        keys: &["2148916229"],
        headline: "auth-online-restricted-headline",
        what_happened: "auth-online-restricted-what",
        hint: Some("auth-online-restricted-hint"),
    },
    AuthHint {
        keys: &["2148916234"],
        headline: "auth-xbox-terms-headline",
        what_happened: "auth-xbox-terms-what",
        hint: Some("auth-xbox-terms-hint"),
    },
    AuthHint {
        keys: &["xerr", "xbox live returned error", "xsts"],
        headline: "auth-xbox-refused-headline",
        what_happened: "auth-xbox-refused-what",
        hint: Some("auth-xbox-refused-hint"),
    },
    AuthHint {
        keys: &["refused this app id", "invalid app registration"],
        headline: "auth-mojang-approval-headline",
        what_happened: "auth-mojang-approval-what",
        hint: Some("auth-mojang-approval-hint"),
    },
    AuthHint {
        keys: &["does not own minecraft"],
        headline: "auth-no-java-headline",
        what_happened: "auth-no-java-what",
        hint: Some("auth-no-java-hint"),
    },
    AuthHint {
        keys: &["did not return a profile", "create one on minecraft.net"],
        headline: "auth-no-profile-headline",
        what_happened: "auth-no-profile-what",
        hint: Some("auth-no-profile-hint"),
    },
    AuthHint {
        keys: &[
            "could not refresh the microsoft sign-in",
            "saved microsoft sign-in",
        ],
        headline: "auth-expired-signin-headline",
        what_happened: "auth-expired-signin-what",
        hint: Some("auth-expired-signin-hint"),
    },
    AuthHint {
        keys: &["aadsts70002"],
        headline: "auth-public-client-headline",
        what_happened: "auth-public-client-what",
        hint: Some("auth-public-client-hint"),
    },
    AuthHint {
        keys: &[
            "aadsts70018",
            "bad_verification_code",
            "that sign-in code was wrong",
        ],
        headline: "auth-wrong-code-headline",
        what_happened: "auth-wrong-code-what",
        hint: Some("auth-wrong-code-hint"),
    },
    AuthHint {
        keys: &["aadsts70019", "the sign-in code expired"],
        headline: "auth-code-expired-headline",
        what_happened: "auth-code-expired-what",
        hint: Some("auth-code-expired-hint"),
    },
    AuthHint {
        keys: &["aadsts65004", "the sign-in was declined", "access_denied"],
        headline: "auth-signin-declined-headline",
        what_happened: "auth-signin-declined-what",
        hint: Some("auth-signin-declined-hint"),
    },
    AuthHint {
        keys: &["returned http 429"],
        headline: "auth-too-many-headline",
        what_happened: "auth-too-many-what",
        hint: Some("auth-too-many-hint"),
    },
    AuthHint {
        keys: &["error sending request for url"],
        headline: "auth-network-headline",
        what_happened: "auth-network-what",
        hint: Some("auth-network-hint"),
    },
];

pub fn pretty_error(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let hint = AUTH_HINTS
        .iter()
        .find(|hint| hint.keys.iter().any(|key| lower.contains(key)))?;
    let raw = text.lines().next().unwrap_or(text);
    let mut out = String::new();
    out.push_str(&i18n::t(hint.headline));
    out.push('\n');
    out.push_str(&i18n::t("what-happened"));
    out.push(' ');
    out.push_str(&i18n::t(hint.what_happened));
    if let Some(tip) = hint.hint {
        out.push('\n');
        out.push_str(&i18n::t("hint"));
        out.push(' ');
        out.push_str(&i18n::t(tip));
    }
    out.push('\n');
    out.push_str(&i18n::t("raw-error"));
    out.push(' ');
    out.push_str(raw);
    Some(out)
}

pub fn client_id(explicit: Option<&str>) -> Result<String> {
    if let Some(id) = explicit {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    if let Ok(id) = std::env::var(TOKEN_ENV) {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    if let Some(id) = option_env!("EHMODPACK_MSA_CLIENT_ID_BUNDLED") {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    bail!(
        "{}",
        i18n::t_args("client-id-missing", &[("env", TOKEN_ENV)])
    )
}

fn owns_minecraft(entitlements: &Value) -> bool {
    let Some(items) = entitlements.get("items").and_then(|v| v.as_array()) else {
        return false;
    };
    items
        .iter()
        .filter_map(|item| item.get("name").and_then(|v| v.as_str()))
        .any(|name| {
            matches!(
                name,
                "game_minecraft" | "product_minecraft" | "game_minecraft_java"
            )
        })
}

pub fn saved() -> Result<Option<StoredAccount>> {
    let raw = match entry()?.get_password() {
        Ok(raw) => raw,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    serde_json::from_str(&raw)
        .context(i18n::t("saved-account-invalid"))
        .map(Some)
}

fn save(account: &StoredAccount) -> Result<()> {
    let raw = serde_json::to_string(account)?;
    entry()?
        .set_password(&raw)
        .map_err(anyhow::Error::from)
        .context(i18n::t("account-save-failed"))
}

pub fn logout() -> Result<bool> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

async fn exchange_ms_token(
    http: &reqwest::Client,
    client_id: &str,
    grant_type: &str,
    grant_key: &str,
    grant_value: &str,
) -> Result<MsToken> {
    let value = post_form(
        http,
        TOKEN_URL,
        &[
            ("client_id", client_id),
            ("scope", SCOPES),
            ("grant_type", grant_type),
            (grant_key, grant_value),
        ],
    )
    .await?;
    parse(&value, "the Microsoft token reply")
}

async fn minecraft_session<F>(
    http: &reqwest::Client,
    client_id: &str,
    ms_token: &MsToken,
    mut notify: F,
) -> Result<(Session, Option<String>)>
where
    F: FnMut(&str),
{
    notify(&i18n::t("xbl-token"));
    let xbl = post_json(
        http,
        XBL_URL,
        json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", ms_token.access_token),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
    )
    .await?;
    let xbl_token = xbl
        .get("Token")
        .and_then(|v| v.as_str())
        .context(i18n::t("xbl-token-missing"))?;
    let xui = xbl
        .get("DisplayClaims")
        .and_then(|v| v.get("xui"))
        .and_then(|v| v.as_array())
        .and_then(|v| v.first())
        .context(i18n::t("xbl-user-hash-missing"))?;
    let uhs = xui
        .get("uhs")
        .and_then(|v| v.as_str())
        .context(i18n::t("xbl-user-hash-missing"))?;
    let xuid = xui.get("xid").and_then(|v| v.as_str()).map(str::to_string);

    notify(&i18n::t("xsts-token"));
    let xsts = post_json(
        http,
        XSTS_URL,
        json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }),
    )
    .await?;
    let xsts_token = xsts
        .get("Token")
        .and_then(|v| v.as_str())
        .context(i18n::t("xsts-token-missing"))?;

    notify(&i18n::t("mc-token"));
    let login = post_json(
        http,
        MC_LOGIN_URL,
        json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") }),
    )
    .await
    .with_context(|| i18n::t_args("mc-app-id-refused", &[("client_id", client_id)]))?;
    let mc: McLogin = parse(&login, &i18n::t("mc-token-reply"))?;

    notify(&i18n::t("mc-ownership"));
    let entitlements = get_with_token(http, MC_ENTITLEMENTS_URL, &mc.access_token).await?;
    if !owns_minecraft(&entitlements) {
        bail!("{}", i18n::t("mc-not-owned"));
    }

    notify(&i18n::t("mc-profile"));
    let profile = get_with_token(http, MC_PROFILE_URL, &mc.access_token)
        .await
        .context(i18n::t("mc-profile-missing"))?;
    let profile: McProfile = parse(&profile, &i18n::t("mc-profile-reply"))?;

    Ok((
        Session {
            name: profile.name,
            uuid: profile.id,
            access_token: mc.access_token,
            xuid,
        },
        ms_token.refresh_token.clone(),
    ))
}

pub async fn login<F>(client_id: &str, mut notify: F) -> Result<StoredAccount>
where
    F: FnMut(&str),
{
    let http = http_client()?;
    let device = post_form(
        &http,
        DEVICE_CODE_URL,
        &[("client_id", client_id), ("scope", SCOPES)],
    )
    .await?;
    let device: DeviceCode = parse(&device, &i18n::t("device-code-reply"))?;
    notify(&i18n::t_args(
        "login-go-to",
        &[
            ("url", &device.verification_uri),
            ("code", &device.user_code),
        ],
    ));
    if let Some(message) = &device.message {
        notify(message);
    }
    notify(&i18n::t("login-waiting"));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = device.interval.max(5);
    let ms_token = loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("{}", i18n::t("login-code-expired"));
        }
        sleep(Duration::from_secs(interval)).await;
        match exchange_ms_token(
            &http,
            client_id,
            "urn:ietf:params:oauth:grant-type:device_code",
            "device_code",
            &device.device_code,
        )
        .await
        {
            Ok(token) => break token,
            Err(e) => match classify_oauth_error(&e.to_string()) {
                PollAction::Pending => {}
                PollAction::Slow => interval += 5,
                PollAction::Fatal(message) => bail!(message),
                PollAction::Other => return Err(e),
            },
        }
    };
    let refresh_token = ms_token
        .refresh_token
        .clone()
        .context(i18n::t("no-refresh-token"))?;

    let (session, rotated) = minecraft_session(&http, client_id, &ms_token, &mut notify).await?;
    notify(&i18n::t("saving-account"));
    let account = StoredAccount {
        client_id: client_id.to_string(),
        refresh_token: rotated.unwrap_or(refresh_token),
        name: session.name,
        uuid: session.uuid,
        xuid: session.xuid,
    };
    save(&account)?;
    Ok(account)
}

pub async fn session<F>(mut notify: F) -> Result<Session>
where
    F: FnMut(&str),
{
    let account = saved()?.context(i18n::t("no-saved-account"))?;
    notify(&i18n::t("refreshing-login"));
    let http = http_client()?;
    let ms_token = exchange_ms_token(
        &http,
        &account.client_id,
        "refresh_token",
        "refresh_token",
        &account.refresh_token,
    )
    .await
    .context(i18n::t("refresh-failed"))?;
    let (session, rotated) =
        minecraft_session(&http, &account.client_id, &ms_token, &mut notify).await?;
    if rotated.is_some()
        || session.name != account.name
        || session.uuid != account.uuid
        || session.xuid != account.xuid
    {
        let updated = StoredAccount {
            client_id: account.client_id,
            refresh_token: rotated.unwrap_or(account.refresh_token),
            name: session.name.clone(),
            uuid: session.uuid.clone(),
            xuid: session.xuid.clone(),
        };
        notify(&i18n::t("updating-account"));
        save(&updated)?;
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_pending_is_not_an_error() {
        assert!(matches!(
            classify_oauth_error("authorization_pending"),
            PollAction::Pending
        ));
        assert!(matches!(
            classify_oauth_error("AADSTS70016: The provided request has not yet been authorized"),
            PollAction::Pending
        ));
        assert!(matches!(
            classify_oauth_error("AADSTS70019: CodeExpired"),
            PollAction::Fatal(_)
        ));
        assert!(matches!(
            classify_oauth_error("slow_down"),
            PollAction::Slow
        ));
        assert!(matches!(
            classify_oauth_error("expired_token"),
            PollAction::Fatal(_)
        ));
        assert!(matches!(
            classify_oauth_error("bad_verification_code"),
            PollAction::Fatal(_)
        ));
    }

    #[test]
    fn xbox_errors_get_a_human_explanation() {
        let hint = pretty_error("Xbox Live returned error 2148916233: no profile").unwrap();
        assert!(hint.contains("No Xbox profile"));
        assert!(!hint.contains("How to fix:"));
        assert!(hint.contains("Hint:"));
        assert!(hint.contains("Minecraft: Java Edition"));
        assert!(pretty_error("some unrelated failure").is_none());
    }

    #[test]
    fn mojang_approval_errors_get_a_human_explanation() {
        let hint = pretty_error("Minecraft services refused this app ID (abc)").unwrap();
        assert!(hint.contains("Mojang hasn't approved this app yet"));
        assert!(hint.contains("https://aka.ms/mce-reviewappid"));
    }

    #[test]
    fn ownership_needs_a_real_minecraft_entitlement() {
        let yes = json!({"items": [{"name": "product_minecraft"}]});
        let no = json!({"items": [{"name": "game_forge"}]});
        assert!(owns_minecraft(&yes));
        assert!(!owns_minecraft(&no));
        assert!(!owns_minecraft(&json!({})));
    }

    #[test]
    fn session_debug_never_shows_the_token() {
        let session = Session {
            name: "swilia".to_string(),
            uuid: "abc".to_string(),
            access_token: "TOP_SECRET".to_string(),
            xuid: None,
        };
        let text = format!("{session:?}");
        assert!(!text.contains("TOP_SECRET"));
        assert!(text.contains("redacted"));
    }
}