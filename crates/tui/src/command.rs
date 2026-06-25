use protocol::Method;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandInputMode {
    Composer,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandArgs {
    raw: String,
    argv: Vec<String>,
}

impl CommandArgs {
    pub fn parse_whitespace(raw: &str) -> Self {
        Self {
            raw: raw.to_owned(),
            argv: raw.split_whitespace().map(str::to_owned).collect(),
        }
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandDiagnostic {
    pub message: String,
}

impl CommandDiagnostic {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvironment {
    pub connected: bool,
    pub running: bool,
    pub paused: bool,
}

#[derive(Debug, Clone)]
pub struct CommandExecution {
    pub method: Option<Method>,
    pub diagnostics: Vec<CommandDiagnostic>,
    pub exit_command_mode: bool,
    pub clear_input: bool,
}

impl CommandExecution {
    pub fn diagnostic(message: impl Into<String>) -> Self {
        Self {
            method: None,
            diagnostics: vec![CommandDiagnostic::new(message)],
            exit_command_mode: false,
            clear_input: false,
        }
    }

    pub fn notice(message: impl Into<String>) -> Self {
        Self {
            method: None,
            diagnostics: vec![CommandDiagnostic::new(message)],
            exit_command_mode: true,
            clear_input: true,
        }
    }
}

pub type ArgumentParser = fn(&str) -> Result<CommandArgs, CommandDiagnostic>;
pub type AvailabilityCheck = fn(&CommandEnvironment) -> Result<(), CommandDiagnostic>;
pub type CommandExecutor = fn(CommandInvocation<'_>) -> CommandExecution;

#[derive(Clone)]
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description: &'static str,
    pub argument_parser: ArgumentParser,
    pub can_execute: AvailabilityCheck,
    pub executor: CommandExecutor,
}

pub struct CommandInvocation<'a> {
    pub registry: &'a CommandRegistry,
    pub command: &'a CommandSpec,
    pub args: CommandArgs,
    pub environment: &'a CommandEnvironment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCandidate {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
}

#[derive(Clone)]
pub struct CommandRegistry {
    commands: Vec<CommandSpec>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn builtins() -> Self {
        let mut registry = Self::new();
        registry.register(CommandSpec {
            name: "help",
            aliases: &["?"],
            usage: "help [command]",
            description: "Show available TUI commands or details for one command.",
            argument_parser: help_args,
            can_execute: always_available,
            executor: help_command,
        });
        registry.register(CommandSpec {
            name: "noop",
            aliases: &["nop"],
            usage: "noop",
            description: "Validate command dispatch without side effects.",
            argument_parser: no_args,
            can_execute: always_available,
            executor: noop_command,
        });
        registry.register(CommandSpec {
            name: "compact",
            aliases: &[],
            usage: "compact",
            description: "Request immediate Worker context compaction.",
            argument_parser: compact_args,
            can_execute: compact_available,
            executor: compact_command,
        });
        registry.register(CommandSpec {
            name: "rewind",
            aliases: &["rollback"],
            usage: "rewind",
            description: "Open the rewind target picker.",
            argument_parser: rewind_args,
            can_execute: rewind_available,
            executor: rewind_command,
        });
        registry.register(CommandSpec {
            name: "peer",
            aliases: &[],
            usage: "peer <worker-name>",
            description: "Register another existing Worker as a reciprocal metadata peer.",
            argument_parser: peer_args,
            can_execute: peer_available,
            executor: peer_command,
        });
        registry
    }

    pub fn register(&mut self, spec: CommandSpec) {
        debug_assert!(!self.commands.iter().any(|c| c.name == spec.name));
        self.commands.push(spec);
    }

    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    pub fn find(&self, name_or_alias: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|command| {
            command.name == name_or_alias
                || command.aliases.iter().any(|alias| *alias == name_or_alias)
        })
    }

    pub fn suggest(&self, command_line: &str) -> Vec<CommandCandidate> {
        let prefix = command_prefix(command_line);
        if prefix.is_empty() {
            return self.commands.iter().map(CommandCandidate::from).collect();
        }
        self.commands
            .iter()
            .filter(|command| {
                command.name.starts_with(prefix)
                    || command
                        .aliases
                        .iter()
                        .any(|alias| alias.starts_with(prefix))
            })
            .map(CommandCandidate::from)
            .collect()
    }

    pub fn dispatch(
        &self,
        command_line: &str,
        environment: &CommandEnvironment,
    ) -> CommandExecution {
        let trimmed = command_line.trim();
        if trimmed.is_empty() {
            return CommandExecution::diagnostic(
                "Empty command. Type :help for available commands.",
            );
        }
        let (name, raw_args) = split_command(trimmed);
        let Some(command) = self.find(name) else {
            return CommandExecution::diagnostic(format!(
                "Unknown command: {name}. Type :help for available commands."
            ));
        };
        let args = match (command.argument_parser)(raw_args) {
            Ok(args) => args,
            Err(err) => return CommandExecution::diagnostic(err.message),
        };
        if let Err(err) = (command.can_execute)(environment) {
            return CommandExecution::diagnostic(err.message);
        }
        (command.executor)(CommandInvocation {
            registry: self,
            command,
            args,
            environment,
        })
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::builtins()
    }
}

impl From<&CommandSpec> for CommandCandidate {
    fn from(command: &CommandSpec) -> Self {
        Self {
            name: command.name,
            usage: command.usage,
            description: command.description,
        }
    }
}

fn split_command(trimmed: &str) -> (&str, &str) {
    match trimmed.find(char::is_whitespace) {
        Some(idx) => {
            let (name, rest) = trimmed.split_at(idx);
            (name, rest.trim_start())
        }
        None => (trimmed, ""),
    }
}

fn command_prefix(command_line: &str) -> &str {
    let trimmed = command_line.trim_start();
    match trimmed.find(char::is_whitespace) {
        Some(idx) => &trimmed[..idx],
        None => trimmed,
    }
}

fn always_available(_environment: &CommandEnvironment) -> Result<(), CommandDiagnostic> {
    Ok(())
}

fn no_args(raw: &str) -> Result<CommandArgs, CommandDiagnostic> {
    let args = CommandArgs::parse_whitespace(raw);
    if args.argv().is_empty() {
        Ok(args)
    } else {
        Err(CommandDiagnostic::new("Invalid arguments. Usage: noop"))
    }
}

fn help_args(raw: &str) -> Result<CommandArgs, CommandDiagnostic> {
    let args = CommandArgs::parse_whitespace(raw);
    if args.argv().len() <= 1 {
        Ok(args)
    } else {
        Err(CommandDiagnostic::new(
            "Invalid arguments. Usage: help [command]",
        ))
    }
}

fn compact_args(raw: &str) -> Result<CommandArgs, CommandDiagnostic> {
    let args = CommandArgs::parse_whitespace(raw);
    if args.argv().is_empty() {
        Ok(args)
    } else {
        Err(CommandDiagnostic::new("Invalid arguments. Usage: compact"))
    }
}

fn rewind_args(raw: &str) -> Result<CommandArgs, CommandDiagnostic> {
    let args = CommandArgs::parse_whitespace(raw);
    if args.argv().is_empty() {
        Ok(args)
    } else {
        Err(CommandDiagnostic::new("Invalid arguments. Usage: rewind"))
    }
}

fn peer_args(raw: &str) -> Result<CommandArgs, CommandDiagnostic> {
    let args = CommandArgs::parse_whitespace(raw);
    if args.argv().len() == 1 {
        Ok(args)
    } else {
        Err(CommandDiagnostic::new(
            "Invalid arguments. Usage: peer <worker-name>",
        ))
    }
}

fn compact_available(environment: &CommandEnvironment) -> Result<(), CommandDiagnostic> {
    if !environment.connected {
        return Err(CommandDiagnostic::new(
            "Cannot compact: not connected to a Worker.",
        ));
    }
    if environment.running {
        return Err(CommandDiagnostic::new(
            "Cannot compact while the Worker is running.",
        ));
    }
    if environment.paused {
        return Err(CommandDiagnostic::new(
            "Cannot compact while the Worker is paused; resume or start a fresh turn first.",
        ));
    }
    Ok(())
}

fn rewind_available(environment: &CommandEnvironment) -> Result<(), CommandDiagnostic> {
    if !environment.connected {
        return Err(CommandDiagnostic::new(
            "Cannot rewind before the Worker is connected.",
        ));
    }
    if environment.running {
        return Err(CommandDiagnostic::new(
            "Cannot rewind while the Worker is running.",
        ));
    }
    Ok(())
}

fn peer_available(environment: &CommandEnvironment) -> Result<(), CommandDiagnostic> {
    if !environment.connected {
        return Err(CommandDiagnostic::new(
            "Cannot register a peer before the Worker is connected.",
        ));
    }
    if environment.running {
        return Err(CommandDiagnostic::new(
            "Cannot register a peer while the Worker is running.",
        ));
    }
    Ok(())
}

fn help_command(invocation: CommandInvocation<'_>) -> CommandExecution {
    if let Some(name) = invocation.args.argv().first() {
        let Some(command) = invocation.registry.find(name) else {
            return CommandExecution::diagnostic(format!(
                "Unknown command: {name}. Type :help for available commands."
            ));
        };
        let aliases = if command.aliases.is_empty() {
            "".to_owned()
        } else {
            format!(" aliases: {}.", command.aliases.join(", "))
        };
        return CommandExecution::notice(format!(
            "command: {} — usage: {}.{} {}",
            command.name, command.usage, aliases, command.description
        ));
    }

    let list = invocation
        .registry
        .commands()
        .iter()
        .map(|command| format!("{} ({})", command.name, command.usage))
        .collect::<Vec<_>>()
        .join(", ");
    CommandExecution::notice(format!("available commands: {list}"))
}

fn noop_command(invocation: CommandInvocation<'_>) -> CommandExecution {
    let _ = invocation.command;
    let _ = invocation.environment;
    let _ = invocation.args.raw();
    CommandExecution::notice("noop: no action")
}

fn compact_command(invocation: CommandInvocation<'_>) -> CommandExecution {
    let _ = invocation.command;
    let _ = invocation.environment;
    let _ = invocation.args.raw();
    CommandExecution {
        method: Some(Method::Compact),
        diagnostics: vec![CommandDiagnostic::new("compact requested")],
        exit_command_mode: true,
        clear_input: true,
    }
}

fn rewind_command(invocation: CommandInvocation<'_>) -> CommandExecution {
    let _ = invocation.command;
    let _ = invocation.environment;
    let _ = invocation.args.raw();
    CommandExecution {
        method: Some(Method::ListRewindTargets),
        diagnostics: vec![CommandDiagnostic::new("rewind picker requested")],
        exit_command_mode: true,
        clear_input: true,
    }
}

fn peer_command(invocation: CommandInvocation<'_>) -> CommandExecution {
    let _ = invocation.command;
    let _ = invocation.environment;
    let name = invocation.args.argv()[0].clone();
    CommandExecution {
        method: Some(Method::RegisterPeer { name: name.clone() }),
        diagnostics: vec![CommandDiagnostic::new(format!(
            "peer metadata registration requested with `{name}`"
        ))],
        exit_command_mode: true,
        clear_input: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> CommandEnvironment {
        CommandEnvironment {
            connected: true,
            running: false,
            paused: false,
        }
    }

    #[test]
    fn builtins_suggest_by_prefix() {
        let registry = CommandRegistry::builtins();
        assert_eq!(registry.suggest("he")[0].name, "help");
        assert_eq!(registry.suggest("n")[0].name, "noop");
    }

    #[test]
    fn unknown_command_is_local_diagnostic() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("wat", &env());
        assert!(result.method.is_none());
        assert!(!result.exit_command_mode);
        assert!(result.diagnostics[0].message.contains("Unknown command"));
    }

    #[test]
    fn invalid_arguments_are_local_diagnostic() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("noop extra", &env());
        assert!(result.method.is_none());
        assert!(!result.exit_command_mode);
        assert!(result.diagnostics[0].message.contains("Invalid arguments"));
    }

    #[test]
    fn compact_command_returns_compact_method_not_run() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("compact", &env());
        assert!(matches!(result.method, Some(Method::Compact)));
        assert!(result.exit_command_mode);
        assert!(result.clear_input);
        assert!(result.diagnostics[0].message.contains("compact requested"));
    }

    #[test]
    fn compact_invalid_arguments_are_local_diagnostic() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("compact now", &env());
        assert!(result.method.is_none());
        assert!(!result.exit_command_mode);
        assert!(result.diagnostics[0].message.contains("Invalid arguments"));
    }

    #[test]
    fn compact_rejects_running_and_paused_locally() {
        let registry = CommandRegistry::builtins();
        let mut running = env();
        running.running = true;
        let result = registry.dispatch("compact", &running);
        assert!(result.method.is_none());
        assert!(result.diagnostics[0].message.contains("running"));

        let mut paused = env();
        paused.paused = true;
        let result = registry.dispatch("compact", &paused);
        assert!(result.method.is_none());
        assert!(result.diagnostics[0].message.contains("paused"));
    }

    #[test]
    fn rewind_command_and_alias_return_list_method() {
        let registry = CommandRegistry::builtins();
        for command in ["rewind", "rollback"] {
            let result = registry.dispatch(command, &env());
            assert!(matches!(result.method, Some(Method::ListRewindTargets)));
            assert!(result.exit_command_mode);
            assert!(result.clear_input);
            assert!(result.diagnostics[0].message.contains("rewind picker"));
        }
    }

    #[test]
    fn rewind_invalid_arguments_are_local_diagnostic() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("rewind now", &env());
        assert!(result.method.is_none());
        assert!(!result.exit_command_mode);
        assert!(result.diagnostics[0].message.contains("Invalid arguments"));
    }

    #[test]
    fn rewind_rejects_running_but_allows_paused() {
        let registry = CommandRegistry::builtins();
        let mut running = env();
        running.running = true;
        let result = registry.dispatch("rewind", &running);
        assert!(result.method.is_none());
        assert!(result.diagnostics[0].message.contains("running"));

        let mut paused = env();
        paused.paused = true;
        let result = registry.dispatch("rewind", &paused);
        assert!(matches!(result.method, Some(Method::ListRewindTargets)));
    }

    #[test]
    fn peer_command_returns_register_peer_method() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("peer reviewer", &env());
        assert!(matches!(
            result.method,
            Some(Method::RegisterPeer { ref name }) if name == "reviewer"
        ));
        assert!(result.exit_command_mode);
        assert!(result.clear_input);
        assert!(
            result.diagnostics[0]
                .message
                .contains("metadata registration")
        );
    }

    #[test]
    fn peer_invalid_arguments_are_local_diagnostic() {
        let registry = CommandRegistry::builtins();
        for command in ["peer", "peer one two"] {
            let result = registry.dispatch(command, &env());
            assert!(result.method.is_none());
            assert!(!result.exit_command_mode);
            assert!(result.diagnostics[0].message.contains("Invalid arguments"));
        }
    }

    #[test]
    fn ticket_command_is_unknown() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("ticket intake add role shortcuts", &env());
        assert!(result.method.is_none());
        assert!(!result.exit_command_mode);
        assert!(
            result.diagnostics[0]
                .message
                .contains("Unknown command: ticket")
        );
    }

    #[test]
    fn peer_help_mentions_metadata_registration() {
        let registry = CommandRegistry::builtins();
        let result = registry.dispatch("help peer", &env());
        assert!(result.method.is_none());
        assert!(result.diagnostics[0].message.contains("peer <worker-name>"));
        assert!(result.diagnostics[0].message.contains("metadata peer"));
    }

    #[test]
    fn peer_rejects_disconnected() {
        let registry = CommandRegistry::builtins();
        let mut disconnected = env();
        disconnected.connected = false;
        let result = registry.dispatch("peer reviewer", &disconnected);
        assert!(result.method.is_none());
        assert!(result.diagnostics[0].message.contains("connected"));
    }

    #[test]
    fn peer_rejects_running() {
        let registry = CommandRegistry::builtins();
        let mut running = env();
        running.running = true;
        let result = registry.dispatch("peer reviewer", &running);
        assert!(result.method.is_none());
        assert!(result.diagnostics[0].message.contains("running"));
    }
}
