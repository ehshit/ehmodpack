use anyhow::{bail, Result};
use std::sync::OnceLock;

const MATCH_CAP: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Hit {
    BlockPage,
    PlainHtml,
}

const FINGERPRINT: &str = r#"(?is)(?:you have tried to access a web page.{0,100}internet usage polic|powered.{0,60}by.{0,60}fortiguard|fortiguard[\s-]*(?:web[\s-]*)?filter|web[\s-]*filter[\s-]*violation|page you have requested has been blocked.{0,60}url is banned|:8008/xx/yy/zz/ci/[a-z]{8,}|fortiguard[\s-]+intrusion[\s-]+prevention|fortigate[\s-]+application[\s-]+control|powered.{0,60}by.{0,60}fortinet\b|fortinet\.net|application[\s-]+blocked|blocked[\s-]+by[\s-]+content[\s-]*keeper|notification\s*:\s*polic(?:y|ies)\s*:\s*url[\s-]+filtering|redirecting.{0,40}you.{0,40}to.{0,40}barracuda[\s-]+web[\s-]+filter|gateprotect.{0,40}content[\s-]+filter|netsweeper[\s-]+restriction|\burl\b.{1,120}sp.{0,10}er[\s-]+gate|blocked[\s-]+by[\s-]+(?:your|a|the)[\s-]+(?:network[\s-]+|system[\s-]+)?(?:administrator|admin|employer|organization|organisation|school)|contact[\s-]+(?:your|the)[\s-]+(?:network[\s-]+|system[\s-]+)?(?:administrator|admin)|(?:internet[\s-]+usage[\s-]+polic|acceptable[\s-]+(?:use|usage)[\s-]+polic)|<title>\s*[^<]*(?:block(?:ed|ing)?|forbid(?:den)?|denied|restricted|prohibit(?:ed)?|violation)[^<]*</title>|access[\s-]+(?:to|for)[\s-]+(?:this|the)[\s-]+\w{3,12}[\s-]+(?:is[\s-]+)?(?:blocked|restricted|denied|prohibited)|(?:this|the)[\s-]+(?:site|website|page|url)\s+(?:has\s+been|was|is)\s+(?:blocked|blacklisted|banned)|<p>\s*sorry,\s*but\s+the\s+url\s+you.{0,150}requesting\s+is\s+prohibited|<meta[^>]+http-equiv\s*=\s*["']?refresh["']?[^>]+url\s*=\s*[^"'>]*(?:block|filter|denied|restrict|reject|warn))"#;

fn fingerprint() -> &'static regex::Regex {
    static CELL: OnceLock<regex::Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(FINGERPRINT).expect("fingerprint regex must compile")
    })
}

pub fn check(url: &str, content_type: &str, body: &[u8]) -> Result<()> {
    match detect(content_type, body) {
        None => Ok(()),
        Some(Hit::BlockPage) => bail!("{}", blocked_message(url)),
        Some(Hit::PlainHtml) => bail!("{}", webpage_message(url)),
    }
}

pub fn detect(content_type: &str, body: &[u8]) -> Option<Hit> {
    if !looks_like_html(content_type, body) {
        return None;
    }
    let head = &body[..body.len().min(MATCH_CAP)];
    let hay = String::from_utf8_lossy(head);
    if fingerprint().is_match(&hay) {
        Some(Hit::BlockPage)
    } else {
        Some(Hit::PlainHtml)
    }
}

fn looks_like_html(content_type: &str, body: &[u8]) -> bool {
    if content_type.contains("text/html") || content_type.contains("xhtml") {
        return true;
    }
    let head = String::from_utf8_lossy(&body[..body.len().min(512)]);
    let head = head
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

fn blocked_message(url: &str) -> String {
    let host = host_of(url);
    let (name, env) = service(url).unwrap_or((host, None));
    let mut msg = format!(
        "We're unable to connect to {name} since it has been blocked by your internet provider\n\n\
         If this is your computer on your wifi and this error also is a normal webpage on your browser, then {name} is blocked in your territory until it is unblocked\n\n\
         If this is a school device, your school admin blocked {name} and its APIs to be able to fetch, contact them to remove the ban"
    );
    if let Some(env) = env {
        msg.push_str(&format!(" or pass {env}= when doing requests"));
    } else {
        msg.push('.');
    }
    msg
}

fn webpage_message(url: &str) -> String {
    let host = host_of(url);
    let (name, _) = service(url).unwrap_or((host, None));
    format!(
        "We couldn't connect to {name} since something between your internet made {name}'s API a webpage instead"
    )
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    rest.get(..end).unwrap_or(rest)
}

fn service(url: &str) -> Option<(&'static str, Option<&'static str>)> {
    let u = url.to_ascii_lowercase();
    if u.contains("modrinth") {
        Some(("Modrinth", Some("EHMODPACK_API")))
    } else if u.contains("fabricmc") {
        Some(("the Fabric meta", Some("EHMODPACK_FABRIC_META")))
    } else if u.contains("quiltmc") {
        Some(("the Quilt meta", Some("EHMODPACK_QUILT_META")))
    } else if u.contains("neoforged") {
        Some(("the NeoForge maven", Some("EHMODPACK_NEOFORGE_MAVEN")))
    } else if u.contains("mojang") {
        Some(("Mojang's version manifest", None))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fortiguard_branding_matches() {
        let body = br#"<!DOCTYPE html><html><body>Powered By FortiGuard</body></html>"#;
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn fortinet_policy_sentence_matches() {
        let body = b"<!doctype html><html><body>You have tried to access a web page which is in violation of your internet usage policy.</body></html>";
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn block_titles_match() {
        let body = b"<!doctype html><html><head><title>Site Blocked</title></head><body>customized</body></html>";
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn forbidden_titles_match_too() {
        let body = b"<html><head><title>403 Forbidden</title></head><html>";
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn admin_contact_pages_match() {
        let body = b"<!doctype html><html><body><p>This site was blocked by your network administrator</p></body></html>";
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn fortiguard_pass_through_url_needs_any_tail_not_one_magic_string() {
        let body = b"<!doctype html><html><body>meta refresh to :8008/XX/YY/ZZ/CI/QWERTYUIOP</body></html>";
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn usage_policy_phrases_match() {
        let body = b"<!doctype html><html><body>in violation of our acceptable use policy</body></html>";
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn meta_refresh_redirects_match() {
        let body = b"<!doctype html><html><head><meta http-equiv=\"refresh\" content=\"0;url=http://filter.local/blocked\"></head></html>";
        assert_eq!(detect("text/html", body), Some(Hit::BlockPage));
    }

    #[test]
    fn real_json_is_left_alone() {
        let body = br#"{"hits":[],"total_hits":0,"limit":20}"#;
        assert_eq!(detect("application/json", body), None);
    }

    #[test]
    fn binaries_are_left_alone() {
        let body = b"PK\x03\x04\x14\x00java jar bytes here";
        assert_eq!(detect("application/java-archive", body), None);
    }

    #[test]
    fn html_without_any_signature_is_plain_html() {
        let body = b"<!doctype html><html><body>custom prison</body></html>";
        assert_eq!(detect("text/html", body), Some(Hit::PlainHtml));
    }

    #[test]
    fn blocked_message_hides_the_vendor() {
        let msg = blocked_message("https://api.modrinth.com/v2/search?query=");
        assert!(msg.contains("Modrinth"));
        assert!(msg.contains("EHMODPACK_API"));
        assert!(!msg.contains("FortiGuard"));
        assert!(!msg.contains("Fortinet"));
    }

    #[test]
    fn plain_html_message_matches_the_spec() {
        let msg = webpage_message("https://api.modrinth.com/v2/search?query=");
        assert_eq!(
            msg,
            "We couldn't connect to Modrinth since something between your internet made Modrinth's API a webpage instead"
        );
    }

    #[test]
    fn message_for_fabric_points_at_the_fabric_override() {
        let msg = blocked_message("https://meta.fabricmc.net/v2/versions/loader/1.21.11");
        assert!(msg.contains("the Fabric meta"));
        assert!(msg.contains("EHMODPACK_FABRIC_META"));
    }

    #[test]
    fn message_without_an_override_still_reads_clean() {
        let msg = blocked_message("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json");
        assert!(msg.contains("Mojang's version manifest"));
        assert!(!msg.contains("EHMODPACK_"));
    }
}