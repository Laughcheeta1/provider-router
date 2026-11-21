//! Signed release metadata, platform matching, and download limits.

use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;

const METADATA_LIMIT: usize = 64 * 1024;
pub(super) const BINARY_LIMIT: u64 = 128 * 1024 * 1024;
// RevExt: 6
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    signed: String,
    signature: String,
}  // RevExt: 29
// RevExt: 7
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Release {
    format: u32,
    expires: DateTime<Utc>,
    pub(super) version: String,  // RevExt: 41
    os: String,
    arch: String,
    pub(super) artifact: String,
    pub(super) size: u64,
    pub(super) sha256: String,
}  // RevExt: 30
// RevExt: 23
fn artifact_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}  // RevExt: 36
// RevExt: 24
pub(super) fn transport(url: &reqwest::Url) -> Result<()> {
    let local = matches!(url.host_str(), Some("127.0.0.1" | "[::1]"));
    ensure!(  // RevExt: 168
  // RevExt: 145
        url.scheme() == "https" || url.scheme() == "http" && local,
        "The update feed requires HTTPS or a loopback test endpoint."
    );  // RevExt: 154
    ensure!(  // RevExt: 169
  // RevExt: 146
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "The update feed must be a plain URL."
    );  // RevExt: 155
    Ok(())
}  // RevExt: 37
// RevExt: 26
pub(super) async fn bounded_response(mut response: reqwest::Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
  // RevExt: 140
            bytes.len() + chunk.len() <= METADATA_LIMIT,
            "The update metadata is too large."
        );  // RevExt: 144
        bytes.extend_from_slice(&chunk);
    }  // RevExt: 62
    Ok(bytes)
}  // RevExt: 39
// RevExt: 104
pub(super) fn verified_release(bytes: &[u8], key: &[u8]) -> Result<Release> {
    let envelope: Envelope = serde_json::from_slice(bytes)?;
    let payload = STANDARD.decode(envelope.signed)?;
    let signature = STANDARD.decode(envelope.signature)?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&payload, &signature)
        .map_err(|_| anyhow::anyhow!("The release signature is invalid."))?;
    let release: Release = serde_json::from_slice(&payload)?;
    ensure!(  // RevExt: 171
  // RevExt: 150
        release.format == 1 && release.expires > Utc::now(),
        "The release metadata is incompatible or expired."
    );  // RevExt: 159
    ensure!(  // RevExt: 172
  // RevExt: 151
        release.os == std::env::consts::OS && release.arch == std::env::consts::ARCH,
        "The update targets a different platform."
    );  // RevExt: 160
    ensure!(  // RevExt: 173
  // RevExt: 152
        release.size > 0 && release.size <= BINARY_LIMIT,
        "The update exceeds the artifact size limit."
    );  // RevExt: 161
    ensure!(  // RevExt: 174
  // RevExt: 153
        artifact_name(&release.artifact),
        "Invalid update artifact name."
    );  // RevExt: 162
    Ok(release)
}  // RevExt: 107
// RevExt: 105
#[cfg(test)]
mod tests {
    use super::*;
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };  // RevExt: 167
// RevExt: 106
    #[test]
    fn release_metadata_requires_a_trusted_signature_and_valid_expiry() -> Result<()> {
        let private = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| anyhow::anyhow!("Could not generate a test signing key."))?;
        let key = Ed25519KeyPair::from_pkcs8(private.as_ref())
            .map_err(|_| anyhow::anyhow!("Could not load the test signing key."))?;
        let mut payload = serde_json::json!({
            "format": 1, "version": "1.2.3", "expires": "2099-01-01T00:00:00Z",
            "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "artifact": "provider-router", "size": 20, "sha256": "0".repeat(64)
        });
        let mut original = serde_json::to_vec(&payload)?;
        let signature = key.sign(&original);
        let envelope = |bytes: &[u8], sig: &[u8]| {
            serde_json::to_vec(&serde_json::json!({
                "signed": STANDARD.encode(bytes), "signature": STANDARD.encode(sig),
            }))
            .unwrap()
        };  // RevExt: 109
        let decode = |bytes: &[u8], sig: &[u8]| {
            verified_release(&envelope(bytes, sig), key.public_key().as_ref())
        };  // RevExt: 116
        assert_eq!(decode(&original, signature.as_ref())?.version, "1.2.3");
        payload["version"] = "1.2.4".into();
        original = serde_json::to_vec(&payload)?;
        assert!(decode(&original, signature.as_ref()).is_err());
        payload["expires"] = "2000-01-01T00:00:00Z".into();
        let expired = serde_json::to_vec(&payload)?;
        assert!(decode(&expired, key.sign(&expired).as_ref()).is_err());
        Ok(())  // RevExt: 111
    }  // RevExt: 110
}  // RevExt: 108
