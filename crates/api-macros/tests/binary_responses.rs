#![cfg(all(feature = "reqwest", feature = "axum"))]

use api_macros::{
    BinaryBody, Operation, api,
    axum::framework::{Body, Response, Router, get},
    reqwest::{ClientError, ClientFailure},
};
#[cfg(feature = "openapi")]
use api_macros::{
    WireKind,
    openapi::{OpenApiInfo, OpenApiSchema},
};
#[cfg(feature = "openapi")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[cfg(feature = "openapi")]
use serde_json::json;

const BYTES: &[u8] = b"\0\xff\x80\r\n{\"not\":\"a JSON response\"}";
const ETAG: &str = "\"binary-v1\"";
const DISPOSITION: &str = "attachment; filename=fixture.bin";

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(JsonSchema))]
pub struct PublicError {
    message: String,
}
#[cfg(feature = "openapi")]
impl OpenApiSchema for PublicError {}
impl api_macros::HttpError for PublicError {
    fn status_code(&self) -> u16 {
        404
    }
}

#[cfg_attr(feature = "openapi", api(reqwest, axum, openapi))]
#[cfg_attr(not(feature = "openapi"), api(reqwest, axum))]
pub trait DownloadApi {
    #[get("/plain/{mode}", error_status = 404)]
    async fn plain(&self, mode: String) -> Result<api_macros::BinaryBody, PublicError>;

    #[get(
        "/download/{mode}",
        error_status = 404,
        responses = [
            (status = 200, body = BinaryBody, headers = [
                ("x-byte-length", u32), ("content-disposition", String), ("etag", String)
            ]),
            (status = 304, headers = [("etag", String)])
        ]
    )]
    async fn download(&self, mode: String)
    -> Result<download_api_responses::Download, PublicError>;
}

struct DownloadService;
impl DownloadApi for DownloadService {
    async fn plain(&self, mode: String) -> Result<BinaryBody, PublicError> {
        if mode == "missing" {
            return Err(PublicError {
                message: "not found".to_owned(),
            });
        }
        Ok(if mode == "empty" {
            b"".as_slice()
        } else {
            BYTES
        }
        .into())
    }

    async fn download(
        &self,
        mode: String,
    ) -> Result<download_api_responses::Download, PublicError> {
        if mode == "cached" {
            return Ok(download_api_responses::Download::Status304 {
                header_etag: ETAG.to_owned(),
            });
        }
        let body = self.plain(mode).await?;
        Ok(download_api_responses::Download::Status200 {
            header_x_byte_length: body.len() as u32,
            header_content_disposition: DISPOSITION.to_owned(),
            header_etag: ETAG.to_owned(),
            body,
        })
    }
}

struct Server {
    base_url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            api_macros::axum::framework::serve(listener, router)
                .await
                .unwrap();
        });
        Self { base_url, task }
    }

    fn client(&self, limit: usize) -> DownloadApiClient {
        DownloadApiClient::builder(&self.base_url)
            .unwrap()
            .response_body_limit(limit)
            .build()
            .unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn binary_success_preserves_exact_bytes_content_type_and_declared_headers() {
    let server = Server::start(DownloadApiAxum::router(DownloadService)).await;
    let client = server.client(BYTES.len());
    for (mode, expected) in [("full", BYTES), ("empty", b"".as_slice())] {
        let response = reqwest::get(format!("{}plain/{mode}", server.base_url))
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            response.headers()["content-type"],
            "application/octet-stream"
        );
        assert_eq!(response.bytes().await.unwrap().as_ref(), expected);
        assert_eq!(
            client.plain(mode.to_owned()).await.unwrap().as_ref(),
            expected
        );

        let response = client.download(mode.to_owned()).await.unwrap();
        match response {
            download_api_responses::Download::Status200 {
                body,
                header_x_byte_length,
                header_content_disposition,
                header_etag,
            } => {
                assert_eq!(body.as_ref(), expected);
                assert_eq!(header_x_byte_length as usize, expected.len());
                assert_eq!(header_content_disposition, DISPOSITION);
                assert_eq!(header_etag, ETAG);
            }
            other => panic!("expected binary success, received {other:?}"),
        }
    }
    assert!(
        matches!(client.download("cached".to_owned()).await.unwrap(),
        download_api_responses::Download::Status304 { header_etag } if header_etag == ETAG)
    );
}

#[cfg(feature = "openapi")]
#[test]
fn binary_success_metadata_and_openapi_use_inline_binary_schema_with_headers() {
    let document = download_api_openapi(OpenApiInfo {
        title: "Download fixture",
        version: "1",
        source_digest: "fixture",
    })
    .unwrap();
    let value = document.as_value();
    for (path, metadata) in [
        ("/plain/{mode}", download_api_operations::Plain::METADATA),
        (
            "/download/{mode}",
            download_api_operations::Download::METADATA,
        ),
    ] {
        assert_eq!(
            metadata.success_responses[0].body.wire_kind,
            WireKind::Binary
        );
        assert_eq!(metadata.error_responses[0].body.wire_kind, WireKind::Json);
        let responses = &value["paths"][path]["get"]["responses"];
        assert_eq!(
            responses["200"]["content"],
            json!({
                "application/octet-stream": { "schema": { "type": "string", "format": "binary" } }
            })
        );
        assert_eq!(
            responses["404"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/PublicError"
        );
    }
    let metadata = download_api_operations::Download::METADATA;
    assert_eq!(
        metadata.success_responses[1].body.wire_kind,
        WireKind::Empty
    );
    let responses = &value["paths"]["/download/{mode}"]["get"]["responses"];
    for header in metadata.success_responses[0].headers {
        assert!(responses["200"]["headers"][header.wire_name]["schema"].is_object());
    }
    assert!(responses["304"].get("content").is_none());
    assert!(responses["304"]["headers"]["etag"]["schema"].is_object());
    assert!(
        value["components"]["schemas"]
            .as_object()
            .unwrap()
            .keys()
            .all(|name| !name.contains("BinaryBody"))
    );
}

#[tokio::test]
async fn binary_success_keeps_json_public_errors_and_rejects_unexpected_statuses() {
    let server = Server::start(DownloadApiAxum::router(DownloadService)).await;
    let client = server.client(1024);
    for result in [
        client.plain("missing".to_owned()).await.map(|_| ()),
        client.download("missing".to_owned()).await.map(|_| ()),
    ] {
        match result.unwrap_err() {
            ClientError::Public { status, error } => {
                assert_eq!(status.as_u16(), 404);
                assert_eq!(error.message, "not found");
            }
            other => panic!("expected JSON public error, received {other:?}"),
        }
    }
    for (status, expected) in [
        (
            201,
            ClientFailure::UnexpectedStatus {
                expected: 200,
                actual: 201,
            },
        ),
        (404, ClientFailure::ErrorResponseDecode { status: 404 }),
    ] {
        let server = Server::start(Router::new().route(
            download_api_operations::Plain::METADATA.path,
            get(move || async move {
                Response::builder()
                    .status(status)
                    .body(Body::from(BYTES))
                    .unwrap()
            }),
        ))
        .await;
        assert!(matches!(server.client(1024).plain("full".to_owned()).await,
            Err(ClientError::Failure(actual)) if actual == expected));
    }
}

#[tokio::test]
async fn binary_declared_headers_reject_missing_invalid_and_duplicate_values() {
    for (values, expected) in [
        (
            vec![],
            ClientFailure::MissingResponseHeader { name: "etag" },
        ),
        (
            vec![b"\xff".as_slice()],
            ClientFailure::InvalidResponseHeader { name: "etag" },
        ),
        (
            vec![b"one".as_slice(), b"two".as_slice()],
            ClientFailure::DuplicateResponseHeader { name: "etag" },
        ),
    ] {
        let server = Server::start(Router::new().route(
            download_api_operations::Download::METADATA.path,
            get(move || {
                let values = values.clone();
                async move {
                    let mut response = Response::builder()
                        .status(200)
                        .header("content-type", "application/octet-stream")
                        .header("x-byte-length", BYTES.len())
                        .header("content-disposition", DISPOSITION);
                    for value in values {
                        response = response.header("etag", value);
                    }
                    response.body(Body::from(BYTES)).unwrap()
                }
            }),
        ))
        .await;
        assert!(
            matches!(server.client(1024).download("full".to_owned()).await,
            Err(ClientError::Failure(actual)) if actual == expected)
        );
    }
}

#[tokio::test]
async fn binary_responses_enforce_the_same_limit_with_and_without_content_length() {
    for chunked in [false, true] {
        let server = Server::start(Router::new().route(
            download_api_operations::Plain::METADATA.path,
            get(move || async move {
                if chunked {
                    let chunks = [BYTES[..3].to_vec(), BYTES[3..].to_vec()];
                    let stream = futures::stream::iter(chunks.map(Ok::<_, std::io::Error>));
                    Response::builder()
                        .status(200)
                        .body(Body::from_stream(stream))
                        .unwrap()
                } else {
                    api_macros::axum::binary_response(api_macros::axum::status(200), BYTES.into())
                }
            }),
        ))
        .await;
        assert_eq!(
            server
                .client(BYTES.len())
                .plain("full".to_owned())
                .await
                .unwrap()
                .as_ref(),
            BYTES
        );
        assert!(
            matches!(server.client(BYTES.len() - 1).plain("full".to_owned()).await,
            Err(ClientError::Failure(ClientFailure::ResponseTooLarge { limit })) if limit == BYTES.len() - 1)
        );
    }
}
