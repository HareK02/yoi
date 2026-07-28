use client::{BackendTarget, LocalTarget, Target, TargetKind};
use serde::Deserialize;

use super::{ParseError, resolve_backend_url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliConnectionRequirement {
    LocalOnly,
    BackendOnly,
    ConnectionAware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliCommand {
    DefaultTui,
    Workers,
    Resume,
    Panel,
    Keys,
    SetupModel,
    WorkerRuntime,
    WorkerCleanup,
    Ticket,
    Objective,
    Plugin,
    Mcp,
    MemoryLint,
    Session,
    WorkspaceServer,
    Login,
}

impl CliCommand {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            CliCommand::DefaultTui => "yoi",
            CliCommand::Workers => "yoi workers",
            CliCommand::Resume => "yoi resume",
            CliCommand::Panel => "yoi panel",
            CliCommand::Keys => "yoi keys",
            CliCommand::SetupModel => "yoi setup-model",
            CliCommand::WorkerRuntime => "yoi worker",
            CliCommand::WorkerCleanup => "yoi worker management",
            CliCommand::Ticket => "yoi ticket",
            CliCommand::Objective => "yoi objective",
            CliCommand::Plugin => "yoi plugin",
            CliCommand::Mcp => "yoi mcp",
            CliCommand::MemoryLint => "yoi memory lint",
            CliCommand::Session => "yoi session",
            CliCommand::WorkspaceServer => "yoi workspace/server",
            CliCommand::Login => "yoi login",
        }
    }

    pub(crate) fn connection_requirement(self) -> CliConnectionRequirement {
        match self {
            CliCommand::DefaultTui => CliConnectionRequirement::ConnectionAware,
            CliCommand::Workers | CliCommand::Login => CliConnectionRequirement::BackendOnly,
            CliCommand::Resume
            | CliCommand::Panel
            | CliCommand::Keys
            | CliCommand::SetupModel
            | CliCommand::WorkerRuntime
            | CliCommand::WorkerCleanup
            | CliCommand::Ticket
            | CliCommand::Objective
            | CliCommand::Plugin
            | CliCommand::Mcp
            | CliCommand::MemoryLint
            | CliCommand::Session
            | CliCommand::WorkspaceServer => CliConnectionRequirement::LocalOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub(crate) enum ClientDefaultConnection {
    Local,
    Backend,
}

impl Default for ClientDefaultConnection {
    fn default() -> Self {
        Self::Local
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliConnectionInput<'a> {
    LocalDefault,
    BackendTarget {
        explicit_backend_url: Option<String>,
        workspace_id: Option<&'a str>,
    },
}

pub(crate) trait CliConnectionResolver {
    fn resolve_connection(
        &self,
        command: CliCommand,
        input: CliConnectionInput<'_>,
    ) -> Result<Box<dyn Target>, ParseError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ClientConfigCliConnectionResolver;

impl CliConnectionResolver for ClientConfigCliConnectionResolver {
    fn resolve_connection(
        &self,
        command: CliCommand,
        input: CliConnectionInput<'_>,
    ) -> Result<Box<dyn Target>, ParseError> {
        match (command.connection_requirement(), input) {
            (CliConnectionRequirement::LocalOnly, CliConnectionInput::LocalDefault) => {
                Ok(Box::new(LocalTarget::new()))
            }
            (CliConnectionRequirement::LocalOnly, CliConnectionInput::BackendTarget { .. }) => {
                Err(ParseError(format!(
                    "{} uses a local connection target and cannot accept Backend target options",
                    command.display_name()
                )))
            }
            (CliConnectionRequirement::BackendOnly, CliConnectionInput::LocalDefault) => {
                Err(ParseError(format!(
                    "{} requires a Backend connection target",
                    command.display_name()
                )))
            }
            (
                CliConnectionRequirement::BackendOnly | CliConnectionRequirement::ConnectionAware,
                CliConnectionInput::BackendTarget {
                    explicit_backend_url,
                    workspace_id,
                },
            ) => Ok(Box::new(BackendTarget::new(
                resolve_backend_url(explicit_backend_url, workspace_id)?,
                workspace_id.map(str::to_string),
            ))),
            (CliConnectionRequirement::ConnectionAware, CliConnectionInput::LocalDefault) => {
                Ok(Box::new(LocalTarget::new()))
            }
        }
    }
}

pub(crate) fn resolve_local_cli_connection<R: CliConnectionResolver + ?Sized>(
    resolver: &R,
    command: CliCommand,
) -> Result<Box<dyn Target>, ParseError> {
    let target = resolver.resolve_connection(command, CliConnectionInput::LocalDefault)?;
    match target.kind() {
        TargetKind::Local => Ok(target),
        TargetKind::Backend => Err(ParseError(format!(
            "{} resolved a Backend target where a local target was required",
            command.display_name()
        ))),
    }
}

pub(crate) fn resolve_backend_cli_connection<R: CliConnectionResolver + ?Sized>(
    resolver: &R,
    command: CliCommand,
    explicit_backend_url: Option<String>,
    workspace_id: Option<&str>,
) -> Result<Box<dyn Target>, ParseError> {
    let target = resolver.resolve_connection(
        command,
        CliConnectionInput::BackendTarget {
            explicit_backend_url,
            workspace_id,
        },
    )?;
    match target.kind() {
        TargetKind::Backend => Ok(target),
        TargetKind::Local => Err(ParseError(format!(
            "{} resolved a local target where a Backend target was required",
            command.display_name()
        ))),
    }
}

pub(crate) fn backend_target_option_error_for_local_command(
    command: CliCommand,
    option: &str,
) -> ParseError {
    ParseError(format!(
        "{} uses a local connection target and cannot accept Backend target option `{option}`",
        command.display_name()
    ))
}

pub(crate) fn is_backend_target_option(arg: &str) -> bool {
    matches!(
        arg,
        "--backend" | "--workspace-id" | "--runtime-id" | "--runtime" | "--worker-id"
    ) || arg.starts_with("--backend=")
        || arg.starts_with("--workspace-id=")
        || arg.starts_with("--runtime-id=")
        || arg.starts_with("--runtime=")
        || arg.starts_with("--worker-id=")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_command_connection_requirements_are_explicit() {
        assert_eq!(
            CliCommand::DefaultTui.connection_requirement(),
            CliConnectionRequirement::ConnectionAware
        );
        assert_eq!(
            CliCommand::Workers.connection_requirement(),
            CliConnectionRequirement::BackendOnly
        );
        assert_eq!(
            CliCommand::Login.connection_requirement(),
            CliConnectionRequirement::BackendOnly
        );

        for command in [
            CliCommand::Resume,
            CliCommand::Panel,
            CliCommand::Keys,
            CliCommand::SetupModel,
            CliCommand::WorkerRuntime,
            CliCommand::WorkerCleanup,
            CliCommand::Ticket,
            CliCommand::Objective,
            CliCommand::Plugin,
            CliCommand::Mcp,
            CliCommand::MemoryLint,
            CliCommand::Session,
            CliCommand::WorkspaceServer,
        ] {
            assert_eq!(
                command.connection_requirement(),
                CliConnectionRequirement::LocalOnly,
                "{} should be local-only",
                command.display_name()
            );
        }
    }

    #[test]
    fn cli_connection_resolver_rejects_local_only_backend_target() {
        let resolver = ClientConfigCliConnectionResolver;
        let err = resolver
            .resolve_connection(
                CliCommand::Resume,
                CliConnectionInput::BackendTarget {
                    explicit_backend_url: Some("http://127.0.0.1:8787".to_string()),
                    workspace_id: None,
                },
            )
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "yoi resume uses a local connection target and cannot accept Backend target options"
        );
    }

    #[test]
    fn cli_connection_resolver_rejects_backend_only_local_default() {
        let resolver = ClientConfigCliConnectionResolver;
        let err = resolver
            .resolve_connection(CliCommand::Workers, CliConnectionInput::LocalDefault)
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "yoi workers requires a Backend connection target"
        );
    }

    #[test]
    fn cli_connection_resolver_selects_backend_for_connection_aware_target() {
        let resolver = ClientConfigCliConnectionResolver;
        let target = resolve_backend_cli_connection(
            &resolver,
            CliCommand::DefaultTui,
            Some("http://127.0.0.1:8787".to_string()),
            None,
        )
        .unwrap();

        assert_eq!(target.kind(), TargetKind::Backend);
        let workers = target
            .list_workers(client::WorkerListRequest::new(None))
            .unwrap();
        assert_eq!(workers.target.base_url, "http://127.0.0.1:8787");
        assert_eq!(workers.target.workspace_id, None);
    }
}
