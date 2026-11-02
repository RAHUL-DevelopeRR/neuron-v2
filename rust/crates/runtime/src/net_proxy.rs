//! A loopback HTTP proxy that only reaches hosts on the sandbox network
//! allowlist ([`crate::net_allow`]).
//!
//! It handles `CONNECT host:port` (HTTPS and any TLS client honouring
//! `HTTPS_PROXY`) by tunnelling bytes, and absolute-form plain HTTP requests
//! (`GET http://host/path`) by forwarding them. Anything else, or a host not
//! on the list, gets `403`. It binds `127.0.0.1` on an ephemeral port, one
//! per process, started on first use; the allowlist it checks is replaced on
//! each [`ensure_started`] call so a settings change applies to the next
//! command.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::thread;
use std::time::Duration;

use crate::net_allow::NetworkAllowlist;

const MAX_HEAD_BYTES: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A running proxy.
#[derive(Debug)]
pub struct AllowlistProxy {
    addr: SocketAddr,
    allowlist: Arc<RwLock<NetworkAllowlist>>,
}

impl AllowlistProxy {
    /// Starts a proxy on `127.0.0.1:0` in a background thread.
    pub fn start(allowlist: NetworkAllowlist) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let allowlist = Arc::new(RwLock::new(allowlist));
        let shared = Arc::clone(&allowlist);
        thread::Builder::new()
            .name("neuron-net-proxy".to_string())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let allowlist = Arc::clone(&shared);
                    let _ = thread::Builder::new()
                        .name("neuron-net-proxy-conn".to_string())
                        .spawn(move || {
                            let list = allowlist
                                .read()
                                .unwrap_or_else(PoisonError::into_inner)
                                .clone();
                            let _ = handle(stream, &list);
                        });
                }
            })?;
        Ok(Self { addr, allowlist })
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn set_allowlist(&self, allowlist: NetworkAllowlist) {
        *self
            .allowlist
            .write()
            .unwrap_or_else(PoisonError::into_inner) = allowlist;
    }
}

/// The process-wide proxy, started on first call, with `allowlist` in force.
pub fn ensure_started(allowlist: NetworkAllowlist) -> io::Result<SocketAddr> {
    static PROXY: OnceLock<AllowlistProxy> = OnceLock::new();
    if let Some(proxy) = PROXY.get() {
        proxy.set_allowlist(allowlist);
        return Ok(proxy.addr());
    }
    let started = AllowlistProxy::start(allowlist.clone())?;
    let proxy = PROXY.get_or_init(|| started);
    proxy.set_allowlist(allowlist);
    Ok(proxy.addr())
}

fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD_BYTES || stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad head"));
        }
        head.push(byte[0]);
    }
    Ok(head)
}

fn deny(stream: &mut TcpStream, what: &str) -> io::Result<()> {
    let body = format!("blocked by the Neuron sandbox network allowlist: {what}\n");
    write!(
        stream,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn connect(authority: &str) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
    for addr in authority.to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn handle(mut client: TcpStream, allowlist: &NetworkAllowlist) -> io::Result<()> {
    let head = read_head(&mut client)?;
    let text = String::from_utf8_lossy(&head).into_owned();
    let request_line = text.lines().next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or("HTTP/1.1");

    if method.eq_ignore_ascii_case("CONNECT") {
        if !allowlist.allows_authority(target) {
            return deny(&mut client, target);
        }
        let Ok(upstream) = connect(target) else {
            client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        };
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
        return tunnel(client, upstream);
    }

    // Absolute-form plain HTTP: http://host[:port]/path
    let Some(rest) = target.strip_prefix("http://") else {
        return deny(&mut client, "non-proxy request");
    };
    let (authority, path) = rest
        .find('/')
        .map_or((rest, "/"), |index| (&rest[..index], &rest[index..]));
    let with_port = if authority.contains(':') && !authority.ends_with(']') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    if !allowlist.allows_authority(&with_port) {
        return deny(&mut client, authority);
    }
    let Ok(mut upstream) = connect(&with_port) else {
        client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    let rewritten = text.replacen(&request_line, &format!("{method} {path} {version}"), 1);
    upstream.write_all(rewritten.as_bytes())?;
    tunnel(client, upstream)
}

fn tunnel(client: TcpStream, upstream: TcpStream) -> io::Result<()> {
    let mut client_read = client.try_clone()?;
    let mut upstream_write = upstream.try_clone()?;
    let forward = thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut upstream_write);
        let _ = upstream_write.shutdown(Shutdown::Write);
    });
    let mut upstream_read = upstream;
    let mut client_write = client;
    let _ = io::copy(&mut upstream_read, &mut client_write);
    let _ = client_write.shutdown(Shutdown::Write);
    let _ = forward.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AllowlistProxy;
    use crate::net_allow::NetworkAllowlist;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn echo_server() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut buf = [0_u8; 64];
                let n = stream.read(&mut buf).unwrap_or(0);
                let _ = stream.write_all(&buf[..n]);
            }
        });
        port
    }

    fn read_until_blank_line(stream: &mut TcpStream) -> String {
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
            head.push(byte[0]);
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    #[test]
    fn connect_to_an_allowed_host_tunnels_bytes() {
        let port = echo_server();
        let proxy = AllowlistProxy::start(NetworkAllowlist::new(&["127.0.0.1"])).expect("proxy");
        let mut client = TcpStream::connect(proxy.addr()).expect("connect proxy");
        write!(
            client,
            "CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: x\r\n\r\n"
        )
        .expect("write");
        let head = read_until_blank_line(&mut client);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        client.write_all(b"ping").expect("write");
        let mut buf = [0_u8; 4];
        client.read_exact(&mut buf).expect("echo");
        assert_eq!(&buf, b"ping");
    }

    #[test]
    fn hosts_off_the_list_get_403() {
        let proxy = AllowlistProxy::start(NetworkAllowlist::new(&["github.com"])).expect("proxy");
        for request in [
            "CONNECT evil.example:443 HTTP/1.1\r\n\r\n",
            "GET http://evil.example/x HTTP/1.1\r\nHost: evil.example\r\n\r\n",
            "GET /relative HTTP/1.1\r\n\r\n",
        ] {
            let mut client = TcpStream::connect(proxy.addr()).expect("connect proxy");
            client.write_all(request.as_bytes()).expect("write");
            let mut response = String::new();
            let _ = client.read_to_string(&mut response);
            assert!(
                response.starts_with("HTTP/1.1 403"),
                "{request}: {response}"
            );
            assert!(response.contains("allowlist"));
        }
    }

    #[test]
    fn the_allowlist_can_be_replaced_while_running() {
        let port = echo_server();
        let proxy = AllowlistProxy::start(NetworkAllowlist::default()).expect("proxy");
        proxy.set_allowlist(NetworkAllowlist::new(&[format!("127.0.0.1:{port}")]));
        let mut client = TcpStream::connect(proxy.addr()).expect("connect proxy");
        write!(client, "CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n").expect("write");
        assert!(read_until_blank_line(&mut client).starts_with("HTTP/1.1 200"));
    }
}
