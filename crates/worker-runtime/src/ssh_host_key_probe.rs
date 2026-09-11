//! Side-effect-free SSH host key discovery for Repository trust enrollment.
//!
//! Probing only observes public host keys. It does not persist trust, use clone
//! credentials, or authenticate to the target host.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub const SSH_HOST_KEY_PROBE_PATH: &str = "/v1/repositories/ssh/probe";
pub const SSH_HOST_KEY_PROBE_OPERATION: &str = "workdirs:operate";
pub(crate) const SSH_KEYSCAN_TIMEOUT: Duration = Duration::from_secs(10);
const SSH_KEYSCAN_CONNECT_TIMEOUT_SECONDS: &str = "5";
const MAX_SSH_KEYSCAN_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_PROBE_CANDIDATES: usize = 32;
const MAX_DIAGNOSTIC_BYTES: usize = 256;

/// `POST /v1/repositories/ssh/probe` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostKeyProbeRequest {
    pub hostname: String,
    pub port: u16,
}

/// One public host key observed by an SSH host key probe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostKeyCandidate {
    /// Canonical OpenSSH public key text (`algorithm base64-key`), without a host prefix.
    pub public_key: String,
    /// OpenSSH public key algorithm name.
    pub algorithm: String,
    /// OpenSSH SHA-256 fingerprint (`SHA256:base64-digest`).
    pub fingerprint: String,
}

/// `POST /v1/repositories/ssh/probe` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostKeyProbeResponse {
    pub candidates: Vec<SshHostKeyCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SshHostKeyProbeError {
    #[error("SSH host key probe hostname is invalid")]
    InvalidHostname,
    #[error("SSH host key probe port must be greater than zero")]
    InvalidPort,
    #[error("SSH host key probe executable is unavailable")]
    Unavailable,
    #[error("SSH host key probe timed out")]
    Timeout,
    #[error("SSH host key probe failed: {diagnostic}")]
    Failed { diagnostic: String },
}

/// Observe the target's public Ed25519 host keys without persisting trust or using credentials.
pub async fn probe_ssh_host_keys(
    request: &SshHostKeyProbeRequest,
) -> Result<SshHostKeyProbeResponse, SshHostKeyProbeError> {
    probe_ssh_host_keys_with_program(request, Path::new("ssh-keyscan"), SSH_KEYSCAN_TIMEOUT).await
}

pub(crate) async fn probe_ssh_host_keys_with_program(
    request: &SshHostKeyProbeRequest,
    program: &Path,
    timeout: Duration,
) -> Result<SshHostKeyProbeResponse, SshHostKeyProbeError> {
    validate_request(request)?;

    let mut command = Command::new(program);
    command
        .args(["-T", SSH_KEYSCAN_CONNECT_TIMEOUT_SECONDS])
        .arg("-p")
        .arg(request.port.to_string())
        .args(["-t", "ed25519"])
        .arg(&request.hostname)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // ssh-keyscan diagnostics are intentionally not returned or retained: they may contain
        // environment-specific details and are not needed for the public error contract.
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| SshHostKeyProbeError::Timeout)?
        .map_err(|_| SshHostKeyProbeError::Unavailable)?;

    if !output.status.success() {
        return Err(SshHostKeyProbeError::Failed {
            diagnostic: bounded_diagnostic(format!(
                "ssh-keyscan exited unsuccessfully ({})",
                output.status
            )),
        });
    }
    if output.stdout.len() > MAX_SSH_KEYSCAN_OUTPUT_BYTES {
        return Err(SshHostKeyProbeError::Failed {
            diagnostic: "ssh-keyscan output exceeded the probe limit".to_string(),
        });
    }

    let candidates = parse_ssh_keyscan_output(&output.stdout);
    if candidates.is_empty() {
        return Err(SshHostKeyProbeError::Failed {
            diagnostic: "ssh-keyscan returned no valid ssh-ed25519 host keys".to_string(),
        });
    }
    Ok(SshHostKeyProbeResponse { candidates })
}

fn validate_request(request: &SshHostKeyProbeRequest) -> Result<(), SshHostKeyProbeError> {
    if request.port == 0 {
        return Err(SshHostKeyProbeError::InvalidPort);
    }
    validate_hostname(&request.hostname)
}

fn validate_hostname(hostname: &str) -> Result<(), SshHostKeyProbeError> {
    if hostname.is_empty()
        || hostname.len() > 253
        || !hostname.is_ascii()
        || hostname.bytes().any(|byte| byte.is_ascii_whitespace())
        || hostname.starts_with('-')
    {
        return Err(SshHostKeyProbeError::InvalidHostname);
    }
    if hostname.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    let hostname = hostname.strip_suffix('.').unwrap_or(hostname);
    if hostname.is_empty()
        || hostname.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(SshHostKeyProbeError::InvalidHostname);
    }
    Ok(())
}

fn parse_ssh_keyscan_output(output: &[u8]) -> Vec<SshHostKeyCandidate> {
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let mut fields = line.split_ascii_whitespace();
        let (Some(_host), Some(algorithm), Some(encoded_key)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if line.trim_start().starts_with('#') || algorithm != "ssh-ed25519" {
            continue;
        }
        let Ok(key_blob) = STANDARD.decode(encoded_key) else {
            continue;
        };
        if !is_ed25519_public_key_blob(&key_blob) {
            continue;
        }

        let canonical_key = STANDARD.encode(&key_blob);
        if !seen.insert(canonical_key.clone()) {
            continue;
        }
        let public_key = format!("{algorithm} {canonical_key}");
        candidates.push(SshHostKeyCandidate {
            algorithm: algorithm.to_string(),
            fingerprint: format!(
                "SHA256:{}",
                STANDARD_NO_PAD.encode(Sha256::digest(&key_blob))
            ),
            public_key,
        });
        if candidates.len() == MAX_PROBE_CANDIDATES {
            break;
        }
    }
    candidates
}

fn is_ed25519_public_key_blob(blob: &[u8]) -> bool {
    let Some((algorithm, rest)) = take_ssh_string(blob) else {
        return false;
    };
    let Some((public_key, rest)) = take_ssh_string(rest) else {
        return false;
    };
    algorithm == b"ssh-ed25519" && public_key.len() == 32 && rest.is_empty()
}

fn take_ssh_string(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = u32::from_be_bytes(input.get(..4)?.try_into().ok()?) as usize;
    let value = input.get(4..4usize.checked_add(length)?)?;
    let rest = input.get(4usize.checked_add(length)?..)?;
    Some((value, rest))
}

fn bounded_diagnostic(mut diagnostic: String) -> String {
    if diagnostic.len() <= MAX_DIAGNOSTIC_BYTES {
        return diagnostic;
    }
    let mut end = MAX_DIAGNOSTIC_BYTES;
    while !diagnostic.is_char_boundary(end) {
        end -= 1;
    }
    diagnostic.truncate(end);
    diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_ed25519_key(seed: u8) -> String {
        let mut blob = Vec::new();
        blob.extend_from_slice(&("ssh-ed25519".len() as u32).to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&32_u32.to_be_bytes());
        blob.extend_from_slice(&[seed; 32]);
        STANDARD.encode(blob)
    }

    #[test]
    fn hostname_validation_rejects_option_injection_and_ambiguous_text() {
        for hostname in [
            "",
            "-example.test",
            "--help",
            "example.test other.test",
            "example.test\nother.test",
            "example_test",
            ".example.test",
            "example..test",
            "example.test:22",
            "[::1]",
            "éxample.test",
        ] {
            assert_eq!(
                validate_hostname(hostname),
                Err(SshHostKeyProbeError::InvalidHostname),
                "{hostname:?} must be rejected"
            );
        }
        for hostname in [
            "localhost",
            "example.test",
            "example.test.",
            "127.0.0.1",
            "::1",
        ] {
            validate_hostname(hostname).unwrap();
        }
    }

    #[test]
    fn request_validation_rejects_zero_port() {
        assert_eq!(
            validate_request(&SshHostKeyProbeRequest {
                hostname: "example.test".to_string(),
                port: 0,
            }),
            Err(SshHostKeyProbeError::InvalidPort)
        );
    }

    #[test]
    fn parser_accepts_only_valid_ed25519_keys_and_deduplicates() {
        let key = encoded_ed25519_key(7);
        let other_key = encoded_ed25519_key(8);
        let output = format!(
            "# comment\nexample.test ssh-rsa AAAA\nexample.test ssh-ed25519 invalid!\nexample.test ssh-ed25519 {key}\n[example.test]:2222 ssh-ed25519 {key}\nexample.test ssh-ed25519 {other_key}\n"
        );

        let candidates = parse_ssh_keyscan_output(output.as_bytes());

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].algorithm, "ssh-ed25519");
        assert_eq!(candidates[0].public_key, format!("ssh-ed25519 {key}"));
        let decoded = STANDARD.decode(key).unwrap();
        assert_eq!(
            candidates[0].fingerprint,
            format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(decoded)))
        );
    }

    #[test]
    fn parser_rejects_base64_that_is_not_an_ed25519_wire_key() {
        let output = format!("example.test ssh-ed25519 {}\n", STANDARD.encode([1_u8; 32]));
        assert!(parse_ssh_keyscan_output(output.as_bytes()).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unsuccessful_command_does_not_return_stderr() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let program = temp.path().join("ssh-keyscan");
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf 'secret from stderr' >&2\nexit 7\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let error = probe_ssh_host_keys_with_program(
            &SshHostKeyProbeRequest {
                hostname: "example.test".to_string(),
                port: 22,
            },
            &program,
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();

        let diagnostic = error.to_string();
        assert!(matches!(error, SshHostKeyProbeError::Failed { .. }));
        assert!(!diagnostic.contains("secret"));
        assert!(diagnostic.len() <= MAX_DIAGNOSTIC_BYTES + "SSH host key probe failed: ".len());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_execution_times_out_without_returning_process_diagnostics() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap();
        let program = temp.path().join("ssh-keyscan");
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf 'secret from stderr' >&2\nsleep 2\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let request = SshHostKeyProbeRequest {
            hostname: "example.test".to_string(),
            port: 22,
        };

        let error = probe_ssh_host_keys_with_program(&request, &program, Duration::from_millis(20))
            .await
            .unwrap_err();

        assert_eq!(error, SshHostKeyProbeError::Timeout);
        assert!(!error.to_string().contains("secret"));
    }
}
