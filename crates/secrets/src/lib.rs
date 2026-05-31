use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STORE_VERSION: u32 = 1;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 32;
const MAX_ID_LEN: usize = 128;
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("secret id is empty")]
    EmptyId,
    #[error("secret id `{id}` is too long (max {max} bytes)")]
    IdTooLong { id: String, max: usize },
    #[error("secret id `{0}` contains unsupported characters")]
    UnsupportedIdChars(String),
    #[error("secret id `{0}` must not be absolute or contain traversal components")]
    UnsafeId(String),
    #[error("failed to read secret store {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse secret store {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported secret store version {version} in {}", .path.display())]
    UnsupportedVersion { path: PathBuf, version: u32 },
    #[error("failed to decode secret `{id}`")]
    Decode { id: String },
    #[error("secret `{id}` was not found")]
    NotFound { id: String },
    #[error("failed to create secret store directory {}: {source}", .path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write secret store {}: {source}", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SecretId(String);

impl SecretId {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SecretId").field(&self.0).finish()
    }
}

impl fmt::Display for SecretId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([redacted])")
    }
}

#[derive(Debug, Clone)]
pub struct SecretStore {
    path: PathBuf,
    key: [u8; KEY_LEN],
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoreFile {
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    nonce: String,
    ciphertext: String,
    tag: String,
}

impl SecretStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        let path = data_dir.join("secrets").join("store.json");
        Self::at_path_with_key(path, derive_key(data_dir))
    }

    pub fn at_path_for_tests(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        Self::at_path_with_key(
            path.clone(),
            derive_key(path.parent().unwrap_or(Path::new(""))),
        )
    }

    pub fn at_path_with_key(path: PathBuf, key: [u8; KEY_LEN]) -> Self {
        Self { path, key }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list_ids(&self) -> Result<Vec<SecretId>> {
        let file = self.load()?;
        file.entries.into_keys().map(SecretId::parse).collect()
    }

    pub fn get(&self, id: &str) -> Result<SecretValue> {
        let id = SecretId::parse(id.to_string())?;
        let file = self.load()?;
        let entry = file
            .entries
            .get(id.as_str())
            .ok_or_else(|| Error::NotFound { id: id.to_string() })?;
        let plaintext = decrypt_entry(&self.key, &id, entry)?;
        Ok(SecretValue::new(
            String::from_utf8(plaintext).map_err(|_| Error::Decode { id: id.to_string() })?,
        ))
    }

    pub fn set(&self, id: &str, value: SecretValue) -> Result<()> {
        let id = SecretId::parse(id.to_string())?;
        let mut file = self.load()?;
        file.entries.insert(
            id.to_string(),
            encrypt_entry(&self.key, &id, value.expose_secret().as_bytes()),
        );
        self.save(&file)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let id = SecretId::parse(id.to_string())?;
        let mut file = self.load()?;
        let removed = file.entries.remove(id.as_str()).is_some();
        if removed {
            self.save(&file)?;
        }
        Ok(removed)
    }

    fn load(&self) -> Result<StoreFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                let file: StoreFile =
                    serde_json::from_str(&text).map_err(|source| Error::Parse {
                        path: self.path.clone(),
                        source,
                    })?;
                if file.version != STORE_VERSION {
                    return Err(Error::UnsupportedVersion {
                        path: self.path.clone(),
                        version: file.version,
                    });
                }
                Ok(file)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile {
                version: STORE_VERSION,
                entries: BTreeMap::new(),
            }),
            Err(source) => Err(Error::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn save(&self, file: &StoreFile) -> Result<()> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|source| Error::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
        let data = serde_json::to_vec_pretty(file).map_err(|source| Error::Write {
            path: self.path.clone(),
            source: std::io::Error::other(source),
        })?;
        let tmp = self.temp_path();
        {
            let mut fh = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp)
                .map_err(|source| Error::Write {
                    path: tmp.clone(),
                    source,
                })?;
            fh.write_all(&data).map_err(|source| Error::Write {
                path: tmp.clone(),
                source,
            })?;
            fh.sync_all().map_err(|source| Error::Write {
                path: tmp.clone(),
                source,
            })?;
        }
        fs::rename(&tmp, &self.path).map_err(|source| Error::Write {
            path: self.path.clone(),
            source,
        })?;
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    fn temp_path(&self) -> PathBuf {
        let suffix = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file_name = self
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("store.json");
        self.path.with_file_name(format!(
            ".{file_name}.{}.{}.{}.tmp",
            std::process::id(),
            now,
            suffix
        ))
    }
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(Error::EmptyId);
    }
    if id.len() > MAX_ID_LEN {
        return Err(Error::IdTooLong {
            id: id.to_string(),
            max: MAX_ID_LEN,
        });
    }
    if id.starts_with('/') || id.starts_with('~') || id.contains("//") {
        return Err(Error::UnsafeId(id.to_string()));
    }
    for component in id.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(Error::UnsafeId(id.to_string()));
        }
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(Error::UnsupportedIdChars(id.to_string()));
    }
    Ok(())
}

fn derive_key(data_dir: &Path) -> [u8; KEY_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(b"insomnia local secret store obfuscation key v1");
    hasher.update(data_dir.as_os_str().as_encoded_bytes());
    hasher.finalize().into()
}

fn encrypt_entry(key: &[u8; KEY_LEN], id: &SecretId, plaintext: &[u8]) -> Entry {
    let nonce = make_nonce(id.as_str(), plaintext);
    let ciphertext = xor_stream(key, &nonce, plaintext);
    let tag = tag(key, id.as_str(), &nonce, &ciphertext);
    Entry {
        nonce: hex_encode(&nonce),
        ciphertext: hex_encode(&ciphertext),
        tag: hex_encode(&tag),
    }
}

fn decrypt_entry(key: &[u8; KEY_LEN], id: &SecretId, entry: &Entry) -> Result<Vec<u8>> {
    let nonce = hex_decode(&entry.nonce).map_err(|_| Error::Decode { id: id.to_string() })?;
    let ciphertext =
        hex_decode(&entry.ciphertext).map_err(|_| Error::Decode { id: id.to_string() })?;
    let actual_tag = hex_decode(&entry.tag).map_err(|_| Error::Decode { id: id.to_string() })?;
    let expected = tag(key, id.as_str(), &nonce, &ciphertext);
    if actual_tag.as_slice() != expected {
        return Err(Error::Decode { id: id.to_string() });
    }
    Ok(xor_stream(key, &nonce, &ciphertext))
}

fn make_nonce(id: &str, plaintext: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hasher.update(b"insomnia nonce v1");
    hasher.update(now.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(NONCE_COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.update(id.as_bytes());
    hasher.update(plaintext);
    hasher.finalize()[..16].to_vec()
}

fn xor_stream(key: &[u8; KEY_LEN], nonce: &[u8], input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut counter = 0u64;
    for chunk in input.chunks(KEY_LEN) {
        let mut hasher = Sha256::new();
        hasher.update(b"insomnia secret keystream v1");
        hasher.update(key);
        hasher.update(nonce);
        hasher.update(counter.to_le_bytes());
        let block = hasher.finalize();
        for (b, k) in chunk.iter().zip(block.iter()) {
            out.push(b ^ k);
        }
        counter += 1;
    }
    out
}

fn tag(key: &[u8; KEY_LEN], id: &str, nonce: &[u8], ciphertext: &[u8]) -> [u8; TAG_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(b"insomnia secret tag v1");
    hasher.update(key);
    hasher.update(id.as_bytes());
    hasher.update(nonce);
    hasher.update(ciphertext);
    hasher.finalize().into()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(s: &str) -> std::result::Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_value(b: u8) -> std::result::Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (tempfile::TempDir, SecretStore) {
        let dir = tempfile::tempdir().unwrap();
        let store =
            SecretStore::at_path_with_key(dir.path().join("secrets/store.json"), [7u8; KEY_LEN]);
        (dir, store)
    }

    #[test]
    fn roundtrip_list_delete() {
        let (_dir, store) = test_store();
        store
            .set("anthropic/default", SecretValue::new("sk-test-secret"))
            .unwrap();
        assert_eq!(
            store.get("anthropic/default").unwrap().expose_secret(),
            "sk-test-secret"
        );
        assert_eq!(store.list_ids().unwrap()[0].as_str(), "anthropic/default");
        assert!(store.delete("anthropic/default").unwrap());
        assert!(matches!(
            store.get("anthropic/default"),
            Err(Error::NotFound { .. })
        ));
    }

    #[test]
    fn invalid_ids_are_rejected() {
        for id in ["", "/abs", "../x", "x/../y", "x//y", "x y", "x\ny", "~home"] {
            assert!(SecretId::parse(id).is_err(), "{id:?} should be invalid");
        }
        assert!(SecretId::parse("a".repeat(MAX_ID_LEN + 1)).is_err());
        assert!(SecretId::parse("web/brave.default-1").is_ok());
    }

    #[test]
    fn corrupted_store_fails_closed() {
        let (dir, store) = test_store();
        store
            .set("web/brave", SecretValue::new("secret-value"))
            .unwrap();
        let path = dir.path().join("secrets/store.json");
        let mut file: StoreFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entry = file.entries.get_mut("web/brave").unwrap();
        let replacement = if entry.ciphertext.starts_with('0') {
            "1"
        } else {
            "0"
        };
        entry.ciphertext.replace_range(0..1, replacement);
        fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        assert!(matches!(store.get("web/brave"), Err(Error::Decode { id }) if id == "web/brave"));
    }

    #[test]
    fn plaintext_is_not_written_to_disk_or_debug() {
        let (dir, store) = test_store();
        let value = SecretValue::new("sk-plain-must-not-appear");
        assert!(!format!("{value:?}").contains("sk-plain"));
        store.set("provider/test", value).unwrap();
        let text = fs::read_to_string(dir.path().join("secrets/store.json")).unwrap();
        assert!(!text.contains("sk-plain-must-not-appear"));
        assert!(text.contains("provider/test"));
    }

    #[test]
    fn wrong_key_or_tamper_fails_decode() {
        let (dir, store) = test_store();
        store
            .set("provider/test", SecretValue::new("secret-value"))
            .unwrap();
        let wrong =
            SecretStore::at_path_with_key(dir.path().join("secrets/store.json"), [9u8; KEY_LEN]);
        assert!(
            matches!(wrong.get("provider/test"), Err(Error::Decode { id }) if id == "provider/test")
        );
    }
}
