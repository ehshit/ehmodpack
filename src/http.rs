use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;

pub fn content_type(res: &reqwest::Response) -> String {
    res.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase()
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
        if !status.is_success() {
            bail!("GET {url} returned an error");
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
        use anyhow::Context;
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url} failed"))?;
        let ctype = content_type(&response);
        if !response.status().is_success() {
            let body = response
                .bytes()
                .await
                .with_context(|| format!("could not read the body from {url}"))?;
            crate::blockpage::check(url, &ctype, &body)?;
            bail!("GET {url} returned an error");
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