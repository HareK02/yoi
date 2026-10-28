//! Host authorization for the concrete network operation, independent of Tool registration.
use std::sync::Arc;

use agen::tool::ToolError;
use async_trait::async_trait;

use crate::worker::WorkspaceClient;

pub(crate) struct WorkspaceWebAuthorizer(pub Arc<dyn WorkspaceClient>);

#[async_trait]
impl tools::WebOperationAuthorizer for WorkspaceWebAuthorizer {
    async fn authorize(&self, operation: &str, url: &str) -> Result<(), ToolError> {
        self.0
            .authorize_network_operation(operation, url)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))
    }
}
