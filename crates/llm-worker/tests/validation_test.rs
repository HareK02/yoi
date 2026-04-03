use llm_worker::llm_client::providers::openai::OpenAIClient;
use llm_worker::{Worker, WorkerError};

#[test]
fn test_openai_top_k_warning() {
    // Create client with dummy key (validate_config doesn't make network calls, so safe)
    let client = OpenAIClient::new("dummy-key", "gpt-4o");

    // Create Worker with top_k set (OpenAI doesn't support top_k)
    let worker = Worker::new(client).top_k(50);

    // Run validate()
    let result = worker.validate();

    // Verify error is returned and ConfigWarnings is included
    match result {
        Err(WorkerError::ConfigWarnings(warnings)) => {
            assert_eq!(warnings.len(), 1);
            assert_eq!(warnings[0].option_name, "top_k");
            println!("Got expected warning: {}", warnings[0]);
        }
        Ok(_) => panic!("Should have returned validation error"),
        Err(e) => panic!("Unexpected error type: {:?}", e),
    }
}

#[test]
fn test_openai_valid_config() {
    let client = OpenAIClient::new("dummy-key", "gpt-4o");

    // Valid configuration (temperature only)
    let worker = Worker::new(client).temperature(0.7);

    // Run validate()
    let result = worker.validate();

    // Verify success
    assert!(result.is_ok());
}
