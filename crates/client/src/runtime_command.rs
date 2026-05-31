use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

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
    /// The default launch path is always the current `insomnia` executable plus
    /// the unified `pod` prefix argument.
    pub fn resolve() -> io::Result<Self> {
        Self::for_current_exe()
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
}
