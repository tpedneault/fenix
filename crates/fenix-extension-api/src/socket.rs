//! Sockets, to the hosts and ports its manifest names
//! (`permissions.sockets`): TCP connections, with TLS if it likes, and UDP
//! sockets; and local ones -- Unix sockets, named pipes -- by the names it
//! lists (`permissions.ipc`).
//!
//! ```toml
//! [permissions]
//! sockets = ["*:5900-5999", "${setting:vnc.hosts}", "udp://:5000"]
//! ipc = ["discord-ipc-*"]
//! ```
//!
//! Nothing waits: [`connect`] returns at once, and
//! [`crate::Event::SocketConnected`] says when it's made;
//! what comes is [`crate::Event::SocketData`], in order, and its end
//! [`crate::Event::SocketClosed`]. A protocol reads what came with an
//! [`Inbox`], a message at a time.

use crate::{host, ClientCert, TlsOptions};

/// A port on this machine's loopback, 127.0.0.1, listened on: one its
/// manifest lets it take (`permissions.listen`), or 0 for any free one.
/// Its id, and the port it got: each connection made to it comes to
/// [`crate::Extension::socket_accepted`] as a socket of its own;
/// [`close`] on the listener stops it.
///
/// ```toml
/// [permissions]
/// listen = ["127.0.0.1:0"]
/// ```
pub fn listen(port: u16) -> Result<(u64, u16), String> {
    host::tcp_listen(port)
}

/// A TCP connection to `host`:`port`; its id comes back at once.
pub fn connect(host: &str, port: u16) -> Result<u64, String> {
    host::socket_connect(host, port)
}

/// [`connect`] with TLS: connected once the handshake's done.
pub fn connect_tls(host: &str, port: u16, tls: &TlsOptions) -> Result<u64, String> {
    host::socket_connect_tls(host, port, tls)
}

/// TLS from here on, on connected TCP socket `socket`: STARTTLS, once the
/// server said yes. [`crate::Event::SocketConnected`] again when it's
/// done.
pub fn start_tls(socket: u64, tls: &TlsOptions) -> Result<(), String> {
    host::socket_start_tls(socket, tls)
}

/// A local socket by name (`discord-ipc-0`): a named pipe on Windows, a
/// Unix socket elsewhere. A socket as a TCP one is, from then on.
pub fn ipc(name: &str) -> Result<u64, String> {
    host::ipc_connect(name)
}

impl Default for TlsOptions {
    /// The host's name, checked against the system's certificates.
    fn default() -> TlsOptions {
        TlsOptions { server_name: None, pinned: Vec::new(), accept_invalid: false, client_cert: None }
    }
}

impl TlsOptions {
    pub fn new() -> TlsOptions {
        TlsOptions::default()
    }

    /// The name its certificate is for, when it isn't the host's.
    pub fn server_name(mut self, name: &str) -> TlsOptions {
        self.server_name = Some(name.into());
        self
    }

    /// A certificate taken as it is (PEM): a server's own, self-signed.
    pub fn pin(mut self, pem: &str) -> TlsOptions {
        self.pinned.push(pem.into());
        self
    }

    /// Any certificate taken: nothing is checked.
    pub fn accept_invalid(mut self) -> TlsOptions {
        self.accept_invalid = true;
        self
    }

    /// A client certificate and its key, PEM.
    pub fn client_cert(mut self, cert: &str, key: &str) -> TlsOptions {
        self.client_cert = Some(ClientCert { cert: cert.into(), key: key.into() });
        self
    }
}

/// Bytes for TCP socket `socket`, sent in order.
pub fn write(socket: u64, data: &[u8]) -> Result<(), String> {
    host::socket_write(socket, data)
}

/// A UDP socket on this machine's `port` (an entry `:port` in its
/// manifest lets it), or on any free one with 0, to send from: datagrams
/// come as [`crate::Event::Datagram`].
pub fn bind_udp(port: u16) -> Result<u64, String> {
    host::udp_bind(port)
}

/// A datagram from UDP socket `socket` to `host`:`port`.
pub fn send_to(socket: u64, host: &str, port: u16, data: &[u8]) -> Result<(), String> {
    host::udp_send(socket, host, port, data)
}

/// Closes it: nothing more comes from it.
pub fn close(socket: u64) {
    host::socket_close(socket)
}

/// What came on a stream and hasn't been read yet: a protocol takes it a
/// message at a time, as much as has come.
#[derive(Debug, Default, Clone)]
pub struct Inbox {
    bytes: Vec<u8>,
    read: usize,
}

impl Inbox {
    pub fn new() -> Inbox {
        Inbox::default()
    }

    /// What came, after what came before.
    pub fn push(&mut self, data: &[u8]) {
        if self.read > 0 && self.read >= self.bytes.len() / 2 {
            self.bytes.drain(..self.read);
            self.read = 0;
        }
        self.bytes.extend_from_slice(data);
    }

    /// What's there to read.
    pub fn available(&self) -> &[u8] {
        &self.bytes[self.read..]
    }

    pub fn len(&self) -> usize {
        self.bytes.len() - self.read
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The next `n` bytes, once they've all come.
    pub fn take(&mut self, n: usize) -> Option<Vec<u8>> {
        (self.len() >= n).then(|| {
            let out = self.bytes[self.read..self.read + n].to_vec();
            self.read += n;
            out
        })
    }

    /// The next `n` bytes, left there: a header looked at before the
    /// message it heads has all come.
    pub fn peek(&self, n: usize) -> Option<&[u8]> {
        (self.len() >= n).then(|| &self.bytes[self.read..self.read + n])
    }

    /// Up to and with the next `\n`, without it (or `\r\n`), once it's come.
    pub fn take_line(&mut self) -> Option<String> {
        let at = self.available().iter().position(|b| *b == b'\n')?;
        let line = self.take(at + 1)?;
        Some(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inbox_gives_messages_as_they_come_whole() {
        let mut inbox = Inbox::new();
        inbox.push(b"RFB 003.008\nPI");
        assert_eq!(inbox.take_line().as_deref(), Some("RFB 003.008"));
        assert_eq!(inbox.take_line(), None, "half a line");
        assert_eq!(inbox.peek(2), Some(&b"PI"[..]));
        inbox.push(b"NG\r\n\x00\x01");
        assert_eq!(inbox.take_line().as_deref(), Some("PING"));
        assert_eq!(inbox.take(3), None);
        assert_eq!(inbox.take(2), Some(vec![0, 1]));
        assert!(inbox.is_empty());
    }
}
