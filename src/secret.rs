use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{bail, Context, Result};

const KEYRING_SERVICE: &str = "ehmodpack";
const KEYRING_USER: &str = "modrinth-token-key";
const MAGIC: &[u8; 4] = b"EHM1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

fn secret_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config directory on this system")?;
    Ok(base.join("ehmodpack"))
}

fn secret_file() -> Result<PathBuf> {
    Ok(secret_dir()?.join("secret.bin"))
}

fn keyring_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(anyhow::Error::from)
}

fn cached_key() -> Option<[u8; KEY_LEN]> {
    use base64::Engine;
    let raw = keyring_entry().ok()?.get_password().ok()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
    <[u8; KEY_LEN]>::try_from(bytes.as_slice()).ok()
}

fn cache_key(key: &[u8; KEY_LEN]) -> bool {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(key);
    match keyring_entry() {
        Ok(entry) => entry.set_password(&encoded).is_ok(),
        Err(_) => false,
    }
}

fn drop_cached_key() {
    if let Ok(entry) = keyring_entry() {
        let _ = entry.delete_credential();
    }
}

fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

fn derive(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    let mut key = [0u8; KEY_LEN];
    argon2::Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("argon2 could not derive a key: {e}"))?;
    Ok(key)
}

fn cipher(key: &[u8; KEY_LEN]) -> Result<Aes256Gcm> {
    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)))
}

fn seal(token: &str, key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    let salt = random_bytes(SALT_LEN);
    let nonce_bytes = random_bytes(NONCE_LEN);
    let ct = cipher(key)?
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: token.as_bytes(),
                aad: MAGIC,
            },
        )
        .map_err(|_| anyhow::anyhow!("could not encrypt the token"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + salt.len() + nonce_bytes.len() + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn unseal(blob: &[u8], key: &[u8; KEY_LEN]) -> Result<String> {
    let head = MAGIC.len() + SALT_LEN + NONCE_LEN;
    if blob.len() < head + 16 || &blob[..MAGIC.len()] != MAGIC {
        bail!("the saved secret is not an ehmodpack secret, delete it and start over");
    }
    let ct = cipher(key)?
        .decrypt(
            Nonce::from_slice(&blob[MAGIC.len() + SALT_LEN..head]),
            Payload {
                msg: &blob[head..],
                aad: MAGIC,
            },
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "We couldn't decrypt your token, did you change your password via some special way?"
            )
        })?;
    String::from_utf8(ct).context("the decrypted token was not valid utf-8")
}

fn read_blob() -> Result<Option<Vec<u8>>> {
    let path = secret_file()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("could not read {}", path.display())),
    }
}

fn write_blob(blob: &[u8]) -> Result<()> {
    let path = secret_file()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    std::fs::write(&path, blob).with_context(|| format!("could not write {}", path.display()))?;
    restrict(&path)
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("could not lock down {}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<()> {
    Ok(())
}

fn prompt(message: &str) -> Result<String> {
    inquire::Password::new(message)
        .prompt()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn generated() -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(32))
}

pub fn has_secret() -> bool {
    matches!(read_blob(), Ok(Some(_)))
}

pub fn backend() -> &'static str {
    if cached_key().is_some() {
        "os keychain over argon2id"
    } else {
        "argon2id"
    }
}

pub fn save(token: &str) -> Result<()> {
    let key = match cached_key() {
        Some(k) => k,
        None => {
            let probe = derive(&generated(), &random_bytes(SALT_LEN))?;
            if cache_key(&probe) {
                probe
            } else {
                drop_cached_key();
                let passphrase = prompt("no os keychain, pick a passphrase for your modrinth token")?;
                derive(&passphrase, &random_bytes(SALT_LEN))?
            }
        }
    };
    let blob = seal(token, &key)?;
    write_blob(&blob)
}

pub fn load() -> Result<Option<String>> {
    let Some(blob) = read_blob()? else {
        return Ok(None);
    };
    let key = match cached_key() {
        Some(k) => k,
        None => {
            let salt = blob
                .get(MAGIC.len()..MAGIC.len() + SALT_LEN)
                .context("the saved secret is truncated")?;
            derive(&prompt("your passphrase")?, salt)?
        }
    };
    Ok(Some(unseal(&blob, &key)?))
}

pub fn clear() -> Result<()> {
    drop_cached_key();
    match std::fs::remove_file(secret_file()?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("could not delete the saved secret: {e}")),
    }
}