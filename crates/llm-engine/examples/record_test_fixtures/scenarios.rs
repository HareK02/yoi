//! Test fixture request definitions
//!
//! Defines requests and output file names for each scenario

use llm_engine::llm_client::{Request, ToolDefinition};

/// Test scenario
pub struct TestScenario {
    /// Scenario name (description)
    pub name: &'static str,
    /// Output file name (without extension)
    pub output_name: &'static str,
    /// Request
    pub request: Request,
}

/// Get all test scenarios
pub fn scenarios() -> Vec<TestScenario> {
    vec![
        simple_text_scenario(),
        tool_call_scenario(),
        long_text_scenario(),
    ]
}

/// Simple text response
fn simple_text_scenario() -> TestScenario {
    TestScenario {
        name: "Simple text response",
        output_name: "simple_text",
        request: Request::new()
            .system("You are a helpful assistant. Be very concise.")
            .user("Say hello in one word.")
            .max_tokens(50),
    }
}

/// Response with tool call
fn tool_call_scenario() -> TestScenario {
    let get_weather_tool = ToolDefinition::new("get_weather")
        .description("Get the current weather for a city")
        .input_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "The city name"
                }
            },
            "required": ["city"]
        }));

    TestScenario {
        name: "Tool call response",
        output_name: "tool_call",
        request: Request::new()
            .system("You are a helpful assistant. Use tools when appropriate.")
            .user("What's the weather in Tokyo? Use the get_weather tool.")
            .tool(get_weather_tool)
            .max_tokens(200),
    }
}

/// Long text generation scenario
fn long_text_scenario() -> TestScenario {
    TestScenario {
        name: "Long text response",
        output_name: "long_text",
        request: Request::new()
            .system("You are a creative writer.")
            .user("Write a short story about a robot discovering a garden. It should be at least 300 words.")
            .max_tokens(1000),
    }
}
