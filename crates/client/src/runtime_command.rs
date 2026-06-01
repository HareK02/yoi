use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

const POD_RUNTIME_COMMAND_ENV: &str = "YOI_POD_RUNTIME_COMMAND";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodRuntimeCommand {
    pub program: PathBuf,
    pub prefix_args: Vec<OsString>,
}

impl PodRuntimeCommand {
    pub fn new(program: impl Into<PathBuf>, prefix_args: Vec<OsString>) -> Self {
        Self {
            program: program.into(),
            prefix_args,
        }
    }

    pub fn for_current_exe() -> io::Result<Self> {
        Ok(Self::for_executable(std::env::current_exe()?))
    }

    pub fn for_executable(program: impl Into<PathBuf>) -> Self {
        Self::new(program, vec![OsString::from("pod")])
    }

    /// Resolve the Pod runtime command used for subprocess launches.
    ///
    /// The default launch path is always the current `yoi` executable plus
    /// the unified `pod` prefix argument. During development, a non-empty
    /// `YOI_POD_RUNTIME_COMMAND` value replaces only the executable path;
    /// the `pod` prefix is still added here and the env value is not parsed as a
    /// shell command.
    pub fn resolve() -> io::Result<Self> {
        Self::resolve_from_env_value(
            std::env::var_os(POD_RUNTIME_COMMAND_ENV),
            std::env::current_exe,
        )
    }

    fn resolve_from_env_value<F>(
        override_program: Option<OsString>,
        current_exe: F,
    ) -> io::Result<Self>
    where
        F: FnOnce() -> io::Result<PathBuf>,
    {
        if let Some(program) = override_program.filter(|program| !program.as_os_str().is_empty()) {
            return Ok(Self::for_executable(program));
        }

        Ok(Self::for_executable(current_exe()?))
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn prefix_args(&self) -> &[OsString] {
        &self.prefix_args
    }

    pub fn argv_with<I, S>(&self, args: I) -> Vec<OsString>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut argv = self.prefix_args.clone();
        argv.extend(args.into_iter().map(Into::into));
        argv
    }
}

impl fmt::Display for PodRuntimeCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program.display())?;
        for arg in &self.prefix_args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yoi_binary_defaults_to_pod_prefix() {
        let command = PodRuntimeCommand::for_executable("/opt/yoi/bin/yoi");

        assert_eq!(command.program(), Path::new("/opt/yoi/bin/yoi"));
        assert_eq!(command.prefix_args(), [OsString::from("pod")]);
        assert_eq!(
            command.argv_with(["--pod", "agent"]),
            vec!["pod", "--pod", "agent"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn any_runtime_executable_gets_pod_prefix() {
        let command = PodRuntimeCommand::for_executable("/opt/yoi/bin/custom-runtime");

        assert_eq!(command.program(), Path::new("/opt/yoi/bin/custom-runtime"));
        assert_eq!(command.prefix_args(), [OsString::from("pod")]);
        assert_eq!(
            command.argv_with(["--pod", "agent"]),
            vec!["pod", "--pod", "agent"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn resolve_uses_current_exe_when_override_is_unset() {
        let command = PodRuntimeCommand::resolve_from_env_value(None, || {
            Ok(PathBuf::from("/opt/yoi/bin/yoi"))
        })
        .unwrap();

        assert_eq!(
            command,
            PodRuntimeCommand::for_executable("/opt/yoi/bin/yoi")
        );
    }

    #[test]
    fn resolve_uses_current_exe_when_override_is_empty() {
        let command = PodRuntimeCommand::resolve_from_env_value(Some(OsString::new()), || {
            Ok(PathBuf::from("/opt/yoi/bin/yoi"))
        })
        .unwrap();

        assert_eq!(
            command,
            PodRuntimeCommand::for_executable("/opt/yoi/bin/yoi")
        );
    }

    #[test]
    fn resolve_override_replaces_only_program_and_keeps_pod_prefix() {
        let command = PodRuntimeCommand::resolve_from_env_value(
            Some(OsString::from("/tmp/rebuilt yoi")),
            || panic!("override must not inspect current_exe"),
        )
        .unwrap();

        assert_eq!(command.program(), Path::new("/tmp/rebuilt yoi"));
        assert_eq!(command.prefix_args(), [OsString::from("pod")]);
        assert_eq!(
            command.argv_with(["--pod", "agent"]),
            vec!["pod", "--pod", "agent"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }
}
