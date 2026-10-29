//! Isolated TLS peer for crate-local integration tests. Production builds do
//! not contain its private DNS/socket/trust overrides. The certificate hostname
//! still passes through the real HTTPS client's name verification and SNI.

use std::collections::VecDeque;
use std::future::pending;
use std::io::Write;
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use super::*;
use crate::plugin_binding::{PluginConnectionLimits, PluginNetworkPolicy};

pub(crate) const HOST: &str = "api.example.com";

pub(super) struct TestOverrides {
    pub(super) connect_to: Option<SocketAddr>,
    pub(super) root: Option<reqwest::Certificate>,
    pub(super) answers: Mutex<VecDeque<DnsAnswer>>,
    pub(super) calls: Arc<AtomicUsize>,
}

pub(super) enum DnsAnswer {
    Addresses(Vec<SocketAddr>),
    Pending(Option<tokio::sync::oneshot::Sender<()>>),
    Barrier {
        started: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<Vec<SocketAddr>>,
    },
}

impl TestOverrides {
    pub(super) async fn resolve(&self) -> Result<Vec<SocketAddr>, TransportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answers.lock().unwrap().pop_front();
        match answer {
            Some(DnsAnswer::Addresses(addresses)) => Ok(addresses),
            Some(DnsAnswer::Barrier { started, release }) => {
                let _ = started.send(());
                release.await.map_err(|_| TransportError::Dns)
            }
            Some(DnsAnswer::Pending(started)) => {
                if let Some(started) = started {
                    let _ = started.send(());
                }
                pending().await
            }
            None => Err(TransportError::Dns),
        }
    }
}

pub(super) fn policy() -> TransportPolicy {
    TransportPolicy {
        denied_addresses: BTreeSet::new(),
        denied_hosts: BTreeSet::new(),
    }
}

pub(super) fn binding() -> PluginConnectionBinding {
    PluginConnectionBinding {
        origin: format!("https://{HOST}"),
        network: PluginNetworkPolicy::PublicInternet,
        methods: vec![PluginHttpMethod::Get, PluginHttpMethod::Post],
        path_prefixes: vec!["/api".into()],
        request_headers: vec!["accept".into(), "x-order".into()],
        response_headers: vec![
            "content-type".into(),
            "x-order".into(),
            "location".into(),
            "content-encoding".into(),
        ],
        secret_ref: String::new(),
        limits: PluginConnectionLimits {
            timeout_ms: 5_000,
            max_request_bytes: 1024,
            max_response_bytes: 1024,
            max_header_bytes: 4096,
        },
    }
}

pub(super) fn request() -> HttpsRequest {
    HttpsRequest {
        method: PluginHttpMethod::Get,
        path: "/api/items".into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        timeout_ms: 5_000,
        effect_key: "effect-1".into(),
    }
}

pub(super) fn addr(ip: &str, port: u16) -> SocketAddr {
    SocketAddr::new(ip.parse().unwrap(), port)
}

pub(crate) struct Peer {
    pub(super) address: SocketAddr,
    pub(super) root: reqwest::Certificate,
    pub(super) observations: mpsc::Receiver<Vec<u8>>,
    pub(super) connections: Arc<AtomicUsize>,
    closed: mpsc::Receiver<()>,
    task: JoinHandle<()>,
    _directory: tempfile::TempDir,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) enum Reply {
    Bytes(Vec<u8>),
    StallAfter(Vec<u8>),
    Stall,
    Close,
    Gated(tokio::sync::oneshot::Receiver<Vec<u8>>),
}

impl Peer {
    pub(crate) async fn new(certificate_host: &str, replies: Vec<Reply>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let output = Command::new("openssl")
            .current_dir(directory.path())
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-noenc",
                "-days",
                "1",
                "-keyout",
                "key.pem",
                "-out",
                "cert.pem",
                "-subj",
                "/CN=isolated-plugin-peer",
                "-addext",
                &format!("subjectAltName=DNS:{certificate_host}"),
                "-addext",
                "extendedKeyUsage=serverAuth",
            ])
            .output()
            .expect("TLS tests require openssl");
        assert!(
            output.status.success(),
            "isolated TLS certificate generation failed"
        );
        let certificate = std::fs::read(directory.path().join("cert.pem")).unwrap();
        let key = std::fs::read(directory.path().join("key.pem")).unwrap();
        let identity = native_tls::Identity::from_pkcs8(&certificate, &key).unwrap();
        let acceptor =
            tokio_native_tls::TlsAcceptor::from(native_tls::TlsAcceptor::new(identity).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (observed, observations) = mpsc::channel(16);
        let (closed_sender, closed) = mpsc::channel(16);
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        let mut replies = VecDeque::from(replies);
        let task = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        count.fetch_add(1, Ordering::SeqCst);
                        let acceptor = acceptor.clone();
                        let observed = observed.clone();
                        let closed_sender = closed_sender.clone();
                        let reply = replies.pop_front().unwrap_or(Reply::Close);
                        handlers.spawn(async move {
                            let Ok(mut socket) = acceptor.accept(socket).await else { return; };
                            let mut bytes = Vec::new();
                            let mut buffer = [0; 4096];
                            loop {
                                let Ok(length) = socket.read(&mut buffer).await else { return; };
                                if length == 0 { return; }
                                bytes.extend_from_slice(&buffer[..length]);
                                assert!(bytes.len() <= 2 * 1024 * 1024, "test request exceeded fixture bound");
                                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                                    let text = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                                    let length = text.lines().find_map(|line| line.strip_prefix("content-length: "))
                                        .map(|n| n.parse::<usize>().unwrap()).unwrap_or(0);
                                    if bytes.len() >= end + 4 + length { break; }
                                }
                            }
                            observed.send(bytes).await.unwrap();
                            match reply {
                                Reply::Bytes(bytes) => { let _ = socket.write_all(&bytes).await; let _ = socket.shutdown().await; }
                                Reply::StallAfter(bytes) => { let _ = socket.write_all(&bytes).await; pending::<()>().await; }
                                Reply::Stall => {
                                    let mut byte = [0];
                                    match socket.read(&mut byte).await {
                                        Ok(0) | Err(_) => { let _ = closed_sender.send(()).await; }
                                        Ok(_) => panic!("unexpected bytes after complete request"),
                                    }
                                },
                                Reply::Close => {},
                                Reply::Gated(release) => {
                                    if let Ok(bytes) = release.await {
                                        let _ = socket.write_all(&bytes).await;
                                        let _ = socket.shutdown().await;
                                    }
                                }
                            }
                        });
                    }
                    Some(result) = handlers.join_next(), if !handlers.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            address,
            root: reqwest::Certificate::from_pem(&certificate).unwrap(),
            observations,
            connections,
            closed,
            task,
            _directory: directory,
        }
    }

    pub(crate) fn binding(&self) -> PluginConnectionBinding {
        let mut binding = binding();
        binding.origin = format!("https://{HOST}:{}", self.address.port());
        binding
    }

    pub(crate) fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub(crate) async fn closed(&mut self) {
        tokio::time::timeout(Duration::from_secs(2), self.closed.recv())
            .await
            .unwrap()
            .unwrap();
    }

    pub(crate) fn transport_with_policy(&self, policy: TransportPolicy) -> HttpsTransport {
        let mut transport = self.transport_for_requests(4);
        transport.policy = policy;
        transport
    }

    pub(crate) fn transport(&self) -> HttpsTransport {
        self.transport_for_requests(1)
    }

    pub(crate) fn transport_for_requests(&self, requests: usize) -> HttpsTransport {
        self.transport_with_answers(vec![vec![addr("8.8.8.8", self.address.port())]; requests])
    }

    pub(crate) fn transport_with_answers(&self, answers: Vec<Vec<SocketAddr>>) -> HttpsTransport {
        let mut transport = HttpsTransport::new(policy());
        transport.test_overrides = Some(TestOverrides {
            connect_to: Some(self.address),
            root: Some(self.root.clone()),
            answers: Mutex::new(answers.into_iter().map(DnsAnswer::Addresses).collect()),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        transport
    }

    pub(crate) fn transport_with_dns_gate(
        &self,
    ) -> (
        HttpsTransport,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<Vec<SocketAddr>>,
    ) {
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, answer) = tokio::sync::oneshot::channel();
        let mut transport = self.transport();
        transport.test_overrides.as_mut().unwrap().answers = Mutex::new(
            [DnsAnswer::Barrier {
                started,
                release: answer,
            }]
            .into_iter()
            .collect(),
        );
        (transport, waiting, release)
    }

    pub(crate) fn public_address(&self) -> SocketAddr {
        addr("8.8.8.8", self.address.port())
    }

    pub(crate) async fn observed(&mut self) -> Vec<u8> {
        self.observed_within(Duration::from_secs(5)).await
    }

    pub(crate) async fn observed_within(&mut self, deadline: Duration) -> Vec<u8> {
        tokio::time::timeout(deadline, self.observations.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

pub(crate) fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    let mut response = format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .into_bytes();
    for (name, value) in headers {
        write!(response, "{name}: {value}\r\n").unwrap();
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body);
    Reply::Bytes(response)
}
