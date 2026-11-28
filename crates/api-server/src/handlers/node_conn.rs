/// Resolve the kubelet connection parameters from a `Node` resource.
///
/// Extracted from `proxy.rs` so that Tasks 6 and 7 (exec/attach/logs routing)
/// can reuse the same InternalIP→ExternalIP fallback, advertised-port lookup,
/// and scheme selection without duplicating the logic.
use kubernetes_rs_common::resources::{Node, NodeAddress};

/// The default of `--kubelet-preferred-address-types`
/// (`cmd/kube-apiserver/app/options/options.go:75-85`): `--override-hostname`
/// first, then internal (DNS before IP), then external (DNS before IP).
pub const DEFAULT_PREFERRED_ADDRESS_TYPES: [&str; 5] = [
    "Hostname",
    "InternalDNS",
    "InternalIP",
    "ExternalDNS",
    "ExternalIP",
];

static PREFERRED_ADDRESS_TYPES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// `--kubelet-preferred-address-types` (options.go:129): set once at startup;
/// an empty list keeps the default, as the flag's default does.
pub fn set_preferred_address_types(types: Vec<String>) {
    if !types.is_empty() {
        let _ = PREFERRED_ADDRESS_TYPES.set(types);
    }
}

fn preferred_address_types() -> Vec<&'static str> {
    match PREFERRED_ADDRESS_TYPES.get() {
        Some(t) => t.iter().map(String::as_str).collect(),
        None => DEFAULT_PREFERRED_ADDRESS_TYPES.to_vec(),
    }
}

/// `NoMatchError` (`pkg/util/node/node.go:36-44`).
#[derive(Debug, Clone)]
pub struct NoMatchError {
    pub addresses: Vec<NodeAddress>,
}

impl std::fmt::Display for NoMatchError {
    /// `no preferred addresses found; known addresses: %v`, Go's `%v` of a
    /// `[]v1.NodeAddress` being `[{Type Address} {Type Address}]`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no preferred addresses found; known addresses: [")?;
        for (i, a) in self.addresses.iter().enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            write!(f, "{{{} {}}}", a.address_type, a.address)?;
        }
        write!(f, "]")
    }
}

impl std::error::Error for NoMatchError {}

/// `GetPreferredNodeAddress` (`pkg/util/node/node.go:47-57`): the first
/// address of the first preferred type, in the preference order given. The
/// `kubernetes.io/hostname` label is not an address and is ignored.
pub fn get_preferred_node_address(
    node: &Node,
    preferred_address_types: &[&str],
) -> Result<String, NoMatchError> {
    let addresses = node
        .status
        .as_ref()
        .and_then(|s| s.addresses.as_deref())
        .unwrap_or_default();
    for t in preferred_address_types {
        if let Some(a) = addresses.iter().find(|a| a.address_type == *t) {
            return Ok(a.address.clone());
        }
    }
    Err(NoMatchError {
        addresses: addresses.to_vec(),
    })
}

/// Resolved kubelet connection parameters for a node.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeConn {
    /// The hostname or IP address to connect to (first of `DEFAULT_PREFERRED_ADDRESS_TYPES`).
    pub host: String,
    /// The TCP port of the kubelet API.
    pub port: u16,
    /// URL scheme. Always `"https"`: the kubelet serves its `:10250` API over
    /// TLS (#1644), matching upstream, and a Kubernetes api-server always
    /// proxies logs/exec/attach/metrics to `https://<nodeIP>:10250`. The
    /// kubelet's serving cert is self-signed, so the proxy client skips
    /// verification (see `streamproxy`'s TLS connector and the reqwest
    /// `danger_accept_invalid_certs` log client).
    pub scheme: &'static str,
}

/// Resolve kubelet connection parameters from a node's status.
///
/// Port resolution order (mirrors `pkg/registry/core/node/strategy.go::ResourceLocation`):
/// 1. `port_override` — explicit port from the URL id (`<name>:<port>`).
/// 2. `status.daemonEndpoints.kubeletEndpoint.port` — advertised by the kubelet.
/// 3. 10250 — the Kubernetes default.
///
/// Returns `Err(kubernetes_rs_common::Error::Internal)` (upstream's plain error) when the node has no
/// usable address.
pub fn node_conn(
    node: &Node,
    port_override: Option<u16>,
) -> Result<NodeConn, kubernetes_rs_common::Error> {
    node_conn_with_types(node, port_override, &preferred_address_types())
}

/// [`node_conn`] with an explicit `--kubelet-preferred-address-types`.
pub fn node_conn_with_types(
    node: &Node,
    port_override: Option<u16>,
    preferred: &[&str],
) -> Result<NodeConn, kubernetes_rs_common::Error> {
    // `NodeConnectionInfoGetter.GetConnectionInfo`
    // (`pkg/kubelet/client/kubelet_client.go:195`) with
    // `--kubelet-preferred-address-types`. It returns the NoMatchError
    // unchanged, a plain error the REST layer reports as a 500.
    let host = get_preferred_node_address(node, preferred)
        .map_err(|e| kubernetes_rs_common::Error::Internal(e.to_string()))?;

    let port = port_override.unwrap_or_else(|| {
        node.status
            .as_ref()
            .and_then(|s| s.daemon_endpoints.as_ref())
            .and_then(|d| d.kubelet_endpoint.as_ref())
            .map(|e| e.port as u16)
            .unwrap_or(10250)
    });

    Ok(NodeConn {
        host,
        port,
        scheme: "https",
    })
}
