//! Provider dispatch shared by regular search and native checkout search.
//!
//! Glob/Grep own request defaults, input handling, and formatting; this module checks provider
//! routing and read-only response kinds. Only native search uses `checkout_search`; provider
//! wrappers restrict its request scope and output coordinates. Tools neither create scopes
//! nor reinterpret rendered paths.
use agen::tool::ToolError;
use workdir::{
    CheckoutSearchOperation, CheckoutSearchRequest, CheckoutSearchResult, GlobRequest, GlobResult,
    GrepRequest, GrepResult, WorkdirSessionHandle,
};

use crate::{ToolsError, file_target::checked_error};

pub(crate) struct SearchTarget {
    session: WorkdirSessionHandle,
    checkout: bool,
}

impl SearchTarget {
    pub fn tool(session: WorkdirSessionHandle) -> Self {
        Self {
            session,
            checkout: false,
        }
    }

    pub fn checkout(session: WorkdirSessionHandle) -> Self {
        Self {
            session,
            checkout: true,
        }
    }

    pub async fn glob(&self, request: GlobRequest) -> Result<GlobResult, ToolError> {
        if self.checkout {
            let result = self
                .session
                .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::Glob(
                    request,
                )))
                .await
                .map_err(checked_error)?;
            glob_result(result)
        } else {
            self.session
                .glob(request)
                .await
                .map_err(|error| ToolsError::from(error).into())
        }
    }

    pub async fn grep(&self, request: GrepRequest) -> Result<GrepResult, ToolError> {
        if self.checkout {
            let result = self
                .session
                .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::Grep(
                    request,
                )))
                .await
                .map_err(checked_error)?;
            grep_result(result)
        } else {
            self.session
                .grep(request)
                .await
                .map_err(|error| ToolsError::from(error).into())
        }
    }
}

pub(crate) fn glob_result(result: CheckoutSearchResult) -> Result<GlobResult, ToolError> {
    match result {
        CheckoutSearchResult::Glob(result) => Ok(result),
        _ => Err(crate::file_target::readonly_result_error("Glob")),
    }
}

pub(crate) fn grep_result(result: CheckoutSearchResult) -> Result<GrepResult, ToolError> {
    match result {
        CheckoutSearchResult::Grep(result) => Ok(result),
        _ => Err(crate::file_target::readonly_result_error("Grep")),
    }
}
