use std::error::Error;
use std::fmt::{self, Display, Formatter};

use url::Url;

/// The media type emitted for every WIP request and every response with a body.
pub const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// A fixed standard WIP over HTTP v1 route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    /// Observe one object and its indexable descendants.
    Observe,
    /// Fetch an interface descriptor.
    FetchInterface,
    /// Call one operation.
    CallOperation,
}

impl Route {
    /// Returns the RFC 3986 relative reference used below an endpoint prefix.
    #[must_use]
    pub const fn relative_reference(self) -> &'static str {
        match self {
            Self::Observe => "v1/observe",
            Self::FetchInterface => "v1/fetch_interface",
            Self::CallOperation => "v1/call_operation",
        }
    }
}

/// A canonical absolute hierarchical base URL whose path always ends in `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint(Url);

impl Endpoint {
    /// Parses and canonicalizes an endpoint.
    ///
    /// The input must be an absolute hierarchical URL with a host and without a
    /// query or fragment. Its path is an endpoint prefix, not a Worldspace path.
    /// A trailing slash is appended when absent so RFC 3986 relative-reference
    /// resolution preserves the final prefix segment. The supplied scheme is
    /// preserved; scheme and transport security are deployment concerns.
    pub fn parse(input: &str) -> Result<Self, EndpointError> {
        let mut url = Url::parse(input).map_err(EndpointError::InvalidUrl)?;
        if url.cannot_be_a_base() || url.host_str().is_none() {
            return Err(EndpointError::NotAbsoluteBase);
        }
        if url.query().is_some() {
            return Err(EndpointError::HasQuery);
        }
        if url.fragment().is_some() {
            return Err(EndpointError::HasFragment);
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(Self(url))
    }

    /// Returns the canonical base URL.
    #[must_use]
    pub fn as_url(&self) -> &Url {
        &self.0
    }

    /// Resolves one fixed v1 route below this endpoint prefix.
    #[must_use]
    pub fn route(&self, route: Route) -> Url {
        self.0
            .join(route.relative_reference())
            .expect("fixed relative WIP routes are valid URL references")
    }

    /// Recognizes an exact standard route below this endpoint.
    ///
    /// Query strings, fragments, additional path segments, and Worldspace paths
    /// are not route components and therefore never match.
    #[must_use]
    pub fn recognize(&self, candidate: &Url) -> Option<Route> {
        const ROUTES: [Route; 3] = [Route::Observe, Route::FetchInterface, Route::CallOperation];
        if candidate.query().is_some() || candidate.fragment().is_some() {
            return None;
        }
        ROUTES
            .into_iter()
            .find(|route| self.route(*route) == *candidate)
    }
}

impl Display for Endpoint {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl TryFrom<&str> for Endpoint {
    type Error = EndpointError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// Why an endpoint is not a valid standard WIP over HTTP base.
#[derive(Debug)]
pub enum EndpointError {
    /// URL parsing failed.
    InvalidUrl(url::ParseError),
    /// The URL is not a hierarchical absolute URL with an authority.
    NotAbsoluteBase,
    /// A query component was supplied.
    HasQuery,
    /// A fragment component was supplied.
    HasFragment,
}

impl Display for EndpointError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(error) => write!(formatter, "invalid endpoint URL: {error}"),
            Self::NotAbsoluteBase => {
                formatter.write_str("WIP endpoint must be an absolute base URL")
            }
            Self::HasQuery => formatter.write_str("WIP endpoint must not contain a query"),
            Self::HasFragment => formatter.write_str("WIP endpoint must not contain a fragment"),
        }
    }
}

impl Error for EndpointError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidUrl(error) => Some(error),
            _ => None,
        }
    }
}
