//! A single bounded response-loss injection in the shared product harness.
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::time::Instant;

use reqwest::blocking::{Client, Request};
use serde_json::Value;

use super::{EXIT_WAIT, HTTP_WAIT_STEP, STARTUP_WAIT};
use crate::{HarnessError, Result};

const MAX_HEADER: usize = 16 * 1024;
const MAX_MESSAGE: usize = 2 * 1024 * 1024;

fn failure(message: &str) -> HarnessError {
    HarnessError::Protocol(message.into())
}

pub(super) fn drop_response(client: &Client, mut request: Request) -> Result<Value> {
    let host: IpAddr = request
        .url()
        .host_str()
        .ok_or_else(|| failure("missing relay host"))?
        .parse()
        .map_err(|_| failure("relay target must be a literal loopback address"))?;
    if !host.is_loopback() || request.url().scheme() != "http" {
        return Err(failure(
            "response-loss relay only permits owned loopback HTTP",
        ));
    }
    let port = request
        .url()
        .port()
        .ok_or_else(|| failure("missing relay port"))?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let relay_address = listener.local_addr()?;
    request
        .url_mut()
        .set_port(Some(relay_address.port()))
        .map_err(|_| failure("set relay port"))?;
    request
        .url_mut()
        .set_host(Some("127.0.0.1"))
        .map_err(|_| failure("set relay host"))?;
    std::thread::scope(|scope| {
        let relay = scope.spawn(move || -> Result<Value> {
            let deadline = Instant::now() + STARTUP_WAIT;
            let mut downstream = loop {
                match listener.accept() {
                    Ok((stream, peer)) if peer.ip().is_loopback() => break stream,
                    Ok(_) => return Err(failure("non-loopback relay peer")),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(failure("response-loss relay accept deadline exceeded"));
                        }
                        std::thread::sleep(HTTP_WAIT_STEP);
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            let request_bytes = read_message(&mut DeadlineStream {
                stream: &mut downstream,
                deadline,
            })?;
            let mut upstream = TcpStream::connect_timeout(&(host, port).into(), EXIT_WAIT)?;
            upstream.set_write_timeout(Some(EXIT_WAIT))?;
            // The body, request identity and authorization bytes are forwarded
            // unchanged. Neither request nor response headers enter diagnostics.
            upstream.write_all(&request_bytes)?;
            let response = read_message(&mut DeadlineStream {
                stream: &mut upstream,
                deadline,
            })?;
            let boundary =
                header_end(&response).ok_or_else(|| failure("missing upstream header"))?;
            let status = std::str::from_utf8(&response[..boundary])
                .ok()
                .and_then(|header| header.lines().next())
                .and_then(|line| line.split_whitespace().nth(1));
            if status != Some("200") {
                return Err(failure("restore relay upstream did not return HTTP 200"));
            }
            let value = serde_json::from_slice(&response[boundary..])?;
            // Intentionally send zero bytes back; the caller must observe loss.
            drop(downstream);
            Ok(value)
        });
        let caller_failed = client.execute(request).is_err();
        let evidence = relay
            .join()
            .map_err(|_| failure("response-loss relay panicked"))??;
        if !caller_failed {
            return Err(failure("caller unexpectedly received a response"));
        }
        Ok(evidence)
    })
}

struct DeadlineStream<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}

impl Read for DeadlineStream<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "relay deadline exceeded",
            ));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(buffer)
    }
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|i| i + 4)
}

fn read_message(stream: &mut impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let mut expected = None;
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(failure("relay peer closed before complete HTTP message"));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_MESSAGE {
            return Err(failure("relay HTTP byte budget exceeded"));
        }
        if expected.is_none() {
            if let Some(boundary) = header_end(&bytes) {
                if boundary > MAX_HEADER {
                    return Err(failure("relay HTTP header budget exceeded"));
                }
                let header = std::str::from_utf8(&bytes[..boundary])
                    .map_err(|_| failure("invalid relay HTTP header"))?;
                let mut lengths = header
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>());
                let length = lengths
                    .next()
                    .ok_or_else(|| failure("relay requires Content-Length"))?
                    .map_err(|_| failure("invalid relay Content-Length"))?;
                if lengths.next().is_some()
                    || length > MAX_MESSAGE - boundary
                    || header
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
                {
                    return Err(failure("unsupported relay HTTP framing"));
                }
                expected = Some(boundary + length);
            } else if bytes.len() > MAX_HEADER {
                return Err(failure("relay HTTP header budget exceeded"));
            }
        }
        if let Some(expected) = expected {
            if bytes.len() == expected {
                return Ok(bytes);
            }
            if bytes.len() > expected {
                return Err(failure("unexpected relay trailing bytes"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_rejects_incomplete_ambiguous_or_unbounded_messages() {
        for bytes in [
            b"HTTP/1.1 200 OK\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nshort",
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n",
        ] {
            assert!(read_message(&mut &bytes[..]).is_err());
        }
        let valid = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(read_message(&mut &valid[..]).unwrap(), valid);
    }
}
