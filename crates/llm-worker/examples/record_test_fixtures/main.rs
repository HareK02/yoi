//! Test fixture recording tool
//!
//! Records API responses for defined scenarios.
//!
//! ## Usage
//!
//! ```bash
//! # Show available scenarios
//! cargo run --example record_test_fixtures
//!
//! # Record specific scenario
//! ANTHROPIC_API_KEY=your-key cargo run --example record_test_fixtures -- simple_text
//! ANTHROPIC_API_KEY=your-key cargo run --example record_test_fixtures -- tool_call
//!
//! # Record all scenarios
//! ANTHROPIC_API_KEY=your-key cargo run --example record_test_fixtures -- --all
//! ```

mod recorder;
mod scenarios;

use clap::{Parser, ValueEnum};
use llm_worker::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};
use llm_worker::llm_client::scheme::{
    Scheme, anthropic::AnthropicScheme, gemini::GeminiScheme, openai_chat::OpenAIScheme,
};
use llm_worker::llm_client::transport::{HttpTransport, ResolvedAuth};

/// 既定の capability: fixture 記録には cache_control を付けない
/// （既知モデルの静的テーブルを経由すると scheme 毎に自動設定される）。
fn fallback_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}

fn make_transport<S: Scheme>(
    scheme: S,
    model: &str,
    auth: ResolvedAuth,
) -> HttpTransport<S> {
    let cap = scheme.capability_for(model).unwrap_or_else(fallback_capability);
    let base_url = scheme.default_base_url().to_string();
    HttpTransport::new(scheme, model.to_string(), base_url, auth, cap)
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Scenario name
    #[arg(short, long)]
    scenario: Option<String>,

    /// Run all scenarios
    #[arg(long, default_value_t = false)]
    all: bool,

    /// Client to use
    #[arg(short, long, value_enum, default_value_t = ClientType::Anthropic)]
    client: ClientType,

    /// Model to use (optional, defaults per client)
    #[arg(short, long)]
    model: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum ClientType {
    Anthropic,
    Gemini,
    Openai,
    Ollama,
}

async fn run_scenario_with_anthropic(
    scenario: &scenarios::TestScenario,
    subdir: &str,
    model: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable must be set");
    let model = model.as_deref().unwrap_or("claude-sonnet-4-20250514");
    let client = make_transport(
        AnthropicScheme::new(),
        model,
        ResolvedAuth::ApiKey(api_key),
    );

    recorder::record_request(
        &client,
        scenario.request.clone(),
        scenario.name,
        scenario.output_name,
        subdir,
        model,
    )
    .await?;
    Ok(())
}

async fn run_scenario_with_openai(
    scenario: &scenarios::TestScenario,
    subdir: &str,
    model: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let api_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY environment variable must be set");
    let model = model.as_deref().unwrap_or("gpt-4o");
    let client = make_transport(OpenAIScheme::new(), model, ResolvedAuth::ApiKey(api_key));

    recorder::record_request(
        &client,
        scenario.request.clone(),
        scenario.name,
        scenario.output_name,
        subdir,
        model,
    )
    .await?;
    Ok(())
}

async fn run_scenario_with_ollama(
    scenario: &scenarios::TestScenario,
    subdir: &str,
    model: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Ollama = Anthropic scheme + base_url 差し替え + 認証なし
    let model = model.as_deref().unwrap_or("llama3");
    let client = HttpTransport::new(
        AnthropicScheme::new(),
        model.to_string(),
        "http://localhost:11434".to_string(),
        ResolvedAuth::None,
        fallback_capability(),
    );

    recorder::record_request(
        &client,
        scenario.request.clone(),
        scenario.name,
        scenario.output_name,
        subdir,
        model,
    )
    .await?;
    Ok(())
}

async fn run_scenario_with_gemini(
    scenario: &scenarios::TestScenario,
    subdir: &str,
    model: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let api_key =
        std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY environment variable must be set");
    let model = model.as_deref().unwrap_or("gemini-2.0-flash");
    let client = make_transport(GeminiScheme::new(), model, ResolvedAuth::ApiKey(api_key));

    recorder::record_request(
        &client,
        scenario.request.clone(),
        scenario.name,
        scenario.output_name,
        subdir,
        model,
    )
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();
    let args = Args::parse();

    if !args.all && args.scenario.is_none() {
        use clap::CommandFactory;
        let mut cmd = Args::command();
        cmd.error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "Either --all or --scenario <SCENARIO> must be provided",
        )
        .exit();
    }

    let all_scenarios = scenarios::scenarios();

    // Determine scenarios to run
    let scenarios_to_run: Vec<_> = if args.all {
        all_scenarios
    } else {
        let scenario_name = args.scenario.as_ref().unwrap();
        let found: Vec<_> = all_scenarios
            .into_iter()
            .filter(|s| s.output_name == scenario_name)
            .collect();

        if found.is_empty() {
            eprintln!("Error: Unknown scenario '{}'", scenario_name);
            // Verify correct name by listing
            println!("Available scenarios:");
            for s in scenarios::scenarios() {
                println!("  {}", s.output_name);
            }
            std::process::exit(1);
        }
        found
    };

    println!("=== Test Fixture Generator ===");
    println!("Client: {:?}", args.client);
    if let Some(ref m) = args.model {
        println!("Model: {}", m);
    }
    println!("Scenarios: {}\n", scenarios_to_run.len());

    let subdir = match args.client {
        ClientType::Anthropic => "anthropic",
        ClientType::Gemini => "gemini",
        ClientType::Openai => "openai",
        ClientType::Ollama => "ollama",
    };

    // Scenario filtering is already done in main.rs logic
    // Here we just execute in a simple loop
    for scenario in scenarios_to_run {
        match args.client {
            ClientType::Anthropic => {
                run_scenario_with_anthropic(&scenario, subdir, args.model.clone()).await?
            }
            ClientType::Gemini => {
                run_scenario_with_gemini(&scenario, subdir, args.model.clone()).await?
            }
            ClientType::Openai => {
                run_scenario_with_openai(&scenario, subdir, args.model.clone()).await?
            }
            ClientType::Ollama => {
                run_scenario_with_ollama(&scenario, subdir, args.model.clone()).await?
            }
        }
    }

    println!("\n✅ Done!");
    println!("Run tests with: cargo test -p worker");

    Ok(())
}
