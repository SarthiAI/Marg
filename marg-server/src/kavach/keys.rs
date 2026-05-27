//! Marg's Kavach signing keypair: persistent across restarts, mode 0640.
//!
//! Kavach's `KavachKeyPair` does not derive `Serialize`/`Deserialize`. We hold
//! the persisted form as a small Marg-side struct with base64-encoded byte
//! vectors. On first boot, if `keypair_path` is missing we generate, write,
//! and chmod 0640 (root:marg in production via the systemd unit). On every
//! subsequent boot we read the file back.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use data_encoding::BASE64;
use kavach_pq::KavachKeyPair;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// JSON layout written to `[kavach].keypair_path`. Bumping the version is a
/// breaking change; document and provide a migration when that happens.
#[derive(Debug, Serialize, Deserialize)]
pub struct MargKavachKeyFile {
    pub version: u32,
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub ml_dsa_signing_key_b64: String,
    pub ml_dsa_verifying_key_b64: String,
    pub ml_kem_decapsulation_key_b64: String,
    pub ml_kem_encapsulation_key_b64: String,
    pub ed25519_signing_key_b64: String,
    pub ed25519_verifying_key_b64: String,
    pub x25519_secret_key_b64: String,
    pub x25519_public_key_b64: String,
}

impl MargKavachKeyFile {
    /// Convert this file form back into a Kavach `KavachKeyPair`. The Kavach
    /// type does `Zeroize` on drop, so the in-memory secret bytes are scrubbed
    /// when the runtime shuts down.
    pub fn into_keypair(self) -> Result<KavachKeyPair> {
        let dec = |s: &str, label: &str| -> Result<Vec<u8>> {
            BASE64
                .decode(s.as_bytes())
                .with_context(|| format!("decoding {} from key file", label))
        };
        Ok(KavachKeyPair {
            id: self.id.clone(),
            created_at: self.created_at,
            expires_at: None,
            ml_dsa_signing_key: dec(&self.ml_dsa_signing_key_b64, "ml_dsa_signing_key")?,
            ml_dsa_verifying_key: dec(&self.ml_dsa_verifying_key_b64, "ml_dsa_verifying_key")?,
            ml_kem_decapsulation_key: dec(
                &self.ml_kem_decapsulation_key_b64,
                "ml_kem_decapsulation_key",
            )?,
            ml_kem_encapsulation_key: dec(
                &self.ml_kem_encapsulation_key_b64,
                "ml_kem_encapsulation_key",
            )?,
            ed25519_signing_key: dec(&self.ed25519_signing_key_b64, "ed25519_signing_key")?,
            ed25519_verifying_key: dec(&self.ed25519_verifying_key_b64, "ed25519_verifying_key")?,
            x25519_secret_key: dec(&self.x25519_secret_key_b64, "x25519_secret_key")?,
            x25519_public_key: dec(&self.x25519_public_key_b64, "x25519_public_key")?,
        })
    }

    pub fn from_keypair(kp: &KavachKeyPair) -> Self {
        Self {
            version: 1,
            id: kp.id.clone(),
            created_at: kp.created_at,
            ml_dsa_signing_key_b64: BASE64.encode(&kp.ml_dsa_signing_key),
            ml_dsa_verifying_key_b64: BASE64.encode(&kp.ml_dsa_verifying_key),
            ml_kem_decapsulation_key_b64: BASE64.encode(&kp.ml_kem_decapsulation_key),
            ml_kem_encapsulation_key_b64: BASE64.encode(&kp.ml_kem_encapsulation_key),
            ed25519_signing_key_b64: BASE64.encode(&kp.ed25519_signing_key),
            ed25519_verifying_key_b64: BASE64.encode(&kp.ed25519_verifying_key),
            x25519_secret_key_b64: BASE64.encode(&kp.x25519_secret_key),
            x25519_public_key_b64: BASE64.encode(&kp.x25519_public_key),
        }
    }
}

/// Load the Kavach keypair from `path`, generating + persisting it on first
/// boot. The file is written mode 0640 on unix (matches the `marg.key` line
/// in the shipped systemd unit). The parent directory is created if missing.
pub fn load_or_generate_keypair(path: &str) -> Result<KavachKeyPair> {
    let p = Path::new(path);
    if p.exists() {
        let text = std::fs::read_to_string(p)
            .with_context(|| format!("reading kavach keypair at {}", p.display()))?;
        let file: MargKavachKeyFile = serde_json::from_str(&text)
            .with_context(|| format!("parsing kavach keypair at {}", p.display()))?;
        if file.version != 1 {
            return Err(anyhow!(
                "unsupported kavach keypair file version {}, expected 1",
                file.version
            ));
        }
        let kp = file
            .into_keypair()
            .with_context(|| "rehydrating kavach keypair from file")?;
        tracing::info!(
            key_id = %kp.id,
            path = %p.display(),
            "loaded kavach signing keypair"
        );
        Ok(kp)
    } else {
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("creating kavach keypair parent dir {}", parent.display())
                })?;
            }
        }
        let kp = KavachKeyPair::generate()
            .map_err(|e| anyhow!("generating kavach keypair: {}", e))?;
        let file = MargKavachKeyFile::from_keypair(&kp);
        let text = serde_json::to_string_pretty(&file)
            .with_context(|| "serializing fresh kavach keypair")?;
        write_keyfile(p, &text)
            .with_context(|| format!("writing kavach keypair to {}", p.display()))?;
        tracing::warn!(
            key_id = %kp.id,
            path = %p.display(),
            "generated fresh kavach signing keypair (mode 0640). Back this file up: losing it makes the audit chain unverifiable"
        );
        Ok(kp)
    }
}

fn write_keyfile(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o640);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}
