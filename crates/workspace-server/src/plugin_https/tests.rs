//! Component/connection tests for the production HTTPS adapter. Only DNS answers,
//! the socket route to an isolated peer, and that peer's trust root are replaced;
//! request validation, public-address admission, pinning, TLS name verification,
//! HTTP serialization, response limits and cancellation remain production code.
//! Run with the crate's normal lib tests (`plugin_https` filter). TLS fixtures
//! require the `openssl` executable and never change process environment or trust.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use super::test_support::*;
use super::*;

#[test]
fn ambiguous_or_out_of_scope_paths_are_denied() {
    let binding = binding();
    for path in [
        "https://other.example/api",
        "//other.example/api",
        "/api?x=y",
        "/api#fragment",
        "/api\\item",
        "/api/../item",
        "/api/./item",
        "/api//item",
        "/api/item/",
        "/api/%2e%2e/item",
        "/api/%2Fitem",
        "/api/%5citem",
        "/api/%252fitem",
        "/apix",
        "/api;param",
        "/",
        "/api/é",
        "/api/\nitem",
    ] {
        let mut request = request();
        request.path = path.into();
        assert!(
            matches!(prepare(&binding, request), Err(TransportError::Denied)),
            "{path:?}"
        );
    }
    for path in ["/api", "/api/items", "/api/items-1.json", "/api/a:b@c"] {
        let mut request = request();
        request.path = path.into();
        let prepared = prepare(&binding, request).unwrap();
        assert_eq!(prepared.url.path(), path);
        assert_eq!(prepared.normalized().path, path);
    }
}

#[test]
fn query_encoding_and_normalized_headers_preserve_duplicates_and_order() {
    let mut request = request();
    request.query = vec![("q".into(), "a b&c".into()), ("q".into(), "/?é".into())];
    request.headers = vec![
        ("X-Order".into(), "second".into()),
        ("x-order".into(), "first".into()),
    ];
    let prepared = prepare(&binding(), request.clone()).unwrap();
    assert_eq!(prepared.url.query(), Some("q=a+b%26c&q=%2F%3F%C3%A9"));
    assert_eq!(prepared.normalized().query, request.query);
    assert_eq!(
        prepared.normalized().headers,
        vec![
            ("x-order".into(), "second".into()),
            ("x-order".into(), "first".into())
        ]
    );
    let json = serde_json::to_value(prepared.normalized()).unwrap();
    assert_eq!(json["method"], "GET");
    assert_eq!(json["headers"][0][0], "x-order");
}

#[test]
fn credential_and_control_headers_never_gain_authority_from_allowlists() {
    for name in [
        "authorization",
        "cookie",
        "cookie2",
        "set-cookie",
        "host",
        "connection",
        "keep-alive",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "content-length",
        "expect",
        "proxy-authorization",
        "proxy-anything",
        "sec-websocket-key",
        "forwarded",
        "via",
        "x-forwarded-arbitrary",
        "x-api-key",
        "api-key",
        "accept-encoding",
    ] {
        let mut binding = binding();
        binding.request_headers.push(name.into());
        let mut request = request();
        request
            .headers
            .push((name.to_uppercase(), "must-not-send".into()));
        assert!(prepare(&binding, request).is_err(), "{name}");
    }
    for value in ["a\r\nb", "a\0b", "a\tb", "é"] {
        let mut request = request();
        request.headers.push(("accept".into(), value.into()));
        assert!(matches!(
            prepare(&binding(), request),
            Err(TransportError::InvalidRequest)
        ));
    }
}

#[test]
fn request_limits_timeout_and_effect_identity_are_validated_before_dispatch() {
    let mut binding = binding();
    binding.limits.max_request_bytes = 4;
    let mut input = request();
    input.body = vec![0; 4];
    input.timeout_ms = 60_000;
    assert_eq!(
        prepare(&binding, input.clone())
            .unwrap()
            .normalized()
            .timeout_ms,
        5_000
    );
    input.body.push(0);
    assert!(matches!(
        prepare(&binding, input),
        Err(TransportError::RequestLimit)
    ));
    let mut input = request();
    input.headers = vec![("accept".into(), "ok".into()); 65];
    assert!(matches!(
        prepare(&binding, input),
        Err(TransportError::HeaderLimit)
    ));
    let mut input = request();
    input.headers = vec![("accept".into(), "x".repeat(4096))];
    assert!(matches!(
        prepare(&binding, input),
        Err(TransportError::HeaderLimit)
    ));
    let mut input = request();
    input.query = vec![("q".into(), "é".repeat(4000))];
    assert!(matches!(
        prepare(&binding, input),
        Err(TransportError::RequestLimit)
    ));
    for key in [String::new(), "a".repeat(129), "a\nb".into()] {
        let mut input = request();
        input.effect_key = key;
        assert!(matches!(
            prepare(&binding, input),
            Err(TransportError::InvalidRequest)
        ));
    }
    let mut input = request();
    input.timeout_ms = 0;
    assert!(matches!(
        prepare(&binding, input),
        Err(TransportError::InvalidRequest)
    ));
    let mut input = request();
    input.method = PluginHttpMethod::Delete;
    assert!(matches!(
        prepare(&binding, input),
        Err(TransportError::Denied)
    ));
}

#[test]
fn all_dns_candidates_must_be_public_and_outside_operator_exclusions() {
    let public = addr("8.8.8.8", 443);
    for ip in [
        "0.0.0.0",
        "10.1.2.3",
        "100.100.100.200",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.168.1.1",
        "192.0.0.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "255.255.255.255",
        "168.63.129.16",
        "::",
        "::1",
        "::ffff:8.8.8.8",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "2001:db8::1",
        "2001::1",
        "2002:0808:0808::1",
        "64:ff9b::808:808",
        "3fff::1",
    ] {
        for candidates in [vec![addr(ip, 443), public], vec![public, addr(ip, 443)]] {
            assert_eq!(
                check_addresses(&candidates, &policy()),
                Err(TransportError::Denied),
                "{ip}"
            );
        }
    }
    assert_eq!(
        check_addresses(&[public, addr("2606:4700:4700::1111", 443)], &policy()),
        Ok(())
    );
    let mut denied = policy();
    denied.denied_addresses.insert(public.ip());
    assert_eq!(
        check_addresses(&[public], &denied),
        Err(TransportError::Denied)
    );
    assert_eq!(check_addresses(&[], &policy()), Err(TransportError::Dns));
    assert_eq!(
        check_addresses(&vec![public; 65], &policy()),
        Err(TransportError::Dns)
    );
}

#[tokio::test]
async fn pinned_connector_rejects_any_other_hostname_without_dns_fallback() {
    use reqwest::dns::Resolve;
    let resolver = PinnedResolver::new(HOST.into(), vec![addr("8.8.8.8", 443)]);
    let resolved: Vec<_> = resolver
        .resolve(HOST.parse().unwrap())
        .await
        .unwrap()
        .collect();
    assert_eq!(resolved, vec![addr("8.8.8.8", 443)]);
    assert!(
        resolver
            .resolve("other.example.com".parse().unwrap())
            .await
            .is_err()
    );
}

async fn execute(
    transport: &HttpsTransport,
    binding: &PluginConnectionBinding,
    input: HttpsRequest,
    secret: Option<&str>,
) -> Result<HttpsResponse, TransportError> {
    transport
        .execute(
            binding,
            prepare(binding, input)?,
            secret,
            &CancellationToken::new(),
        )
        .await
}

#[tokio::test]
async fn tls_peer_receives_encoded_query_duplicate_headers_and_scoped_secret() {
    let mut peer = Peer::new(
        HOST,
        vec![response(
            503,
            &[
                ("Content-Type", "application/octet-stream"),
                ("X-Order", "first"),
                ("X-Order", "second"),
                ("Set-Cookie", "must-not-return"),
                ("Authorization", "must-not-return"),
            ],
            &[0, 255, 128],
        )],
    )
    .await;
    let mut binding = peer.binding();
    binding.secret_ref = "credential-ref".into();
    let mut input = request();
    input.method = PluginHttpMethod::Post;
    input.query = vec![("q".into(), "a & b".into()), ("q".into(), "/?".into())];
    input.headers = vec![
        ("x-order".into(), "second".into()),
        ("X-Order".into(), "first".into()),
    ];
    input.body = vec![1, 0, 255];
    let output = execute(
        &peer.transport(),
        &binding,
        input,
        Some("Bearer isolated-test-secret"),
    )
    .await
    .unwrap();
    assert_eq!(output.status, 503);
    assert_eq!(output.body, vec![0, 255, 128]);
    assert_eq!(
        output
            .headers
            .iter()
            .filter(|(k, _)| k == "x-order")
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        !output
            .headers
            .iter()
            .any(|(k, _)| k == "set-cookie" || k == "authorization")
    );
    let wire = peer.observed().await;
    let text = String::from_utf8_lossy(&wire).to_lowercase();
    assert!(text.starts_with("post /api/items?q=a+%26+b&q=%2f%3f http/1.1\r\n"));
    assert!(text.contains(&format!("host: {HOST}:{}\r\n", peer.address.port())));
    assert!(text.contains("authorization: bearer isolated-test-secret\r\n"));
    assert!(text.contains("x-order: second\r\nx-order: first\r\n"));
    assert!(wire.ends_with(&[1, 0, 255]));
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn response_connection_nominated_headers_are_not_exposed() {
    let peer = Peer::new(
        HOST,
        vec![response(
            200,
            &[
                ("Connection", "x-order"),
                ("X-Order", "hop-value"),
                ("Content-Type", "text/plain"),
            ],
            b"ok",
        )],
    )
    .await;
    let output = execute(&peer.transport(), &peer.binding(), request(), None)
        .await
        .unwrap();
    assert!(!output.headers.iter().any(|(name, _)| name == "x-order"));
    assert!(
        output
            .headers
            .iter()
            .any(|(name, _)| name == "content-type")
    );
}

#[tokio::test]
async fn redirects_are_returned_without_following_or_forwarding_credentials() {
    let peer = Peer::new(
        HOST,
        vec![response(
            302,
            &[("Location", "https://127.0.0.1/private")],
            b"redirect",
        )],
    )
    .await;
    let mut binding = peer.binding();
    binding.secret_ref = "credential-ref".into();
    let output = execute(
        &peer.transport(),
        &binding,
        request(),
        Some("Bearer secret"),
    )
    .await
    .unwrap();
    assert_eq!(output.status, 302);
    assert_eq!(output.body, b"redirect");
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tls_name_mismatch_and_untrusted_certificate_are_rejected() {
    for (host, trust) in [("wrong.example.com", true), (HOST, false)] {
        let peer = Peer::new(host, vec![]).await;
        let mut transport = peer.transport();
        if !trust {
            transport.test_overrides.as_mut().unwrap().root = None;
        }
        assert_eq!(
            execute(&transport, &peer.binding(), request(), None).await,
            Err(TransportError::Unavailable)
        );
        assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
        assert!(peer.observations.is_empty());
    }
}

#[tokio::test]
async fn response_loss_never_retries_even_a_get() {
    let mut peer = Peer::new(
        HOST,
        vec![Reply::Close, response(200, &[], b"must-not-retry")],
    )
    .await;
    assert_eq!(
        execute(&peer.transport(), &peer.binding(), request(), None).await,
        Err(TransportError::Unavailable)
    );
    peer.observed().await;
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mixed_dns_is_denied_before_connect_and_rebinding_is_rechecked_per_execution() {
    let peer = Peer::new(HOST, vec![response(200, &[], b"ok")]).await;
    let port = peer.address.port();
    let transport =
        peer.transport_with_answers(vec![vec![addr("8.8.8.8", port), addr("127.0.0.1", port)]]);
    assert_eq!(
        execute(&transport, &peer.binding(), request(), None).await,
        Err(TransportError::Denied)
    );
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    let transport = peer.transport_with_answers(vec![
        vec![addr("8.8.8.8", port)],
        vec![addr("127.0.0.1", port)],
    ]);
    assert_eq!(
        execute(&transport, &peer.binding(), request(), None)
            .await
            .unwrap()
            .body,
        b"ok"
    );
    assert_eq!(
        transport
            .test_overrides
            .as_ref()
            .unwrap()
            .calls
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        execute(&transport, &peer.binding(), request(), None).await,
        Err(TransportError::Denied)
    );
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport
            .test_overrides
            .as_ref()
            .unwrap()
            .calls
            .load(Ordering::SeqCst),
        2
    );
}

#[tokio::test]
async fn operator_control_plane_exclusions_apply_before_connect() {
    let peer = Peer::new(HOST, vec![]).await;
    for denied_host in [false, true] {
        let mut transport = peer.transport();
        if denied_host {
            transport.policy.denied_hosts.insert(HOST.to_uppercase());
        } else {
            transport
                .policy
                .denied_addresses
                .insert("8.8.8.8".parse().unwrap());
        }
        assert_eq!(
            execute(&transport, &peer.binding(), request(), None).await,
            Err(TransportError::Denied)
        );
    }
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn binding_swap_invalid_credentials_and_generated_header_overflow_never_dispatch() {
    let peer = Peer::new(HOST, vec![]).await;
    let transport = peer.transport();
    let mut binding = peer.binding();
    let prepared = prepare(&binding, request()).unwrap();
    binding.path_prefixes = vec!["/api/items".into()];
    assert_eq!(
        transport
            .execute(&binding, prepared, None, &CancellationToken::new())
            .await,
        Err(TransportError::Denied)
    );
    for (reference, secret) in [
        ("", Some("Bearer unexpected")),
        ("credential", None),
        ("credential", Some("Bearer a\r\nb")),
    ] {
        let mut binding = peer.binding();
        binding.secret_ref = reference.into();
        // Each rejected operation gets an explicit answer; none opens a socket.
        let transport = peer.transport();
        assert_eq!(
            execute(&transport, &binding, request(), secret).await,
            Err(TransportError::InvalidSecret)
        );
    }
    let mut binding = peer.binding();
    binding.secret_ref = "credential".into();
    binding.limits.max_header_bytes = 64;
    assert_eq!(
        execute(
            &peer.transport(),
            &binding,
            request(),
            Some(&"s".repeat(100))
        )
        .await,
        Err(TransportError::HeaderLimit)
    );
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn response_wire_size_count_and_header_bytes_are_bounded_before_return() {
    let cases = [
        response(200, &[], &vec![0; 1025]),
        Reply::Bytes(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n401\r\n"
                .iter()
                .copied()
                .chain(vec![0; 1025])
                .chain(b"\r\n0\r\n\r\n".iter().copied())
                .collect(),
        ),
        response(200, &[("X-Order", &"x".repeat(4096))], b"ok"),
        response(200, &vec![("X-Order", "x"); 65], b"ok"),
    ];
    for (index, reply) in cases.into_iter().enumerate() {
        let peer = Peer::new(HOST, vec![reply]).await;
        let result = execute(&peer.transport(), &peer.binding(), request(), None).await;
        let expected = if index < 2 {
            TransportError::ResponseLimit
        } else {
            TransportError::HeaderLimit
        };
        assert_eq!(result, Err(expected), "case {index}");
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[tokio::test]
async fn gzip_is_bounded_after_expansion_including_concatenated_members() {
    for (payload, accepted) in [
        (gzip(&vec![0; 1024]), true),
        (gzip(&vec![0; 1025]), false),
        ([gzip(&vec![0; 600]), gzip(&vec![0; 600])].concat(), false),
    ] {
        let peer = Peer::new(
            HOST,
            vec![response(200, &[("Content-Encoding", "gzip")], &payload)],
        )
        .await;
        let result = execute(&peer.transport(), &peer.binding(), request(), None).await;
        if accepted {
            let result = result.unwrap();
            assert_eq!(result.body.len(), 1024);
            assert!(!result.headers.iter().any(|(k, _)| k == "content-encoding"));
        } else {
            assert_eq!(result, Err(TransportError::ResponseLimit));
        }
    }
}

#[tokio::test]
async fn unsupported_or_corrupt_compression_is_not_returned_as_a_success() {
    for (headers, expected) in [
        (
            vec![("Content-Encoding", "br")],
            TransportError::UnsupportedEncoding,
        ),
        (
            vec![
                ("Content-Encoding", "gzip"),
                ("Content-Encoding", "identity"),
            ],
            TransportError::UnsupportedEncoding,
        ),
        (
            vec![("Content-Encoding", "gzip")],
            TransportError::Unavailable,
        ),
    ] {
        let peer = Peer::new(HOST, vec![response(200, &headers, b"not-compressed")]).await;
        assert_eq!(
            execute(&peer.transport(), &peer.binding(), request(), None).await,
            Err(expected)
        );
    }
}

#[tokio::test]
async fn cancellation_covers_dns_and_pre_cancelled_requests_without_network() {
    let mut transport = HttpsTransport::new(policy());
    let calls = Arc::new(AtomicUsize::new(0));
    transport.test_overrides = Some(TestOverrides {
        connect_to: None,
        root: None,
        answers: Mutex::new(VecDeque::from([DnsAnswer::Pending(None)])),
        calls: calls.clone(),
    });
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let binding = binding();
    assert_eq!(
        transport
            .execute(
                &binding,
                prepare(&binding, request()).unwrap(),
                None,
                &cancellation
            )
            .await,
        Err(TransportError::Cancelled)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut input = request();
    input.timeout_ms = 20;
    assert_eq!(
        execute(&transport, &binding, input, None).await,
        Err(TransportError::DeadlineExceeded)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let (started, observed) = tokio::sync::oneshot::channel();
    transport
        .test_overrides
        .as_mut()
        .unwrap()
        .answers
        .lock()
        .unwrap()
        .push_back(DnsAnswer::Pending(Some(started)));
    let cancellation = CancellationToken::new();
    let operation = transport.execute(
        &binding,
        prepare(&binding, request()).unwrap(),
        None,
        &cancellation,
    );
    tokio::pin!(operation);
    tokio::select! {
        value = &mut operation => panic!("pending DNS completed unexpectedly: {value:?}"),
        value = observed => value.unwrap(),
    }
    cancellation.cancel();
    assert_eq!(operation.await, Err(TransportError::Cancelled));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancellation_and_deadline_abort_inflight_response_waits() {
    for reply in [
        Reply::Stall,
        Reply::StallAfter(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\na".to_vec()),
    ] {
        let mut peer = Peer::new(HOST, vec![reply]).await;
        let transport = peer.transport();
        let binding = peer.binding();
        let cancellation = CancellationToken::new();
        let operation = transport.execute(
            &binding,
            prepare(&binding, request()).unwrap(),
            None,
            &cancellation,
        );
        tokio::pin!(operation);
        tokio::select! {
            value = &mut operation => panic!("response completed before cancellation: {value:?}"),
            _ = peer.observed() => {},
        }
        cancellation.cancel();
        assert_eq!(operation.await, Err(TransportError::Cancelled));
        assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
    }
    let mut peer = Peer::new(HOST, vec![Reply::Stall]).await;
    let mut input = request();
    input.timeout_ms = 250;
    let result = execute(&peer.transport(), &peer.binding(), input, None).await;
    assert_eq!(result, Err(TransportError::DeadlineExceeded));
    peer.observed().await;
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn revocation_during_dns_is_checked_by_final_guard_without_opening_peer_connection() {
    let peer = Peer::new(HOST, vec![]).await;
    let mut transport = peer.transport();
    let (dns_started, dns_observed) = tokio::sync::oneshot::channel();
    let (dns_release, dns_answer) = tokio::sync::oneshot::channel();
    transport.test_overrides.as_mut().unwrap().answers =
        Mutex::new(VecDeque::from([DnsAnswer::Barrier {
            started: dns_started,
            release: dns_answer,
        }]));
    let authority = AtomicBool::new(true);
    let guard_called = AtomicBool::new(false);
    let binding = peer.binding();
    let cancellation = CancellationToken::new();
    let operation = transport.execute_with_guard(
        &binding,
        prepare(&binding, request()).unwrap(),
        None,
        &cancellation,
        || async {
            guard_called.store(true, Ordering::SeqCst);
            tokio::task::yield_now().await;
            if authority.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(TransportError::Denied)
            }
        },
    );
    tokio::pin!(operation);
    tokio::select! {
        value = &mut operation => panic!("operation completed during blocked DNS: {value:?}"),
        value = dns_observed => value.unwrap(),
    }
    assert!(!guard_called.load(Ordering::SeqCst));
    assert_eq!(peer.connection_count(), 0);
    authority.store(false, Ordering::SeqCst);
    dns_release
        .send(vec![addr("8.8.8.8", peer.address.port())])
        .unwrap();
    assert_eq!(operation.await, Err(TransportError::Denied));
    assert!(guard_called.load(Ordering::SeqCst));
    assert_eq!(peer.connection_count(), 0);
    assert!(peer.observations.is_empty());
}

#[tokio::test]
async fn successful_async_dispatch_guard_runs_once_and_allows_the_prepared_request() {
    let peer = Peer::new(HOST, vec![response(200, &[], b"guarded")]).await;
    let transport = peer.transport();
    let binding = peer.binding();
    let calls = AtomicUsize::new(0);
    let output = transport
        .execute_with_guard(
            &binding,
            prepare(&binding, request()).unwrap(),
            None,
            &CancellationToken::new(),
            || async {
                tokio::task::yield_now().await;
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(output.body, b"guarded");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(peer.connection_count(), 1);
}

#[tokio::test]
async fn dispatch_guard_is_cancelled_with_the_operation_and_never_opens_a_connection() {
    let peer = Peer::new(HOST, vec![]).await;
    let transport = peer.transport();
    let binding = peer.binding();
    let cancellation = CancellationToken::new();
    let (started, observed) = tokio::sync::oneshot::channel();
    let operation = transport.execute_with_guard(
        &binding,
        prepare(&binding, request()).unwrap(),
        None,
        &cancellation,
        || async {
            started.send(()).unwrap();
            std::future::pending::<Result<(), TransportError>>().await
        },
    );
    tokio::pin!(operation);
    tokio::select! {
        value = &mut operation => panic!("pending authority guard completed: {value:?}"),
        value = observed => value.unwrap(),
    }
    cancellation.cancel();
    assert_eq!(operation.await, Err(TransportError::Cancelled));
    assert_eq!(peer.connection_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn dispatch_guard_deadline_expiry_prevents_connection_even_if_guard_returns_success() {
    let peer = Peer::new(HOST, vec![]).await;
    let transport = peer.transport();
    let binding = peer.binding();
    let result = transport
        .execute_with_guard(
            &binding,
            prepare(&binding, request()).unwrap(),
            None,
            &CancellationToken::new(),
            || async {
                tokio::time::advance(Duration::from_secs(6)).await;
                Ok(())
            },
        )
        .await;
    assert_eq!(result, Err(TransportError::DeadlineExceeded));
    assert_eq!(peer.connection_count(), 0);
}

#[tokio::test]
async fn dispatch_guard_cancellation_prevents_connection_even_if_guard_returns_success() {
    let peer = Peer::new(HOST, vec![]).await;
    let transport = peer.transport();
    let binding = peer.binding();
    let cancellation = CancellationToken::new();
    let result = transport
        .execute_with_guard(
            &binding,
            prepare(&binding, request()).unwrap(),
            None,
            &cancellation,
            || async {
                cancellation.cancel();
                Ok(())
            },
        )
        .await;
    assert_eq!(result, Err(TransportError::Cancelled));
    assert_eq!(peer.connection_count(), 0);
}

#[test]
fn diagnostic_formats_omit_request_response_urls_paths_and_secrets() {
    let mut input = request();
    input.path = "/api/private-path".into();
    input.body = b"secret-body".to_vec();
    let prepared = prepare(&binding(), input.clone()).unwrap();
    let output = HttpsResponse {
        status: 200,
        headers: vec![("x-order".into(), "secret-header".into())],
        body: b"secret-body".to_vec(),
    };
    let debug = format!("{input:?} {prepared:?} {output:?}");
    for forbidden in [HOST, "private-path", "secret-body", "secret-header"] {
        assert!(!debug.contains(forbidden));
    }
    let error = TransportError::Unavailable;
    assert_eq!(error.to_string(), "HTTPS transport unavailable");
    assert!(std::error::Error::source(&error).is_none());
}
