use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;

use http_body_util::{BodyExt, Full};

/// Configuration for the kubectl proxy server
pub struct ProxyConfig {
    /// Address to bind to (default: 127.0.0.1)
    pub address: String,
    /// Port to bind to (default: 8001, 0 = random)
    pub port: u16,
    /// API server URL to proxy to
    pub api_server: String,
    /// Bearer token for authentication
    pub token: Option<String>,
    /// Skip TLS verification when connecting to API server
    pub skip_tls_verify: bool,
}

/// `DefaultHostAcceptRE` (kubectl/pkg/proxy/proxy_server.go:39).
pub const DEFAULT_HOST_ACCEPT_RE: &str = "^localhost$,^127\\.0\\.0\\.1$,^\\[::1\\]$";
/// `DefaultPathAcceptRE` (proxy_server.go:41).
pub const DEFAULT_PATH_ACCEPT_RE: &str = "^.*";
/// `DefaultPathRejectRE` (proxy_server.go:43).
pub const DEFAULT_PATH_REJECT_RE: &str = "^/api/.*/pods/.*/exec,^/api/.*/pods/.*/attach";
/// `DefaultMethodRejectRE` (proxy_server.go:45).
pub const DEFAULT_METHOD_REJECT_RE: &str = "^$";

/// `MakeRegexpArray` (proxy_server.go:63): comma separated regexps; empty
/// string is no regexps.
pub fn make_regexp_array(s: &str) -> Result<Vec<regex::Regex>, regex::Error> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(',').map(regex::Regex::new).collect()
}

/// `matchesRegexp` (proxy_server.go:88).
pub fn matches_regexp(s: &str, regexps: &[regex::Regex]) -> bool {
    regexps.iter().any(|re| re.is_match(s))
}

/// `extractHost` (proxy_server.go:120): the host part of a `Host` header.
pub fn extract_host(header: &str) -> String {
    // net.SplitHostPort; on error the header is the host.
    if let Some(rest) = header.strip_prefix('[') {
        if let Some((h, port)) = rest.split_once("]:") {
            if !port.contains(':') {
                return h.to_string();
            }
        }
        return header.to_string();
    }
    match header.rsplit_once(':') {
        Some((h, _)) if !h.contains(':') => h.to_string(),
        _ => header.to_string(),
    }
}

/// `FilterServer` (proxy_server.go:50-110): accept/reject requests by path,
/// method and host before they are proxied.
#[derive(Debug, Clone)]
pub struct FilterServer {
    pub accept_paths: Vec<regex::Regex>,
    pub reject_paths: Vec<regex::Regex>,
    pub accept_hosts: Vec<regex::Regex>,
    pub reject_methods: Vec<regex::Regex>,
}

impl FilterServer {
    /// `NewFilter` fields from the `--accept-paths`, `--reject-paths`,
    /// `--accept-hosts` and `--reject-methods` strings.
    pub fn new(
        accept_paths: &str,
        reject_paths: &str,
        accept_hosts: &str,
        reject_methods: &str,
    ) -> Result<Self, regex::Error> {
        Ok(Self {
            accept_paths: make_regexp_array(accept_paths)?,
            reject_paths: make_regexp_array(reject_paths)?,
            accept_hosts: make_regexp_array(accept_hosts)?,
            reject_methods: make_regexp_array(reject_methods)?,
        })
    }

    /// The filter built from the flag defaults.
    pub fn with_defaults() -> Self {
        Self::new(
            DEFAULT_PATH_ACCEPT_RE,
            DEFAULT_PATH_REJECT_RE,
            DEFAULT_HOST_ACCEPT_RE,
            DEFAULT_METHOD_REJECT_RE,
        )
        .expect("default filter regexps compile")
    }

    /// `FilterServer.accept` (proxy_server.go:98).
    pub fn accept(&self, method: &str, path: &str, host: &str) -> bool {
        if matches_regexp(path, &self.reject_paths) {
            return false;
        }
        if matches_regexp(method, &self.reject_methods) {
            return false;
        }
        matches_regexp(path, &self.accept_paths) && matches_regexp(host, &self.accept_hosts)
    }
}

struct ProxyState {
    filter: Option<FilterServer>,
    api_server: String,
    token: Option<String>,
    client: reqwest::Client,
}

/// Execute the proxy command: bind, print the address, and serve forever.
pub async fn execute(config: ProxyConfig) -> Result<()> {
    execute_with_filter(config, Some(FilterServer::with_defaults())).await
}

/// [`execute`] with an explicit filter; `None` is `--disable-filter`.
pub async fn execute_with_filter(config: ProxyConfig, filter: Option<FilterServer>) -> Result<()> {
    let listener = bind_listener(&config).await?;
    let addr = listener.local_addr()?;
    println!("Starting to serve on {}", addr);

    serve(listener, config, filter).await
}

/// Bind a TCP listener according to the config. Exposed for testing.
pub async fn bind_listener(config: &ProxyConfig) -> Result<TcpListener> {
    let addr: SocketAddr = format!("{}:{}", config.address, config.port)
        .parse()
        .context("Invalid bind address")?;
    TcpListener::bind(addr)
        .await
        .context("Failed to bind proxy listener")
}

/// Accept connections on the listener and proxy them to the API server.
pub async fn serve(
    listener: TcpListener,
    config: ProxyConfig,
    filter: Option<FilterServer>,
) -> Result<()> {
    let client = if config.skip_tls_verify {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .context("Failed to build HTTP client")?
    } else {
        reqwest::Client::new()
    };

    let state = Arc::new(ProxyState {
        filter,
        api_server: config.api_server.trim_end_matches('/').to_string(),
        token: config.token,
        client,
    });

    loop {
        let (stream, _remote_addr) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let state = Arc::clone(&state);

        tokio::spawn(async move {
            let service = service_fn(move |req: Request<Incoming>| {
                let state = Arc::clone(&state);
                async move { handle_request(req, &state).await }
            });

            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                eprintln!("proxy connection error: {}", err);
            }
        });
    }
}

async fn handle_request(
    req: Request<Incoming>,
    state: &ProxyState,
) -> Result<Response<Full<bytes::Bytes>>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    // `FilterServer.ServeHTTP` (proxy_server.go:128): 403 unless accepted.
    if let Some(filter) = &state.filter {
        let host_header = req
            .headers()
            .get(hyper::header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| uri.authority().map(|a| a.to_string()))
            .unwrap_or_default();
        let host = extract_host(&host_header);
        if !filter.accept(method.as_str(), uri.path(), &host) {
            return Ok(Response::builder()
                .status(403)
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(Full::new(bytes::Bytes::from("Forbidden\n")))
                .unwrap());
        }
    }
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    let target_url = format!("{}{}", state.api_server, path_and_query);

    // Build the outgoing request
    let mut builder = state.client.request(method, &target_url);

    // Copy headers from the incoming request (skip host)
    for (key, value) in req.headers() {
        if key != hyper::header::HOST {
            builder = builder.header(key.clone(), value.clone());
        }
    }

    // Add authentication
    if let Some(ref token) = state.token {
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }

    // Read the request body
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            let error_body = format!("Failed to read request body: {}", e);
            let resp = Response::builder()
                .status(502)
                .body(Full::new(bytes::Bytes::from(error_body)))
                .unwrap();
            return Ok(resp);
        }
    };

    if !body_bytes.is_empty() {
        builder = builder.body(body_bytes.to_vec());
    }

    // Send the request to the API server
    let upstream_response = match builder.send().await {
        Ok(resp) => resp,
        Err(e) => {
            let error_body = format!("Failed to proxy request to {}: {}", target_url, e);
            let resp = Response::builder()
                .status(502)
                .body(Full::new(bytes::Bytes::from(error_body)))
                .unwrap();
            return Ok(resp);
        }
    };

    // Build the response back to the client
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let response_bytes = match upstream_response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            let error_body = format!("Failed to read upstream response: {}", e);
            let resp = Response::builder()
                .status(502)
                .body(Full::new(bytes::Bytes::from(error_body)))
                .unwrap();
            return Ok(resp);
        }
    };

    let mut response_builder = Response::builder().status(status);
    for (key, value) in headers.iter() {
        response_builder = response_builder.header(key, value);
    }

    let response = response_builder.body(Full::new(response_bytes)).unwrap();

    Ok(response)
}
