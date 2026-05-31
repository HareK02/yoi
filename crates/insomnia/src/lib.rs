use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub const POD_COMMAND_OVERRIDE_ENV: &str = "INSOMNIA_POD_COMMAND";

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

    pub fn executable_only(program: impl Into<PathBuf>) -> Self {
        Self::new(program, Vec::new())
    }

    pub fn for_current_exe() -> io::Result<Self> {
        Ok(Self::for_executable(std::env::current_exe()?))
    }

    pub fn for_executable(program: impl Into<PathBuf>) -> Self {
        Self::new(program, vec![OsString::from("pod")])
    }

    /// Resolve the Pod runtime command used for subprocess launches.
    ///
    /// `INSOMNIA_POD_COMMAND` is intentionally executable-only: its value is
    /// used as the program path without shell parsing and without the unified
    /// `pod` prefix arg. That keeps development/test overrides safe while the
    /// default path is always `current_exe() + ["pod"]`.
    pub fn resolve() -> io::Result<Self> {
        if let Some(command) = Self::from_override_env() {
            return Ok(command);
        }
        Self::for_current_exe()
    }

    pub fn from_override_env() -> Option<Self> {
        let raw = std::env::var_os(POD_COMMAND_OVERRIDE_ENV)?;
        if raw.is_empty() {
            return None;
        }
        Some(Self::executable_only(raw))
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore(Option<OsString>);

    impl EnvRestore {
        fn capture() -> Self {
            Self(std::env::var_os(POD_COMMAND_OVERRIDE_ENV))
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(value) => std::env::set_var(POD_COMMAND_OVERRIDE_ENV, value),
                    None => std::env::remove_var(POD_COMMAND_OVERRIDE_ENV),
                }
            }
        }
    }

    #[test]
    fn insomnia_binary_defaults_to_pod_prefix() {
        let command = PodRuntimeCommand::for_executable("/opt/insomnia/bin/insomnia");

        assert_eq!(command.program(), Path::new("/opt/insomnia/bin/insomnia"));
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
        let command = PodRuntimeCommand::for_executable("/opt/insomnia/bin/custom-runtime");

        assert_eq!(
            command.program(),
            Path::new("/opt/insomnia/bin/custom-runtime")
        );
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
    fn env_override_is_executable_only_and_not_shell_parsed() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _restore = EnvRestore::capture();
        unsafe {
            std::env::set_var(POD_COMMAND_OVERRIDE_ENV, "/tmp/mock pod --flag");
        }

        let command = PodRuntimeCommand::resolve().unwrap();

        assert_eq!(command.program(), Path::new("/tmp/mock pod --flag"));
        assert!(command.prefix_args().is_empty());
    }
}
