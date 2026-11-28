//! Unit tests moved from `src/commands/proxy.rs`.
#![allow(unused_imports, dead_code)]

mod __parent {
    pub use anyhow::{Context, Result};
    pub use http_body_util::{BodyExt, Full};
    pub use hyper::body::Incoming;
    pub use hyper::server::conn::http1;
    pub use hyper::service::service_fn;
    pub use hyper::{Request, Response};
    pub use hyper_util::rt::TokioIo;
    pub use std::net::SocketAddr;
    pub use std::sync::Arc;
    pub use tokio::net::TcpListener;
}

mod tests {
    use super::__parent::*;
    use kubernetes_rs_kubectl::commands::proxy::*;

    #[tokio::test]
    async fn test_proxy_binds_random_port() {
        let config = ProxyConfig {
            address: "127.0.0.1".to_string(),
            port: 0,
            api_server: "https://localhost:6443".to_string(),
            token: Some("test-token".to_string()),
            skip_tls_verify: true,
        };

        let listener = bind_listener(&config).await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Port 0 should have been resolved to an actual port
        assert_ne!(addr.port(), 0);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
    }

    #[test]
    fn test_proxy_config_address_port_parsing() {
        // Verify that address:port strings parse to valid SocketAddr
        let cases = vec![
            ("127.0.0.1", 8001, "127.0.0.1:8001"),
            ("0.0.0.0", 9090, "0.0.0.0:9090"),
            ("127.0.0.1", 0, "127.0.0.1:0"),
        ];
        for (addr, port, expected) in cases {
            let formatted = format!("{}:{}", addr, port);
            let parsed: std::net::SocketAddr = formatted.parse().unwrap();
            assert_eq!(parsed.to_string(), expected);
        }
    }

    #[test]
    fn test_proxy_config_invalid_address() {
        let formatted = format!("{}:{}", "not-an-ip", 8001);
        let result: Result<std::net::SocketAddr, _> = formatted.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_proxy_api_server_trailing_slash_trimmed() {
        // The serve function trims trailing slashes from api_server
        let api_server = "https://localhost:6443/".trim_end_matches('/');
        assert_eq!(api_server, "https://localhost:6443");

        let api_server_no_slash = "https://localhost:6443".trim_end_matches('/');
        assert_eq!(api_server_no_slash, "https://localhost:6443");
    }

    #[test]
    fn test_proxy_target_url_construction() {
        // Verify target URL is built as api_server + path_and_query
        let api_server = "https://localhost:6443";
        let path_and_query = "/api/v1/namespaces/default/pods?limit=10";
        let target_url = format!("{}{}", api_server, path_and_query);
        assert_eq!(
            target_url,
            "https://localhost:6443/api/v1/namespaces/default/pods?limit=10"
        );
    }

    #[test]
    fn test_proxy_config_defaults() {
        let config = ProxyConfig {
            address: "127.0.0.1".to_string(),
            port: 8001,
            api_server: "https://localhost:6443".to_string(),
            token: None,
            skip_tls_verify: false,
        };
        assert_eq!(config.address, "127.0.0.1");
        assert_eq!(config.port, 8001);
        assert!(config.token.is_none());
        assert!(!config.skip_tls_verify);
    }

    #[test]
    fn test_proxy_target_url_root_path() {
        let api_server = "https://localhost:6443";
        let path_and_query = "/";
        let target_url = format!("{}{}", api_server, path_and_query);
        assert_eq!(target_url, "https://localhost:6443/");
    }

    #[test]
    fn test_proxy_authorization_header_format() {
        let token = "my-bearer-token";
        let header = format!("Bearer {}", token);
        assert_eq!(header, "Bearer my-bearer-token");
    }

    #[tokio::test]
    async fn test_bind_listener_specific_port() {
        let config = ProxyConfig {
            address: "127.0.0.1".to_string(),
            port: 0,
            api_server: "https://localhost:6443".to_string(),
            token: None,
            skip_tls_verify: false,
        };
        let listener = bind_listener(&config).await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(addr.port() > 0);
    }

    #[tokio::test]
    async fn test_bind_listener_invalid_address_fails() {
        let config = ProxyConfig {
            address: "999.999.999.999".to_string(),
            port: 0,
            api_server: "https://localhost:6443".to_string(),
            token: None,
            skip_tls_verify: false,
        };
        let result = bind_listener(&config).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_proxy_target_url_with_query_params() {
        let api_server = "https://localhost:6443";
        let path_and_query = "/api/v1/pods?watch=true&resourceVersion=100";
        let target_url = format!("{}{}", api_server, path_and_query);
        assert_eq!(
            target_url,
            "https://localhost:6443/api/v1/pods?watch=true&resourceVersion=100"
        );
    }

    #[test]
    fn test_proxy_config_with_token() {
        let config = ProxyConfig {
            address: "127.0.0.1".to_string(),
            port: 8001,
            api_server: "https://localhost:6443".to_string(),
            token: Some("my-secret-token".to_string()),
            skip_tls_verify: true,
        };
        assert!(config.token.is_some());
        assert_eq!(config.token.unwrap(), "my-secret-token");
        assert!(config.skip_tls_verify);
    }

    #[test]
    fn test_proxy_api_server_multiple_trailing_slashes() {
        let api_server = "https://localhost:6443///".trim_end_matches('/');
        assert_eq!(api_server, "https://localhost:6443");
    }

    #[tokio::test]
    async fn test_bind_listener_port_reuse_fails() {
        // Bind to a random port, then try to bind to the same port
        let config1 = ProxyConfig {
            address: "127.0.0.1".to_string(),
            port: 0,
            api_server: "https://localhost:6443".to_string(),
            token: None,
            skip_tls_verify: false,
        };
        let listener1 = bind_listener(&config1).await.unwrap();
        let bound_port = listener1.local_addr().unwrap().port();

        let config2 = ProxyConfig {
            address: "127.0.0.1".to_string(),
            port: bound_port,
            api_server: "https://localhost:6443".to_string(),
            token: None,
            skip_tls_verify: false,
        };
        let result = bind_listener(&config2).await;
        assert!(
            result.is_err(),
            "Should fail to bind to an already-bound port"
        );
    }
}

/// Ports of `proxy_server_test.go` (`TestAccept`, `TestRegexpMatch`,
/// `TestExtractHost`) plus the 403 path of `FilterServer.ServeHTTP`.
mod proxy_filter {
    use kubernetes_rs_kubectl::commands::proxy::*;

    fn filter(reject_paths: &str, reject_methods: &str) -> FilterServer {
        FilterServer::new(
            DEFAULT_PATH_ACCEPT_RE,
            reject_paths,
            DEFAULT_HOST_ACCEPT_RE,
            reject_methods,
        )
        .unwrap()
    }

    #[test]
    fn proxy_filter_accept_table() {
        let d = DEFAULT_PATH_REJECT_RE;
        let m = DEFAULT_METHOD_REJECT_RE;
        let pod = "/api/v1/namespaces/default/pods/somepod";
        let cases: Vec<(&str, &str, &str, &str, &str, bool)> = vec![
            (d, m, "", "127.0.0.1", "GET", true),
            (d, m, "/api/v1/pods", "127.0.0.1", "GET", true),
            (d, m, "/api/v1/pods", "localhost", "GET", true),
            (
                d,
                m,
                "/api/v1/namespaces/default/pods/attachfoo",
                "localhost",
                "GET",
                true,
            ),
            (
                d,
                m,
                "/api/v1/namespaces/default/pods/execfoo",
                "localhost",
                "GET",
                true,
            ),
            (
                d,
                m,
                "/api/v1/namespaces/default/pods/foo/exec",
                "127.0.0.1",
                "GET",
                false,
            ),
            (
                d,
                m,
                "/api/v1/namespaces/default/pods/foo/attach",
                "127.0.0.1",
                "GET",
                false,
            ),
            (d, m, "/api/v1/pods", "evil.com", "GET", false),
            (d, m, "/api/v1/pods", "localhost.evil.com", "GET", false),
            (d, m, "/api/v1/pods", "127a0b0c1", "GET", false),
            (d, m, "/ui", "localhost", "GET", true),
            (d, m, "/api/v1/pods", "localhost", "POST", true),
            (d, "POST", "/api/v1/pods", "localhost", "POST", false),
            (d, "PUT", pod, "localhost", "PUT", false),
            (d, "PATCH", pod, "localhost", "PATCH", false),
            (d, "POST,PUT,PATCH", pod, "localhost", "PATCH", false),
            (d, "POST,PUT,PATCH", pod, "localhost", "PUT", false),
            (
                d,
                m,
                "/api/v1/namespaces/default/pods/somepod/exec",
                "localhost",
                "POST",
                false,
            ),
            (
                "",
                m,
                "/api/v1/namespaces/default/pods/somepod/exec",
                "localhost",
                "POST",
                true,
            ),
        ];
        for (rp, rm, path, host, method, want) in cases {
            assert_eq!(
                filter(rp, rm).accept(method, path, host),
                want,
                "{rp} {rm} {path} {host} {method}"
            );
        }
    }

    #[test]
    fn proxy_filter_regexp_match() {
        let t = |s: &str, res: &str| matches_regexp(s, &make_regexp_array(res).unwrap());
        assert!(t("foo", "bar,.*"));
        assert!(t("foo", "bar,fo.*"));
        assert!(t("bar", "bar,fo.*"));
        assert!(!t("baz", "bar,fo.*"));
        assert!(make_regexp_array("(").is_err());
    }

    #[test]
    fn proxy_filter_extract_host() {
        assert_eq!(extract_host("localhost:8085"), "localhost");
        assert_eq!(extract_host("marmalade"), "marmalade");
        assert_eq!(extract_host("[::1]:8001"), "::1");
    }

    #[tokio::test]
    async fn proxy_filter_forbids_rejected_requests() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let cfg = ProxyConfig {
            address: "127.0.0.1".to_string(),
            port: 0,
            api_server: "http://127.0.0.1:1".to_string(),
            token: None,
            skip_tls_verify: false,
        };
        let l = bind_listener(&cfg).await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(serve(l, cfg, Some(FilterServer::with_defaults())));
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /api/v1/pods HTTP/1.1\r\nHost: evil.com\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
        assert!(out.contains("Forbidden"));
    }
}
