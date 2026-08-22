//! Explicit, checksum-verified desktop update staging.

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures_util::StreamExt;
use homebot_protocol::PROTOCOL_VERSION;
use reqwest::{Client, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
    thread::JoinHandle,
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const MANIFEST_LIMIT: u64 = 64 * 1024;
const ARTIFACT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/luinbytes/HomeBot/releases/latest/download/HomeBot-release.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    schema_version: u16,
    product: String,
    version: String,
    platform: String,
    architecture: String,
    artifact: String,
    bytes: u64,
    sha256: String,
    signing: String,
    protocol_minimum: u16,
    protocol_maximum: u16,
    key_id: String,
    signature: String,
}

#[derive(Serialize)]
struct SignedManifestFields<'a> {
    schema_version: u16,
    product: &'a str,
    version: &'a str,
    platform: &'a str,
    architecture: &'a str,
    artifact: &'a str,
    bytes: u64,
    sha256: &'a str,
    signing: &'a str,
    protocol_minimum: u16,
    protocol_maximum: u16,
    key_id: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCandidate {
    pub version: String,
    artifact_url: Url,
    artifact_name: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug)]
pub enum UpdateCommand {
    Check,
    Stage(UpdateCandidate),
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateEvent {
    Current,
    Available(UpdateCandidate),
    Staged { version: String, path: PathBuf },
    Failed(String),
}

pub struct UpdateCoordinator {
    commands: Sender<UpdateCommand>,
    events: Receiver<UpdateEvent>,
    thread: Option<JoinHandle<()>>,
}

impl UpdateCoordinator {
    #[must_use]
    pub fn start(manifest_url: &str, staging_directory: PathBuf) -> Self {
        let (commands_tx, commands_rx) = channel();
        let (events_tx, events_rx) = channel();
        let manifest_url = manifest_url.to_owned();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else {
                let _ = events_tx.send(UpdateEvent::Failed(
                    "Update runtime could not start".to_owned(),
                ));
                return;
            };
            let client = Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::limited(3))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build();
            let Ok(client) = client else {
                let _ = events_tx.send(UpdateEvent::Failed(
                    "Update client could not start".to_owned(),
                ));
                return;
            };
            while let Ok(command) = commands_rx.recv() {
                let event = match command {
                    UpdateCommand::Check => {
                        runtime.block_on(check(&client, &manifest_url, env!("CARGO_PKG_VERSION")))
                    }
                    UpdateCommand::Stage(candidate) => runtime
                        .block_on(stage(&client, &candidate, &staging_directory))
                        .map(|path| UpdateEvent::Staged {
                            version: candidate.version,
                            path,
                        }),
                    UpdateCommand::Shutdown => break,
                }
                .unwrap_or_else(|error| UpdateEvent::Failed(error.to_string()));
                let _ = events_tx.send(event);
            }
        });
        Self {
            commands: commands_tx,
            events: events_rx,
            thread: Some(thread),
        }
    }

    pub fn send(&self, command: UpdateCommand) {
        let _ = self.commands.send(command);
    }

    pub fn try_events(&self) -> impl Iterator<Item = UpdateEvent> + '_ {
        self.events.try_iter()
    }
}

impl Drop for UpdateCoordinator {
    fn drop(&mut self) {
        let _ = self.commands.send(UpdateCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum UpdateError {
    #[error("The update manifest is unavailable")]
    Unavailable,
    #[error("The update manifest is invalid")]
    InvalidManifest,
    #[error("This update is incompatible with this HomeBot client")]
    Incompatible,
    #[error("The update artifact failed integrity verification")]
    Integrity,
    #[error("The update could not be staged safely")]
    Staging,
}

async fn check(
    client: &Client,
    manifest_url: &str,
    current_version: &str,
) -> Result<UpdateEvent, UpdateError> {
    let url = Url::parse(manifest_url).map_err(|_| UpdateError::InvalidManifest)?;
    if url.scheme() != "https" {
        return Err(UpdateError::InvalidManifest);
    }
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|_| UpdateError::Unavailable)?
        .error_for_status()
        .map_err(|_| UpdateError::Unavailable)?;
    let bytes = bounded_response(response, MANIFEST_LIMIT).await?;
    let manifest: ReleaseManifest =
        serde_json::from_slice(&bytes).map_err(|_| UpdateError::InvalidManifest)?;
    let public_key = release_public_key()?;
    validate_manifest(manifest, &url, current_version, &public_key)
}

fn validate_manifest(
    manifest: ReleaseManifest,
    manifest_url: &Url,
    current_version: &str,
    public_key: &VerifyingKey,
) -> Result<UpdateEvent, UpdateError> {
    verify_manifest_signature(&manifest, public_key)?;
    if manifest.schema_version != 2
        || manifest.product != "HomeBot"
        || manifest.platform != target_platform()
        || manifest.architecture != target_architecture()
        || manifest.protocol_minimum > PROTOCOL_VERSION
        || manifest.protocol_maximum < PROTOCOL_VERSION
        || manifest.bytes == 0
        || manifest.bytes > ARTIFACT_LIMIT
        || manifest.sha256.len() != 64
        || !manifest.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !matches!(manifest.signing.as_str(), "developer-id" | "package")
        || manifest.artifact.is_empty()
        || manifest.artifact.contains('/')
        || manifest.artifact.contains('\\')
        || manifest.artifact == "."
        || manifest.artifact == ".."
    {
        return Err(UpdateError::Incompatible);
    }
    let current = Version::parse(current_version).map_err(|_| UpdateError::InvalidManifest)?;
    let offered = Version::parse(&manifest.version).map_err(|_| UpdateError::InvalidManifest)?;
    if offered <= current {
        return Ok(UpdateEvent::Current);
    }
    let artifact_url = manifest_url
        .join(&manifest.artifact)
        .map_err(|_| UpdateError::InvalidManifest)?;
    if artifact_url.scheme() != "https" || artifact_url.host_str() != manifest_url.host_str() {
        return Err(UpdateError::InvalidManifest);
    }
    Ok(UpdateEvent::Available(UpdateCandidate {
        version: manifest.version,
        artifact_url,
        artifact_name: manifest.artifact,
        bytes: manifest.bytes,
        sha256: manifest.sha256.to_ascii_lowercase(),
    }))
}

fn release_public_key() -> Result<VerifyingKey, UpdateError> {
    let encoded =
        option_env!("HOMEBOT_UPDATE_PUBLIC_KEY_HEX").ok_or(UpdateError::InvalidManifest)?;
    let decoded = decode_hex_32(encoded).ok_or(UpdateError::InvalidManifest)?;
    VerifyingKey::from_bytes(&decoded).map_err(|_| UpdateError::InvalidManifest)
}

fn verify_manifest_signature(
    manifest: &ReleaseManifest,
    public_key: &VerifyingKey,
) -> Result<(), UpdateError> {
    let expected_key_id = format!("{:x}", Sha256::digest(public_key.as_bytes()));
    if manifest.key_id != expected_key_id[..16] {
        return Err(UpdateError::Integrity);
    }
    let signature_bytes = STANDARD
        .decode(&manifest.signature)
        .map_err(|_| UpdateError::Integrity)?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| UpdateError::Integrity)?;
    let payload = manifest_payload(manifest)?;
    public_key
        .verify(&payload, &signature)
        .map_err(|_| UpdateError::Integrity)
}

fn manifest_payload(manifest: &ReleaseManifest) -> Result<Vec<u8>, UpdateError> {
    serde_json::to_vec(&SignedManifestFields {
        schema_version: manifest.schema_version,
        product: &manifest.product,
        version: &manifest.version,
        platform: &manifest.platform,
        architecture: &manifest.architecture,
        artifact: &manifest.artifact,
        bytes: manifest.bytes,
        sha256: &manifest.sha256,
        signing: &manifest.signing,
        protocol_minimum: manifest.protocol_minimum,
        protocol_maximum: manifest.protocol_maximum,
        key_id: &manifest.key_id,
    })
    .map_err(|_| UpdateError::InvalidManifest)
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}

async fn stage(
    client: &Client,
    candidate: &UpdateCandidate,
    directory: &Path,
) -> Result<PathBuf, UpdateError> {
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|_| UpdateError::Staging)?;
    let final_path = directory.join(&candidate.artifact_name);
    if final_path.is_file() && verify_file(&final_path, candidate.bytes, &candidate.sha256).await? {
        return Ok(final_path);
    }
    let temporary = directory.join(format!(".homebot-update-{}.part", Uuid::now_v7()));
    let result = async {
        let response = client
            .get(candidate.artifact_url.clone())
            .send()
            .await
            .map_err(|_| UpdateError::Unavailable)?
            .error_for_status()
            .map_err(|_| UpdateError::Unavailable)?;
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await
            .map_err(|_| UpdateError::Staging)?;
        let mut digest = Sha256::new();
        let mut written = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| UpdateError::Unavailable)?;
            written = written
                .checked_add(u64::try_from(chunk.len()).map_err(|_| UpdateError::Integrity)?)
                .ok_or(UpdateError::Integrity)?;
            if written > candidate.bytes || written > ARTIFACT_LIMIT {
                return Err(UpdateError::Integrity);
            }
            digest.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| UpdateError::Staging)?;
        }
        file.sync_all().await.map_err(|_| UpdateError::Staging)?;
        drop(file);
        if written != candidate.bytes || format!("{:x}", digest.finalize()) != candidate.sha256 {
            return Err(UpdateError::Integrity);
        }
        tokio::fs::rename(&temporary, &final_path)
            .await
            .map_err(|_| UpdateError::Staging)?;
        Ok(final_path.clone())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(temporary).await;
    }
    result
}

async fn bounded_response(response: reqwest::Response, limit: u64) -> Result<Vec<u8>, UpdateError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(UpdateError::InvalidManifest);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| UpdateError::Unavailable)?;
        if bytes.len().saturating_add(chunk.len())
            > usize::try_from(limit).map_err(|_| UpdateError::InvalidManifest)?
        {
            return Err(UpdateError::InvalidManifest);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn verify_file(path: &Path, bytes: u64, expected: &str) -> Result<bool, UpdateError> {
    let content = tokio::fs::read(path)
        .await
        .map_err(|_| UpdateError::Staging)?;
    Ok(u64::try_from(content.len()).ok() == Some(bytes)
        && format!("{:x}", Sha256::digest(content)) == expected)
}

#[must_use]
pub fn default_staging_directory() -> PathBuf {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map_or_else(
            || PathBuf::from("updates"),
            |home| PathBuf::from(home).join("Library/Caches/HomeBot/updates"),
        )
    } else if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(cache).join("homebot/updates")
    } else {
        std::env::var_os("HOME").map_or_else(
            || PathBuf::from("updates"),
            |home| PathBuf::from(home).join(".cache/homebot/updates"),
        )
    }
}

const fn target_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

const fn target_architecture() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use ed25519_dalek::{Signer, SigningKey};

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7_u8; 32])
    }

    fn manifest(version: &str) -> ReleaseManifest {
        let key = signing_key();
        let key_id =
            format!("{:x}", Sha256::digest(key.verifying_key().as_bytes()))[..16].to_owned();
        let mut manifest = ReleaseManifest {
            schema_version: 2,
            product: "HomeBot".to_owned(),
            version: version.to_owned(),
            platform: target_platform().to_owned(),
            architecture: target_architecture().to_owned(),
            artifact: "HomeBot-update.tar.gz".to_owned(),
            bytes: 42,
            sha256: "a".repeat(64),
            signing: if cfg!(target_os = "macos") {
                "developer-id".to_owned()
            } else {
                "package".to_owned()
            },
            protocol_minimum: PROTOCOL_VERSION,
            protocol_maximum: PROTOCOL_VERSION,
            key_id,
            signature: String::new(),
        };
        manifest.signature = STANDARD.encode(
            key.sign(
                &manifest_payload(&manifest)
                    .unwrap_or_else(|error| panic!("manifest payload failed: {error}")),
            )
            .to_bytes(),
        );
        manifest
    }

    #[test]
    fn manifest_requires_newer_compatible_same_origin_artifact() -> Result<(), UpdateError> {
        let url =
            Url::parse("https://github.com/luinbytes/HomeBot/releases/download/v2/manifest.json")
                .map_err(|_| UpdateError::InvalidManifest)?;
        assert!(matches!(
            validate_manifest(
                manifest("2.0.0"),
                &url,
                "1.0.0",
                &signing_key().verifying_key(),
            )?,
            UpdateEvent::Available(_)
        ));
        assert_eq!(
            validate_manifest(
                manifest("1.0.0"),
                &url,
                "1.0.0",
                &signing_key().verifying_key(),
            )?,
            UpdateEvent::Current
        );
        let mut incompatible = manifest("2.0.0");
        incompatible.protocol_minimum = PROTOCOL_VERSION + 1;
        assert!(
            validate_manifest(incompatible, &url, "1.0.0", &signing_key().verifying_key()).is_err()
        );
        let mut traversal = manifest("2.0.0");
        traversal.artifact = "../HomeBot.tar.gz".to_owned();
        assert!(
            validate_manifest(traversal, &url, "1.0.0", &signing_key().verifying_key()).is_err()
        );
        let mut tampered = manifest("2.0.0");
        tampered.sha256 = "b".repeat(64);
        assert!(matches!(
            validate_manifest(tampered, &url, "1.0.0", &signing_key().verifying_key()),
            Err(UpdateError::Integrity)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn existing_staged_file_is_reused_only_after_exact_verification()
    -> Result<(), UpdateError> {
        let directory = tempfile::tempdir().map_err(|_| UpdateError::Staging)?;
        let path = directory.path().join("update.tar.gz");
        tokio::fs::write(&path, b"verified")
            .await
            .map_err(|_| UpdateError::Staging)?;
        assert!(verify_file(&path, 8, &format!("{:x}", Sha256::digest(b"verified"))).await?);
        assert!(!verify_file(&path, 7, &"0".repeat(64)).await?);
        Ok(())
    }

    #[tokio::test]
    async fn fake_release_server_stages_only_exact_artifact() -> Result<(), UpdateError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| UpdateError::Unavailable)?;
        let address = listener
            .local_addr()
            .map_err(|_| UpdateError::Unavailable)?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                Router::new().route("/update.tar.gz", get(|| async { b"verified".to_vec() })),
            )
            .await;
        });
        let directory = tempfile::tempdir().map_err(|_| UpdateError::Staging)?;
        let mut candidate = UpdateCandidate {
            version: "2.0.0".to_owned(),
            artifact_url: Url::parse(&format!("http://{address}/update.tar.gz"))
                .map_err(|_| UpdateError::InvalidManifest)?,
            artifact_name: "update.tar.gz".to_owned(),
            bytes: 8,
            sha256: format!("{:x}", Sha256::digest(b"verified")),
        };
        let staged = stage(&Client::new(), &candidate, directory.path()).await?;
        assert_eq!(
            tokio::fs::read(staged)
                .await
                .map_err(|_| UpdateError::Staging)?,
            b"verified"
        );

        candidate.artifact_name = "wrong.tar.gz".to_owned();
        candidate.sha256 = "0".repeat(64);
        assert!(
            stage(&Client::new(), &candidate, directory.path())
                .await
                .is_err()
        );
        let mut entries = tokio::fs::read_dir(directory.path())
            .await
            .map_err(|_| UpdateError::Staging)?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| UpdateError::Staging)?
        {
            assert!(!entry.file_name().to_string_lossy().ends_with(".part"));
        }
        server.abort();
        Ok(())
    }
}
