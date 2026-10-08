use wip_http::http::{Method, Request, StatusCode};
use wip_http::{Endpoint, RequestMetadataError, Route, validate_http_request};

#[test]
fn endpoint_preserves_http_and_https_when_resolving_canonical_routes() {
    for scheme in ["http", "https"] {
        let endpoint = Endpoint::parse(&format!("{scheme}://example.test/root/prefix")).unwrap();
        assert_eq!(
            endpoint.to_string(),
            format!("{scheme}://example.test/root/prefix/")
        );

        let expected = [
            (Route::Observe, "v1/observe"),
            (Route::FetchInterface, "v1/fetch_interface"),
            (Route::CallOperation, "v1/call_operation"),
        ];
        for (route, suffix) in expected {
            let url = format!("{scheme}://example.test/root/prefix/{suffix}");
            assert_eq!(endpoint.route(route).as_str(), url);
            assert_eq!(
                endpoint.recognize(&wip_http::url::Url::parse(&url).unwrap()),
                Some(route)
            );
        }
        assert_eq!(
            endpoint.recognize(
                &wip_http::url::Url::parse(&format!(
                    "{scheme}://example.test/root/prefix/v1/observe?path=/items"
                ))
                .unwrap()
            ),
            None
        );
    }
}

#[test]
fn routes_never_mix_worldspace_paths_into_endpoint_prefixes() {
    let endpoint = Endpoint::parse("https://example.test/wip/%E4%B8%96%E7%95%8C/").unwrap();
    let route = endpoint.route(Route::Observe);
    assert_eq!(route.path(), "/wip/%E4%B8%96%E7%95%8C/v1/observe");
    assert!(!route.as_str().contains("items"));
}

#[test]
fn inbound_http_metadata_enforces_post_route_and_json() {
    let endpoint = Endpoint::parse("https://example.test/wip").unwrap();
    let valid = Request::builder()
        .method(Method::POST)
        .uri("/wip/v1/observe")
        .header("content-type", "application/json; charset=utf-8")
        .body(Vec::<u8>::new())
        .unwrap();
    assert!(validate_http_request(&endpoint, Route::Observe, &valid).is_ok());

    let wrong_method = Request::builder()
        .method(Method::GET)
        .uri("/wip/v1/observe")
        .header("content-type", "application/json")
        .body(Vec::<u8>::new())
        .unwrap();
    let error = validate_http_request(&endpoint, Route::Observe, &wrong_method).unwrap_err();
    assert!(matches!(&error, RequestMetadataError::MethodNotAllowed));
    assert_eq!(error.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(error.allow(), Some("POST"));

    let wrong_route = Request::builder()
        .method(Method::POST)
        .uri("/wip/v1/fetch_interface")
        .header("content-type", "application/json")
        .body(Vec::<u8>::new())
        .unwrap();
    let error = validate_http_request(&endpoint, Route::Observe, &wrong_route).unwrap_err();
    assert!(matches!(&error, RequestMetadataError::NotFound));
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
    assert_eq!(error.allow(), None);

    let query_and_wrong_method = Request::builder()
        .method(Method::GET)
        .uri("/wip/v1/observe?unexpected=true")
        .header("content-type", "application/json")
        .body(Vec::<u8>::new())
        .unwrap();
    assert!(matches!(
        validate_http_request(&endpoint, Route::Observe, &query_and_wrong_method).unwrap_err(),
        RequestMetadataError::NotFound
    ));

    for accept in [
        "text/plain, */*;q=1, application/json;q=0",
        "application/json; charset=utf-16",
        "application/json; profile=future",
    ] {
        let incompatible_accept = Request::builder()
            .method(Method::POST)
            .uri("/wip/v1/observe")
            .header("content-type", "application/json")
            .header("accept", accept)
            .body(Vec::<u8>::new())
            .unwrap();
        let error =
            validate_http_request(&endpoint, Route::Observe, &incompatible_accept).unwrap_err();
        assert!(matches!(&error, RequestMetadataError::NotAcceptable));
        assert_eq!(error.status(), StatusCode::NOT_ACCEPTABLE);
    }

    for accept in [
        "application/json; charset=UTF-8",
        "application/json; q=1; profile=accept-extension",
    ] {
        let compatible_accept = Request::builder()
            .method(Method::POST)
            .uri("/wip/v1/observe")
            .header("content-type", "application/json")
            .header("accept", accept)
            .body(Vec::<u8>::new())
            .unwrap();
        assert!(validate_http_request(&endpoint, Route::Observe, &compatible_accept).is_ok());
    }

    let unsupported_media_type = Request::builder()
        .method(Method::POST)
        .uri("/wip/v1/observe")
        .header("content-type", "text/plain")
        .header("accept", "application/json")
        .body(Vec::<u8>::new())
        .unwrap();
    let error =
        validate_http_request(&endpoint, Route::Observe, &unsupported_media_type).unwrap_err();
    assert!(matches!(&error, RequestMetadataError::UnsupportedMediaType));
    assert_eq!(error.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    for coding in ["gzip", "identity, gzip"] {
        let unsupported_coding = Request::builder()
            .method(Method::POST)
            .uri("/wip/v1/observe")
            .header("content-type", "application/json")
            .header("content-encoding", coding)
            .body(Vec::<u8>::new())
            .unwrap();
        let error =
            validate_http_request(&endpoint, Route::Observe, &unsupported_coding).unwrap_err();
        assert!(matches!(&error, RequestMetadataError::UnsupportedMediaType));
        assert_eq!(error.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    let identity_coding = Request::builder()
        .method(Method::POST)
        .uri("/wip/v1/observe")
        .header("content-type", "application/json")
        .header("content-encoding", "identity")
        .body(Vec::<u8>::new())
        .unwrap();
    assert!(validate_http_request(&endpoint, Route::Observe, &identity_coding).is_ok());
}

#[test]
fn invalid_endpoint_components_and_non_routes_are_rejected() {
    assert!(Endpoint::parse("/relative").is_err());
    assert!(Endpoint::parse("file:///wip").is_err());
    assert!(Endpoint::parse("https://example.test/wip?q=1").is_err());
    assert!(Endpoint::parse("https://example.test/wip#fragment").is_err());

    let deployment_endpoint = Endpoint::parse("http://user:secret@example.test/wip").unwrap();
    assert_eq!(deployment_endpoint.as_url().scheme(), "http");
    assert_eq!(deployment_endpoint.as_url().username(), "user");

    let endpoint = Endpoint::parse("http://example.test/wip").unwrap();
    for outside in [
        "http://example.test/wip/v1/unknown",
        "http://example.test/wip/v1/batch",
        "http://example.test/wip/v1/observe/extra",
        "https://example.test/wip/v1/observe",
        "http://other.test/wip/v1/observe",
    ] {
        assert_eq!(
            endpoint.recognize(&wip_http::url::Url::parse(outside).unwrap()),
            None
        );
    }
}
