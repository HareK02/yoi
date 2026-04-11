//! Interactive CLI client using Worker
//!
//! A CLI application for interacting with multiple LLM providers (Anthropic, Gemini, OpenAI, Ollama).
//! Demonstrates tool registration and execution, and streaming response display.
//!
//! ## Usage
//!
//! ```bash
//! # Set API keys in .env file
//! echo "ANTHROPIC_API_KEY=your-api-key" > .env
//! echo "GEMINI_API_KEY=your-api-key" >> .env
//! echo "OPENAI_API_KEY=your-api-key" >> .env
//!
//! # Anthropic (default)
//! cargo run --example worker_cli
//!
//! # Gemini
//! cargo run --example worker_cli -- --provider gemini
//!
//! # OpenAI
//! cargo run --example worker_cli -- --provider openai --model gpt-4o
//!
//! # Ollama (local)
//! cargo run --example worker_cli -- --provider ollama --model llama3.2
//!
//! # With options
//! cargo run --example worker_cli -- --provider anthropic --model claude-3-haiku-20240307 --system "You are a helpful assistant."
//!
//! # Show help
//! cargo run --example worker_cli -- --help
//! ```

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::info;
use tracing_subscriber::EnvFilter;

use clap::{Parser, ValueEnum};
use llm_worker::{
    Worker,
    llm_client::{
        LlmClient,
        providers::{
            anthropic::AnthropicClient, gemini::GeminiClient, ollama::OllamaClient,
            openai::OpenAIClient,
        },
    },
    interceptor::{Interceptor, PostToolAction, ToolResultInfo},
    timeline::{Handler, TextBlockEvent, TextBlockKind, ToolUseBlockEvent, ToolUseBlockKind},
};
use llm_worker_macros::tool_registry;

// Required imports for macro expansion
use schemars;
use serde;

// =============================================================================
// Provider Definition
// =============================================================================

/// Available LLM providers
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum Provider {
    /// Anthropic Claude
    #[default]
    Anthropic,
    /// Google Gemini
    Gemini,
    /// OpenAI GPT
    Openai,
    /// Ollama (local)
    Ollama,
}

impl Provider {
    /// Default model for the provider
    fn default_model(&self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-sonnet-4-20250514",
            Provider::Gemini => "gemini-2.0-flash",
            Provider::Openai => "gpt-4o",
            Provider::Ollama => "llama3.2",
        }
    }

    /// Display name for the provider
    fn display_name(&self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic Claude",
            Provider::Gemini => "Google Gemini",
            Provider::Openai => "OpenAI GPT",
            Provider::Ollama => "Ollama (Local)",
        }
    }

    /// Environment variable name for API key
    fn env_var_name(&self) -> Option<&'static str> {
        match self {
            Provider::Anthropic => Some("ANTHROPIC_API_KEY"),
            Provider::Gemini => Some("GEMINI_API_KEY"),
            Provider::Openai => Some("OPENAI_API_KEY"),
            Provider::Ollama => None, // Ollama is local, no key needed
        }
    }
}

// =============================================================================
// CLI Argument Definition
// =============================================================================

/// Interactive CLI client supporting multiple LLM providers
#[derive(Parser, Debug)]
#[command(name = "worker-cli")]
#[command(about = "Interactive CLI client for multiple LLM providers using Worker")]
#[command(version)]
struct Args {
    /// Provider to use
    #[arg(long, value_enum, default_value_t = Provider::Anthropic)]
    provider: Provider,

    /// Model name to use (defaults to provider's default if not specified)
    #[arg(short, long)]
    model: Option<String>,

    /// System prompt
    #[arg(short, long)]
    system: Option<String>,

    /// Disable tools
    #[arg(long, default_value = "false")]
    no_tools: bool,

    /// Initial message (if specified, sends it and exits)
    #[arg(short = 'p', long)]
    prompt: Option<String>,

    /// API key (takes precedence over environment variable)
    #[arg(long)]
    api_key: Option<String>,
}

// =============================================================================
// Tool Definition
// =============================================================================

/// Application context
#[derive(Clone)]
struct AppContext;

#[tool_registry]
impl AppContext {
    /// Get the current date and time
    ///
    /// Returns the system's current date and time.
    #[tool]
    fn get_current_time(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Simple conversion from Unix timestamp
        format!("Current Unix timestamp: {}", now)
    }

    /// Perform a simple calculation
    ///
    /// Executes arithmetic operations on two numbers.
    #[tool]
    fn calculate(&self, a: f64, b: f64, operation: String) -> Result<String, String> {
        let result = match operation.as_str() {
            "add" | "+" => a + b,
            "subtract" | "-" => a - b,
            "multiply" | "*" => a * b,
            "divide" | "/" => {
                if b == 0.0 {
                    return Err("Cannot divide by zero".to_string());
                }
                a / b
            }
            _ => return Err(format!("Unknown operation: {}", operation)),
        };
        Ok(format!("{} {} {} = {}", a, operation, b, result))
    }
}

// =============================================================================
// Streaming Display Handlers
// =============================================================================

/// Handler that outputs text in real-time
struct StreamingPrinter {
    is_first_delta: Arc<Mutex<bool>>,
}

impl StreamingPrinter {
    fn new() -> Self {
        Self {
            is_first_delta: Arc::new(Mutex::new(true)),
        }
    }
}

impl Handler<TextBlockKind> for StreamingPrinter {
    type Scope = ();

    fn on_event(&mut self, _scope: &mut (), event: &TextBlockEvent) {
        match event {
            TextBlockEvent::Start(_) => {
                let mut first = self.is_first_delta.lock().unwrap();
                if *first {
                    print!("\n🤖 ");
                    *first = false;
                }
            }
            TextBlockEvent::Delta(text) => {
                print!("{}", text);
                io::stdout().flush().ok();
            }
            TextBlockEvent::Stop(_) => {
                println!();
            }
        }
    }
}

/// Handler that displays tool calls
struct ToolCallPrinter {
    call_names: Arc<Mutex<HashMap<String, String>>>,
}

impl ToolCallPrinter {
    fn new(call_names: Arc<Mutex<HashMap<String, String>>>) -> Self {
        Self { call_names }
    }
}

#[derive(Default)]
struct ToolCallPrinterScope {
    input_json: String,
}

impl Handler<ToolUseBlockKind> for ToolCallPrinter {
    type Scope = ToolCallPrinterScope;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &ToolUseBlockEvent) {
        match event {
            ToolUseBlockEvent::Start(start) => {
                scope.input_json.clear();
                self.call_names
                    .lock()
                    .unwrap()
                    .insert(start.id.clone(), start.name.clone());
                println!("\n🔧 Calling tool: {}", start.name);
            }
            ToolUseBlockEvent::InputJsonDelta(json) => {
                scope.input_json.push_str(json);
            }
            ToolUseBlockEvent::Stop(_) => {
                if scope.input_json.is_empty() {
                    println!("   Args: {{}}");
                } else {
                    println!("   Args: {}", scope.input_json);
                }
                scope.input_json.clear();
            }
        }
    }
}

/// Policy that displays tool execution results.
struct ToolResultPrinterPolicy {
    call_names: Arc<Mutex<HashMap<String, String>>>,
}

impl ToolResultPrinterPolicy {
    fn new(call_names: Arc<Mutex<HashMap<String, String>>>) -> Self {
        Self { call_names }
    }
}

#[async_trait]
impl Interceptor for ToolResultPrinterPolicy {
    async fn post_tool_call(&self, info: &mut ToolResultInfo) -> PostToolAction {
        let name = self
            .call_names
            .lock()
            .unwrap()
            .remove(&info.result.tool_use_id)
            .unwrap_or_else(|| info.result.tool_use_id.clone());

        if info.result.is_error {
            println!("   Result ({}): ❌ {}", name, info.result.content);
        } else {
            println!("   Result ({}): ✅ {}", name, info.result.content);
        }

        PostToolAction::Continue
    }
}

// =============================================================================
// Client Creation
// =============================================================================

/// Get API key based on provider
fn get_api_key(args: &Args) -> Result<String, String> {
    // CLI argument API key takes precedence
    if let Some(ref key) = args.api_key {
        return Ok(key.clone());
    }

    // Check environment variable based on provider
    if let Some(env_var) = args.provider.env_var_name() {
        std::env::var(env_var).map_err(|_| {
            format!(
                "API key required. Set {} environment variable or use --api-key",
                env_var
            )
        })
    } else {
        // Ollama etc. don't need a key
        Ok(String::new())
    }
}

/// Create client based on provider
fn create_client(args: &Args) -> Result<Box<dyn LlmClient>, String> {
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| args.provider.default_model().to_string());

    let api_key = get_api_key(args)?;

    match args.provider {
        Provider::Anthropic => {
            let client = AnthropicClient::new(&api_key, &model);
            Ok(Box::new(client))
        }
        Provider::Gemini => {
            let client = GeminiClient::new(&api_key, &model);
            Ok(Box::new(client))
        }
        Provider::Openai => {
            let client = OpenAIClient::new(&api_key, &model);
            Ok(Box::new(client))
        }
        Provider::Ollama => {
            let client = OllamaClient::new(&model);
            Ok(Box::new(client))
        }
    }
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file
    dotenv::dotenv().ok();

    // Initialize logging
    // Use RUST_LOG=debug cargo run --example worker_cli ... for detailed logs
    // Default is warn level, can be overridden with RUST_LOG environment variable
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    // Parse CLI arguments
    let args = Args::parse();

    info!(
        provider = ?args.provider,
        model = ?args.model,
        "Starting worker CLI"
    );

    // Interactive mode or one-shot mode
    let is_interactive = args.prompt.is_none();

    // Model name (for display)
    let model_name = args
        .model
        .clone()
        .unwrap_or_else(|| args.provider.default_model().to_string());

    if is_interactive {
        let title = format!("Worker CLI - {}", args.provider.display_name());
        let border_len = title.len() + 6;
        println!("╔{}╗", "═".repeat(border_len));
        println!("║   {}   ║", title);
        println!("╚{}╝", "═".repeat(border_len));
        println!();
        println!("Provider: {}", args.provider.display_name());
        println!("Model: {}", model_name);
        if let Some(ref system) = args.system {
            println!("System: {}", system);
        }
        if args.no_tools {
            println!("Tools: disabled");
        } else {
            println!("Tools:");
            println!("  • get_current_time - Get the current timestamp");
            println!("  • calculate - Perform arithmetic (add, subtract, multiply, divide)");
        }
        println!();
        println!("Type 'quit' or 'exit' to end the session.");
        println!("─────────────────────────────────────────────────");
    }

    // Create client
    let client = match create_client(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            std::process::exit(1);
        }
    };

    // Create Worker
    let mut worker = Worker::new(client);

    let tool_call_names = Arc::new(Mutex::new(HashMap::new()));

    // Set system prompt
    if let Some(ref system_prompt) = args.system {
        worker.set_system_prompt(system_prompt);
    }

    // Register tools (unless --no-tools)
    if !args.no_tools {
        let app = AppContext;
        worker.register_tool(app.get_current_time_definition());
        worker.register_tool(app.calculate_definition());
    }

    // Register streaming display handlers
    worker
        .timeline_mut()
        .on_text_block(StreamingPrinter::new())
        .on_tool_use_block(ToolCallPrinter::new(tool_call_names.clone()));

    worker.set_interceptor(ToolResultPrinterPolicy::new(tool_call_names));

    // One-shot mode
    if let Some(prompt) = args.prompt {
        match worker.run(&prompt).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("\n❌ Error: {}", e);
                std::process::exit(1);
            }
        }

        return Ok(());
    }

    // Interactive loop — first input transitions Mutable → Locked
    print!("\n👤 You: ");
    io::stdout().flush()?;

    let mut first_input = String::new();
    io::stdin().read_line(&mut first_input)?;
    let first_input = first_input.trim();

    if first_input == "quit" || first_input == "exit" || first_input.is_empty() {
        println!("\n👋 Goodbye!");
        return Ok(());
    }

    let (mut locked, _) = match worker.run(first_input).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("\n❌ Error: {}", e);
            return Ok(());
        }
    };

    loop {
        print!("\n👤 You: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        if input == "quit" || input == "exit" {
            println!("\n👋 Goodbye!");
            break;
        }

        match locked.run(input).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("\n❌ Error: {}", e);
            }
        }
    }

    Ok(())
}
