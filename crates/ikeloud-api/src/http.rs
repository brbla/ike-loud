//! HTTPS transport: the single wrapper of `ureq`.
//!
//! The rest of the crate talks to [`Transport`], so the sign-in and Drive
//! logic is tested against a fake server without a network. The real
//! transport ([`HttpsTransport`]) refuses every host outside a fixed list
//! (spec: fixed list of domains), never follows redirects and never sends
//! cookies on its own: cookies are kept by [`crate::cookies::CookieJar`].

use std::io::Read;
use std::time::Duration;

/// HTTP method used by the iCloud web services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET
    Get,
    /// POST
    Post,
    /// PUT
    Put,
}

/// One request.
#[derive(Debug)]
pub struct Request<'a> {
    /// Method.
    pub method: Method,
    /// Full `https://` URL.
    pub url: String,
    /// Headers in the order they are sent.
    pub headers: Vec<(String, String)>,
    /// Body.
    pub body: Body<'a>,
}

/// Body of a request.
pub enum Body<'a> {
    /// No body.
    Empty,
    /// Small body in memory (JSON).
    Bytes(Vec<u8>),
    /// File content: read while sending, `len` bytes exactly (sent as
    /// `Content-Length`), so an upload need not fit in memory.
    Stream {
        /// The content.
        reader: &'a mut dyn Read,
        /// Its length.
        len: u64,
    },
}

impl std::fmt::Debug for Body<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "Empty"),
            Self::Bytes(b) => write!(f, "Bytes({} B)", b.len()),
            Self::Stream { len, .. } => write!(f, "Stream({len} B)"),
        }
    }
}

impl Body<'_> {
    /// The whole body in memory (for tests and the fake server).
    pub fn into_bytes(self) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Empty => Ok(Vec::new()),
            Self::Bytes(b) => Ok(b),
            Self::Stream { reader, len } => {
                let mut out = Vec::new();
                reader.read_to_end(&mut out)?;
                if u64::try_from(out.len()).unwrap_or(u64::MAX) != len {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "stream length differs from the declared one",
                    ));
                }
                Ok(out)
            }
        }
    }
}

/// One response. The body is a stream so a download need not fit in memory.
pub struct Response {
    /// Status code.
    pub status: u16,
    /// Headers; a name can repeat (`Set-Cookie`).
    pub headers: Vec<(String, String)>,
    /// Body.
    pub body: Box<dyn Read + Send>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Headers can carry session tokens; only their names are shown.
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &names)
            .finish_non_exhaustive()
    }
}

impl Response {
    /// First header of the given name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// All headers of the given name (case-insensitive).
    pub fn headers_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Reads the whole body, at most `limit` bytes.
    pub fn read_body(&mut self, limit: u64) -> Result<Vec<u8>, TransportError> {
        let mut out = Vec::new();
        (&mut self.body)
            .take(limit.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        if u64::try_from(out.len()).unwrap_or(u64::MAX) > limit {
            return Err(TransportError::TooLarge);
        }
        Ok(out)
    }
}

/// Transport failure: the request did not produce an HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The host is not on the list of allowed domains, or the URL is not https.
    HostNotAllowed(String),
    /// Network, TLS or timeout error.
    Io(String),
    /// The body was larger than the caller allowed.
    TooLarge,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostNotAllowed(host) => write!(f, "host not allowed: {host}"),
            Self::Io(e) => write!(f, "network error: {e}"),
            Self::TooLarge => write!(f, "response too large"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Something that sends a request and returns a response.
pub trait Transport {
    /// Sends the request.
    fn send(&self, request: Request<'_>) -> Result<Response, TransportError>;
}

/// Hosts the client may talk to: Apple sign-in, the iCloud web services
/// (`setup.icloud.com`, `p123-drivews.icloud.com`, …) and the download
/// servers (`cvws.icloud-content.com`).
pub fn host_allowed(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "idmsa.apple.com"
        || subdomain_of(&host, "icloud.com")
        || subdomain_of(&host, "icloud-content.com")
}

fn subdomain_of(host: &str, domain: &str) -> bool {
    host.strip_suffix(domain)
        .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1)
}

/// Host of an `https://` URL, without port or user info; `None` for any
/// other scheme.
pub fn https_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

/// The real transport over HTTPS.
pub struct HttpsTransport {
    agent: ureq::Agent,
}

impl std::fmt::Debug for HttpsTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpsTransport").finish_non_exhaustive()
    }
}

impl Default for HttpsTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpsTransport {
    /// New transport. Timeouts limit connecting and waiting for the response
    /// head; a long download is not cut off.
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .build()
            .new_agent();
        Self { agent }
    }
}

impl Transport for HttpsTransport {
    fn send(&self, request: Request<'_>) -> Result<Response, TransportError> {
        match https_host(&request.url) {
            Some(host) if host_allowed(host) => {}
            _ => return Err(TransportError::HostNotAllowed(request.url)),
        }
        let method = match request.method {
            Method::Get => ureq::http::Method::GET,
            Method::Post => ureq::http::Method::POST,
            Method::Put => ureq::http::Method::PUT,
        };
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(request.url.as_str());
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let result = match request.body {
            Body::Bytes(body) => builder
                .body(body)
                .map_err(|e| TransportError::Io(e.to_string()))
                .and_then(|r| self.agent.run(r).map_err(io_error)),
            Body::Empty => builder
                .body(())
                .map_err(|e| TransportError::Io(e.to_string()))
                .and_then(|r| self.agent.run(r).map_err(io_error)),
            Body::Stream { reader, len } => builder
                .header("Content-Length", len.to_string())
                .body(ureq::SendBody::from_reader(reader))
                .map_err(|e| TransportError::Io(e.to_string()))
                .and_then(|r| self.agent.run(r).map_err(io_error)),
        }?;
        let status = result.status().as_u16();
        let headers = result
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let body: Box<dyn Read + Send> = Box::new(result.into_body().into_reader());
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

fn io_error(e: ureq::Error) -> TransportError {
    TransportError::Io(e.to_string())
}
