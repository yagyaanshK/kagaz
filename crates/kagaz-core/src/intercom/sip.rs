//! SIP signalling, hand-rolled: enough to register to the server and, next,
//! to receive the doorbell's call. SIP is a line-based text protocol much
//! like HTTP; over TCP each message is framed by its `Content-Length`.

use super::{Error, SipAccount};
use md5::{Digest, Md5};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::time::Duration;

/// A parsed SIP message (a request or a response).
#[derive(Debug, Clone)]
pub struct Message {
    /// The first line, e.g. `REGISTER sip:... SIP/2.0` or `SIP/2.0 200 OK`.
    pub start: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Message {
    /// The status code, when this is a response.
    pub fn status(&self) -> Option<u16> {
        let mut parts = self.start.split_whitespace();
        if parts.next()?.starts_with("SIP/") {
            return parts.next()?.parse().ok();
        }
        None
    }

    /// The reason phrase after the status code.
    pub fn reason(&self) -> String {
        self.start.splitn(3, ' ').nth(2).unwrap_or("").to_string()
    }

    /// The first header with this name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Every header with this name, in order (Via can repeat).
    pub fn headers_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The request method (REGISTER, INVITE, ACK, BYE, ...), when this is a request.
    pub fn method(&self) -> Option<String> {
        let first = self.start.split_whitespace().next()?;
        (!first.starts_with("SIP/")).then(|| first.to_uppercase())
    }

    /// Build a response to this request: echo its Via, From, To, Call-ID and
    /// CSeq, add `to_tag` to the To header (what a 18x/2xx needs), an optional
    /// Contact, and an optional SDP body.
    pub fn response_to(
        &self,
        code: u16,
        reason: &str,
        to_tag: &str,
        contact: Option<&str>,
        sdp: Option<&str>,
    ) -> String {
        let mut lines = vec![format!("SIP/2.0 {code} {reason}")];
        for via in self.headers_named("Via") {
            lines.push(format!("Via: {via}"));
        }
        if let Some(from) = self.header("From") {
            lines.push(format!("From: {from}"));
        }
        if let Some(to) = self.header("To") {
            if to.contains("tag=") || to_tag.is_empty() {
                lines.push(format!("To: {to}"));
            } else {
                lines.push(format!("To: {to};tag={to_tag}"));
            }
        }
        if let Some(cid) = self.header("Call-ID") {
            lines.push(format!("Call-ID: {cid}"));
        }
        if let Some(cseq) = self.header("CSeq") {
            lines.push(format!("CSeq: {cseq}"));
        }
        if let Some(contact) = contact {
            lines.push(format!("Contact: <{contact}>"));
        }
        lines.push("User-Agent: Kagaz".to_string());
        let body = sdp.unwrap_or("");
        if sdp.is_some() {
            lines.push("Content-Type: application/sdp".to_string());
        }
        lines.push(format!("Content-Length: {}", body.len()));
        format!("{}\r\n\r\n{}", lines.join("\r\n"), body)
    }
}

/// A live connection to the SIP server, over TCP or UDP. It carries the
/// local address the OS chose, which goes into Via and Contact.
pub enum Connection {
    Tcp {
        reader: BufReader<TcpStream>,
        writer: TcpStream,
    },
    Udp(UdpSocket),
}

impl Connection {
    /// Open a connection to the account's server.
    pub fn open(account: &SipAccount, timeout: Duration) -> Result<Connection, Error> {
        let addr = resolve(&account.server, account.port)?;
        if account.over_tcp() {
            let stream = TcpStream::connect_timeout(&addr, timeout)
                .map_err(|e| Error::Transport(e.to_string()))?;
            stream
                .set_read_timeout(Some(timeout))
                .map_err(|e| Error::Transport(e.to_string()))?;
            let reader = BufReader::new(
                stream
                    .try_clone()
                    .map_err(|e| Error::Transport(e.to_string()))?,
            );
            Ok(Connection::Tcp {
                reader,
                writer: stream,
            })
        } else {
            let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::Transport(e.to_string()))?;
            sock.connect(addr)
                .map_err(|e| Error::Transport(e.to_string()))?;
            sock.set_read_timeout(Some(timeout))
                .map_err(|e| Error::Transport(e.to_string()))?;
            Ok(Connection::Udp(sock))
        }
    }

    /// The local `ip:port` this connection speaks from.
    pub fn local(&self) -> Result<String, Error> {
        let addr = match self {
            Connection::Tcp { writer, .. } => writer.local_addr(),
            Connection::Udp(sock) => sock.local_addr(),
        }
        .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(addr.to_string())
    }

    pub fn send(&mut self, raw: &str) -> Result<(), Error> {
        match self {
            Connection::Tcp { writer, .. } => writer
                .write_all(raw.as_bytes())
                .and_then(|()| writer.flush())
                .map_err(|e| Error::Transport(e.to_string())),
            Connection::Udp(sock) => sock
                .send(raw.as_bytes())
                .map(|_| ())
                .map_err(|e| Error::Transport(e.to_string())),
        }
    }

    /// Read one whole message (blocks up to the read timeout).
    pub fn recv(&mut self) -> Result<Message, Error> {
        match self {
            Connection::Tcp { reader, .. } => read_tcp_message(reader),
            Connection::Udp(sock) => {
                let mut buf = vec![0u8; 65535];
                let n = sock
                    .recv(&mut buf)
                    .map_err(|e| Error::Transport(e.to_string()))?;
                parse(&String::from_utf8_lossy(&buf[..n]))
            }
        }
    }
}

fn resolve(host: &str, port: u16) -> Result<std::net::SocketAddr, Error> {
    use std::net::ToSocketAddrs;
    (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::Transport(e.to_string()))?
        .next()
        .ok_or_else(|| Error::Transport(format!("no address for {host}")))
}

/// Read headers until the blank line, then the body of `Content-Length`.
fn read_tcp_message(reader: &mut BufReader<TcpStream>) -> Result<Message, Error> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| Error::Transport(e.to_string()))?;
        if n == 0 {
            return Err(Error::Transport("the server closed the connection".into()));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
    }
    let mut msg = parse(&head)?;
    let len: usize = msg
        .header("Content-Length")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    if len > 0 {
        let mut body = vec![0u8; len];
        reader
            .read_exact(&mut body)
            .map_err(|e| Error::Transport(e.to_string()))?;
        msg.body = String::from_utf8_lossy(&body).into_owned();
    }
    Ok(msg)
}

/// Parse a message's start line and headers (body filled in separately for TCP).
pub fn parse(text: &str) -> Result<Message, Error> {
    let mut lines = text.split("\r\n").flat_map(|l| l.split('\n'));
    let start = lines
        .next()
        .ok_or_else(|| Error::Malformed("empty message".into()))?
        .trim_end()
        .to_string();
    let mut headers = Vec::new();
    let mut body = String::new();
    let mut in_body = false;
    for line in lines {
        if in_body {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        } else if line.is_empty() {
            in_body = true;
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(Message {
        start,
        headers,
        body,
    })
}

fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// One field out of a `Digest ...` challenge (`realm`, `nonce`, `qop`, ...).
fn auth_param(challenge: &str, key: &str) -> Option<String> {
    let at = challenge.find(&format!("{key}="))?;
    let rest = &challenge[at + key.len() + 1..];
    if let Some(stripped) = rest.strip_prefix('"') {
        stripped.split('"').next().map(str::to_string)
    } else {
        Some(rest.split([',', ' ']).next().unwrap_or("").to_string())
    }
}

/// The `Authorization` value answering a `WWW-Authenticate`/`Proxy-Authenticate`
/// challenge, by HTTP digest (MD5), with qop=auth when the server asks for it.
fn digest_authorization(account: &SipAccount, method: &str, uri: &str, challenge: &str) -> String {
    let realm = auth_param(challenge, "realm").unwrap_or_default();
    let nonce = auth_param(challenge, "nonce").unwrap_or_default();
    let opaque = auth_param(challenge, "opaque");
    let qop = auth_param(challenge, "qop");
    let ha1 = md5_hex(&format!(
        "{}:{realm}:{}",
        account.username, account.password
    ));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let mut fields = vec![
        format!("username=\"{}\"", account.username),
        format!("realm=\"{realm}\""),
        format!("nonce=\"{nonce}\""),
        format!("uri=\"{uri}\""),
        "algorithm=MD5".to_string(),
    ];
    let response = if let Some(qop) = qop.filter(|q| q.split(',').any(|o| o.trim() == "auth")) {
        let cnonce = token();
        let nc = "00000001";
        let resp = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
        fields.push(format!("qop={qop}"));
        fields.push(format!("nc={nc}"));
        fields.push(format!("cnonce=\"{cnonce}\""));
        resp
    } else {
        md5_hex(&format!("{ha1}:{nonce}:{ha2}"))
    };
    fields.push(format!("response=\"{response}\""));
    if let Some(opaque) = opaque {
        fields.push(format!("opaque=\"{opaque}\""));
    }
    format!("Digest {}", fields.join(", "))
}

/// The outcome of registering.
#[derive(Debug, Clone)]
pub struct Registered {
    /// Seconds the server granted (it may shorten what we asked for).
    pub expires: u32,
    /// The connection, still open — the server reuses it to reach us for
    /// the doorbell's call (important behind NAT, as we are here).
    pub local: String,
}

/// Dialog-level identifiers kept across the REGISTER's two requests.
struct Registrar {
    call_id: String,
    from_tag: String,
    cseq: u32,
}

/// Register the account to its server. Sends REGISTER, answers the auth
/// challenge, and returns once the server says 200 OK. The connection is
/// returned open so the caller can keep it (TCP calls arrive on it).
pub fn register(account: &SipAccount, conn: &mut Connection) -> Result<Registered, Error> {
    let local = conn.local()?;
    let mut reg = Registrar {
        call_id: token(),
        from_tag: token(),
        cseq: 1,
    };
    let first = register_request(account, &local, &reg, None);
    conn.send(&first)?;
    let mut response = conn.recv()?;
    // Skip the provisional 100 Trying, if any.
    while response.status() == Some(100) {
        response = conn.recv()?;
    }
    if matches!(response.status(), Some(401) | Some(407)) {
        let challenge = response
            .header("WWW-Authenticate")
            .or_else(|| response.header("Proxy-Authenticate"))
            .ok_or_else(|| Error::Malformed("auth required but no challenge".into()))?
            .to_string();
        reg.cseq += 1;
        let auth = digest_authorization(account, "REGISTER", &account.aor(), &challenge);
        let retry = register_request(account, &local, &reg, Some(&auth));
        conn.send(&retry)?;
        response = conn.recv()?;
        while response.status() == Some(100) {
            response = conn.recv()?;
        }
    }
    match response.status() {
        Some(200) => {
            let expires = response
                .header("Expires")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(account.expires);
            Ok(Registered { expires, local })
        }
        Some(code) => Err(Error::Rejected(format!("{code} {}", response.reason()))),
        None => Err(Error::Malformed(format!(
            "expected a response, got {:?}",
            response.start
        ))),
    }
}

fn register_request(
    account: &SipAccount,
    local: &str,
    reg: &Registrar,
    auth: Option<&str>,
) -> String {
    let proto = if account.over_tcp() { "TCP" } else { "UDP" };
    let branch = format!("z9hG4bK{}", token());
    let display = if account.display_name.is_empty() {
        account.username.clone()
    } else {
        account.display_name.clone()
    };
    let mut lines = vec![
        format!("REGISTER sip:{} SIP/2.0", server_uri(account)),
        format!("Via: SIP/2.0/{proto} {local};branch={branch};rport"),
        "Max-Forwards: 70".to_string(),
        format!(
            "From: \"{display}\" <{}>;tag={}",
            account.aor(),
            reg.from_tag
        ),
        format!("To: <{}>", account.aor()),
        format!("Call-ID: {}", reg.call_id),
        format!("CSeq: {} REGISTER", reg.cseq),
        format!(
            "Contact: <sip:{}@{local};transport={}>",
            account.username,
            proto.to_lowercase()
        ),
        format!("Expires: {}", account.expires),
        "User-Agent: Kagaz".to_string(),
    ];
    if let Some(auth) = auth {
        lines.push(format!("Authorization: {auth}"));
    }
    lines.push("Content-Length: 0".to_string());
    format!("{}\r\n\r\n", lines.join("\r\n"))
}

fn server_uri(account: &SipAccount) -> String {
    if account.port == 5060 {
        account.server.clone()
    } else {
        format!("{}:{}", account.server, account.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> SipAccount {
        SipAccount {
            username: "20011001".into(),
            password: "secret".into(),
            display_name: "Door".into(),
            server: "203.0.113.10".into(),
            port: 5070,
            transport: "tcp".into(),
            expires: 1800,
        }
    }

    #[test]
    fn parses_a_response_with_headers() {
        let m = parse("SIP/2.0 401 Unauthorized\r\nCSeq: 1 REGISTER\r\nWWW-Authenticate: Digest realm=\"example\", nonce=\"abc123\"\r\n\r\n").unwrap();
        assert_eq!(m.status(), Some(401));
        assert_eq!(m.reason(), "Unauthorized");
        assert_eq!(m.header("cseq"), Some("1 REGISTER"));
        assert!(m.header("WWW-Authenticate").unwrap().contains("nonce"));
    }

    #[test]
    fn reads_digest_challenge_fields() {
        let c = "Digest realm=\"example\", nonce=\"abc123\", qop=\"auth\", opaque=\"xyz\"";
        assert_eq!(auth_param(c, "realm").as_deref(), Some("example"));
        assert_eq!(auth_param(c, "nonce").as_deref(), Some("abc123"));
        assert_eq!(auth_param(c, "qop").as_deref(), Some("auth"));
        assert_eq!(auth_param(c, "opaque").as_deref(), Some("xyz"));
    }

    #[test]
    fn digest_response_matches_the_rfc_shape() {
        // HA1=md5(user:realm:pass), HA2=md5(METHOD:uri), no qop.
        let acct = account();
        let challenge = "Digest realm=\"example\", nonce=\"abc123\"";
        let auth = digest_authorization(&acct, "REGISTER", &acct.aor(), challenge);
        let ha1 = md5_hex("20011001:example:secret");
        let ha2 = md5_hex(&format!("REGISTER:{}", acct.aor()));
        let want = md5_hex(&format!("{ha1}:abc123:{ha2}"));
        assert!(auth.contains(&format!("response=\"{want}\"")));
        assert!(auth.contains("algorithm=MD5"));
    }

    #[test]
    fn qop_auth_uses_cnonce_and_nc() {
        let acct = account();
        let challenge = "Digest realm=\"r\", nonce=\"n\", qop=\"auth\"";
        let auth = digest_authorization(&acct, "REGISTER", &acct.aor(), challenge);
        assert!(auth.contains("qop=auth"));
        assert!(auth.contains("nc=00000001"));
        assert!(auth.contains("cnonce="));
    }

    #[test]
    fn register_request_has_the_core_headers() {
        let acct = account();
        let reg = Registrar {
            call_id: "cid".into(),
            from_tag: "ft".into(),
            cseq: 1,
        };
        let req = register_request(&acct, "192.0.2.6:5062", &reg, None);
        assert!(req.starts_with("REGISTER sip:203.0.113.10:5070 SIP/2.0\r\n"));
        assert!(req.contains("Via: SIP/2.0/TCP 192.0.2.6:5062;branch=z9hG4bK"));
        assert!(req.contains("From: \"Door\" <sip:20011001@203.0.113.10>;tag=ft"));
        assert!(req.contains("CSeq: 1 REGISTER"));
        assert!(req.contains("Contact: <sip:20011001@192.0.2.6:5062;transport=tcp>"));
        assert!(req.ends_with("\r\n\r\n"));
    }
}
