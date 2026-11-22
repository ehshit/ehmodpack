use std::fmt;
use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;

const DOWNLOAD_ATTEMPTS: usize = 10;

#[derive(Debug)]
struct StatusError(u16);

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "server returned HTTP {}", self.0)
    }
}

impl std::error::Error for StatusError {}

pub fn content_type(res: &reqwest::Response) -> String {
    res.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn retryable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if cause.downcast_ref::<StatusError>().is_some() {
            return true;
        }
        if let Some(net) = cause.downcast_ref::<reqwest::Error>() {
            if net.is_timeout() || net.is_connect() || net.is_request() || net.is_body() {
                return true;
            }
            if matches!(
                net.status().map(|s| s.as_u16()),
                Some(408 | 429 | 500..=599)
            ) {
                return true;
            }
        }
        let text = cause.to_string().to_ascii_lowercase();
        [
            "connection closed",
            "connection reset",
            "error sending request",
            "could not read the body",
            "the connection dropped",
            "timed out",
            "unexpected eof",
            "broken pipe",
            "failed to lookup",
        ]
        .iter()
        .any(|needle| text.contains(needle))
    })
}

async fn retry<T, F, Fut>(label: &str, mut task: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut last = None;
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        match task().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if !retryable(&error) {
                    return Err(error);
                }
                crate::progress::line(&format!(
                    "retry {attempt}/{DOWNLOAD_ATTEMPTS} after {label}"
                ));
                last = Some(error);
                if attempt < DOWNLOAD_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(250 * attempt as u64)).await;
                }
            }
        }
    }
    let error = last.unwrap_or_else(|| anyhow::anyhow!("{label} failed"));
    Err(error.context(format!("{label} failed after {DOWNLOAD_ATTEMPTS} attempts")))
}

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(crate::useragent::USER_AGENT)
                .build()
                .context("could not build the http client")?,
        })
    }

    pub fn from_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let body = self.get_bytes(url).await?;
        serde_json::from_slice(&body)
            .with_context(|| format!("could not parse the json from {url}"))
    }

    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        retry(&format!("GET {url}"), || self.get_bytes_once(url)).await
    }

    async fn get_bytes_once(&self, url: &str) -> Result<Vec<u8>> {
        let res = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url} failed"))?;
        let ctype = content_type(&res);
        let status = res.status();
        let body = res
            .bytes()
            .await
            .with_context(|| format!("could not read the body from {url}"))?
            .to_vec();
        crate::blockpage::check(url, &ctype, &body)?;
        if status.is_server_error() || matches!(status.as_u16(), 408 | 429) {
            return Err(StatusError(status.as_u16()).into());
        }
        if !status.is_success() {
            bail!("GET {url} returned HTTP {}", status.as_u16());
        }
        Ok(body)
    }

    pub async fn get_text(&self, url: &str) -> Result<String> {
        let body = self.get_bytes(url).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    pub async fn fetch_to(&self, url: &str, dest: &std::path::Path) -> Result<u64> {
        use anyhow::Context;
        let body = self
            .get_bytes(url)
            .await
            .with_context(|| format!("could not fetch {}", dest.display()))?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest, &body)
            .with_context(|| format!("could not write {}", dest.display()))?;
        Ok(body.len() as u64)
    }

    pub async fn download(&self, url: &str, dest: &std::path::Path, label: &str) -> Result<u64> {
        retry(&format!("{label} from {url}"), || {
            self.download_once(url, dest, label)
        })
        .await
    }

    async fn download_once(&self, url: &str, dest: &std::path::Path, label: &str) -> Result<u64> {
        use anyhow::Context;
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url} failed"))?;
        let ctype = content_type(&response);
        let status = response.status();
        if !response.status().is_success() {
            let body = response
                .bytes()
                .await
                .with_context(|| format!("could not read the body from {url}"))?;
            crate::blockpage::check(url, &ctype, &body)?;
            if status.is_server_error() || matches!(status.as_u16(), 408 | 429) {
                return Err(StatusError(status.as_u16()).into());
            }
            bail!("GET {url} returned HTTP {}", status.as_u16());
        }
        let total = response.content_length();
        let mut bar = crate::progress::Bar::new(label);
        let mut body = Vec::new();
        loop {
            let chunk = response
                .chunk()
                .await
                .with_context(|| format!("the connection dropped while reading {url}"))?;
            let Some(chunk) = chunk else { break };
            body.extend_from_slice(&chunk);
            bar.tick(body.len() as u64, total);
        }
        bar.done(body.len() as u64);
        crate::blockpage::check(url, &ctype, &body)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest, &body)
            .with_context(|| format!("could not write {}", dest.display()))?;
        Ok(body.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn retries_transient_status_until_it_works() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        Mock::given(method("GET"))
            .respond_with(move |_request: &wiremock::Request| {
                let call = seen.fetch_add(1, Ordering::SeqCst) + 1;
                if call < 3 {
                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(200).set_body_string("ok")
                }
            })
            .expect(3)
            .mount(&server)
            .await;

        let http = Http::new().unwrap();
        let text = http
            .get_text(&format!("{}/thing", server.uri()))
            .await
            .unwrap();
        assert_eq!(text, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn stops_after_ten_failed_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .expect(DOWNLOAD_ATTEMPTS as u64)
            .mount(&server)
            .await;

        let http = Http::new().unwrap();
        let error = http
            .get_text(&format!("{}/thing", server.uri()))
            .await
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("failed after 10 attempts"), "{message}");
    }

    #[tokio::test]
    async fn does_not_retry_a_client_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;

        let http = Http::new().unwrap();
        let error = http
            .get_text(&format!("{}/thing", server.uri()))
            .await
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("returned HTTP 400"), "{message}");
    }
}