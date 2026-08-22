#![allow(dead_code)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agen::llm_client::event::Event;
use agen::llm_client::{ClientError, LlmClient, Request};
use async_trait::async_trait;
use futures::Stream;

/// A mock LLM client that replays pre-defined event sequences.
#[derive(Clone)]
pub struct MockLlmClient {
    responses: Arc<Vec<Vec<Event>>>,
    call_count: Arc<AtomicUsize>,
}

impl MockLlmClient {
    pub fn new(events: Vec<Event>) -> Self {
        Self::with_responses(vec![events])
    }

    pub fn with_responses(responses: Vec<Vec<Event>>) -> Self {
        Self {
            responses: Arc::new(responses),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.responses.len() {
            return Err(ClientError::Api {
                status: Some(500),
                code: Some("mock_error".to_string()),
                message: "No more mock responses".to_string(),
                retry_after: None,
            });
        }
        let events = self.responses[count].clone();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}
