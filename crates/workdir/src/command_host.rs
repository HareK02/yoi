//! Host-owned command budgets, shared by all sessions opened by one provider.
//!
//! Configuration is resolved at the host startup boundary, never from a command
//! request or its environment overlay. Each admitted command reserves its entire
//! limit, so the provider's simultaneous upper bound is `memory_max * parallel`.
//! Operators must budget the sum across Runtime and sharing CLI processes, other
//! workloads, and control-process headroom; this is not a host-global OOM guarantee.
//!
//! Linux isolation uses sibling user scopes under `app.slice`, not descendants of
//! the Runtime's tmux scope. The common `user@UID.service` must use OOMPolicy=continue;
//! changing that policy belongs to the operator, never this provider.

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// One provider's shared execution budget, not a Workdir or model capability.
/// Install the same `Arc` on all of the host's sessions, including reconnects.
/// The portable default is unconfined; requiring systemd never permits fallback.
#[derive(Debug)]
pub struct CommandHost {
    pub(crate) memory_max: Option<u64>,
    permits: Arc<Semaphore>,
}

impl Default for CommandHost {
    fn default() -> Self {
        Self {
            memory_max: None,
            permits: Arc::new(Semaphore::new(64)),
        }
    }
}

impl CommandHost {
    pub fn systemd(memory_max: u64, parallel: usize) -> Result<Arc<Self>, String> {
        if memory_max < 16 * 1024 * 1024
            || parallel == 0
            || parallel > 64
            || memory_max.checked_mul(parallel as u64).is_none()
        {
            return Err("command budget requires memory_max >= 16 MiB, parallel 1..=64, and a representable total".into());
        }
        if !cfg!(target_os = "linux") {
            return Err("systemd command isolation requires Linux".into());
        }
        Ok(Arc::new(Self {
            memory_max: Some(memory_max),
            permits: Arc::new(Semaphore::new(parallel)),
        }))
    }

    /// Resolve once in the Runtime or sharing CLI, before accepting commands.
    /// Disabled is the portable default; it explicitly offers no memory isolation.
    /// Required systemd mode has no fallback. Limits have no implicit RAM-size guess.
    pub fn from_environment() -> Result<Arc<Self>, String> {
        fn read(name: &str) -> Result<Option<String>, String> {
            std::env::var_os(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| format!("{name} must contain UTF-8"))
                })
                .transpose()
        }
        let mode = read("YOI_WORKDIR_COMMAND_ISOLATION")?;
        let memory = read("YOI_WORKDIR_COMMAND_MEMORY_MAX")?;
        let parallel = read("YOI_WORKDIR_COMMAND_PARALLEL")?;
        Self::from_values(mode.as_deref(), memory.as_deref(), parallel.as_deref())
    }

    fn from_values(
        mode: Option<&str>,
        memory: Option<&str>,
        parallel: Option<&str>,
    ) -> Result<Arc<Self>, String> {
        match mode.unwrap_or("disabled") {
            "disabled" if memory.is_none() && parallel.is_none() => {
                tracing::warn!("Workdir commands are unconfined: memory isolation is disabled on this host");
                Ok(Arc::new(Self::default()))
            }
            "systemd" => Self::systemd(
                memory.and_then(|s| s.parse().ok()).ok_or("systemd command isolation requires YOI_WORKDIR_COMMAND_MEMORY_MAX in bytes")?,
                parallel.and_then(|s| s.parse().ok()).ok_or("systemd command isolation requires YOI_WORKDIR_COMMAND_PARALLEL")?,
            ),
            _ => Err("invalid Workdir command isolation configuration; use disabled without limits, or systemd with memory/parallel limits".into()),
        }
    }

    pub(crate) fn admit(&self) -> Result<OwnedSemaphorePermit, crate::WorkdirError> {
        self.permits.clone().try_acquire_owned().map_err(|_| crate::WorkdirError::Unavailable(
            "host command reservations exhausted (running or unconfirmed cleanup); inspect host units and provider diagnostics".into()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_configuration_rejects_incomplete_or_unbounded_budgets() {
        for (mode, memory, parallel) in [
            ("other", None, None),
            ("disabled", Some("1"), None),
            ("systemd", None, Some("1")),
            ("systemd", Some("16777216"), None),
            ("systemd", Some("0"), Some("1")),
            ("systemd", Some("16777216"), Some("0")),
            ("systemd", Some("16777216"), Some("65")),
            ("systemd", Some("18446744073709551615"), Some("2")),
        ] {
            assert!(CommandHost::from_values(Some(mode), memory, parallel).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn configured_parallel_budget_reserves_a_whole_command_limit_per_slot() {
        let host = CommandHost::from_values(Some("systemd"), Some("33554432"), Some("2")).unwrap();
        let first = host.admit().unwrap();
        let second = host.admit().unwrap();
        assert!(host.admit().is_err());
        drop(first);
        assert!(host.admit().is_ok());
        drop(second);
    }

    #[test]
    fn reservations_are_shared_and_released_without_queuing() {
        let host = CommandHost::default();
        let permits: Vec<_> = (0..64).map(|_| host.admit().unwrap()).collect();
        assert!(host.admit().is_err());
        drop(permits);
        assert!(host.admit().is_ok());
    }
}
