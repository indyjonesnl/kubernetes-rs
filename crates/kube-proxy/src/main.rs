// mimalloc as the global allocator (off by default; `--features mimalloc`).
// Required for the musl static builds — musl's default allocator is ~10x
// slower under multi-threaded lock contention — and lowers idle RSS (#1041).
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use clap::Parser;
use kubernetes_rs_kube_proxy::{
    iptables::{DEFAULT_CLUSTER_CIDR, DEFAULT_NODEPORT_RANGE},
    KubeProxyConfig,
};
use kubernetes_rs_storage::{StorageBackend, StorageConfig};
use std::sync::Arc;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "kubernetes-rs-kube-proxy")]
#[command(about = "kubernetes-rs Kube-proxy - Network proxy for service load balancing")]
struct Args {
    /// Comma-separated `Gate=bool` pairs, e.g. `StorageCapacityScoring=true`
    /// (upstream `--feature-gates`, component-base featuregate `Set`).
    #[arg(long, default_value = "")]
    feature_gates: String,

    /// Path to a KubeProxyConfiguration file (options.go `--config`). When set
    /// it replaces the per-field flags it covers (options.go:215-231).
    #[arg(long)]
    config: Option<String>,

    /// Hostname override (options.go `--hostname-override`); trimmed and
    /// lower-cased, wins over the config file (processHostnameOverrideFlag).
    #[arg(long, default_value = "")]
    hostname_override: String,

    /// Node name
    #[arg(long)]
    node_name: String,

    /// Etcd endpoints (comma-separated)
    #[arg(long, default_value = "http://localhost:2379")]
    etcd_servers: String,

    /// Storage backend: "etcd" or "sqlite"
    #[arg(long, default_value = "etcd")]
    storage_backend: String,

    /// SQLite database path (only used when --storage-backend=sqlite)
    #[arg(long, default_value = "./data/kubernetes-rs.db")]
    data_dir: String,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Sync interval in seconds
    #[arg(long, default_value = "1")]
    sync_interval: u64,

    /// ClusterIP CIDR — must match the apiserver's
    /// `--service-cluster-ip-range`. Used to scope the POSTROUTING
    /// MASQUERADE rule so it doesn't fire on non-cluster traffic.
    #[arg(long, default_value = DEFAULT_CLUSTER_CIDR)]
    cluster_cidr: String,

    /// NodePort range in iptables `start:end` form — must match the
    /// apiserver's `--service-node-port-range`. Hyphen also accepted
    /// (`30000-32767`) and normalized.
    #[arg(long, default_value = DEFAULT_NODEPORT_RANGE)]
    node_port_range: String,

    /// API server URL for API mode (e.g. https://localhost:6443). kube-proxy
    /// runs host-network, so this is the host-published api-server port, not the
    /// `api-server` compose DNS name.
    #[arg(long, default_value = "https://localhost:6443")]
    api_server_url: String,

    /// Path to a kubeconfig for API mode (CA + server). When set, kube-proxy
    /// reads Services/Endpoints/EndpointSlices from the api-server via
    /// ApiStorage instead of storage (in-cluster compose service). When unset,
    /// the storage backend is used (all-in-one / legacy path).
    #[arg(long)]
    kubeconfig: Option<String>,

    /// Skip TLS verification for the api-server connection in API mode (the
    /// kubeconfig CA normally validates the self-signed cert).
    #[arg(long)]
    insecure_skip_tls_verify: bool,

    /// Also run the node-network agent: write this node's CNI conflist from its
    /// `spec.podCIDR`, install host-gw routes to the other nodes' pod CIDRs, and
    /// keep cluster traffic out of the masquerade path. Requires the
    /// controller-manager to run with `--allocate-node-cidrs`.
    /// Upstream analogue: kindnetd.
    #[arg(long)]
    configure_node_network: bool,

    /// Path of the CNI conflist to write when `--configure-node-network` is set.
    /// Must be in the conf dir the container runtime watches.
    #[arg(long, default_value = "/etc/cni/net.d/10-kubernetes-rs.conflist")]
    cni_conf_path: String,

    /// Cluster-wide pod CIDR (the controller-manager's `--cluster-cidr`). Used as
    /// a no-masquerade CIDR so cross-node pod traffic keeps its source IP.
    #[arg(long, default_value = "10.244.0.0/16")]
    pod_cidr: String,

    /// IP address (and port) for the metrics/status server to serve on
    /// (upstream `--metrics-bind-address`; empty = `127.0.0.1:10249`, a bare IP
    /// gets port 10249).
    #[arg(long, default_value = "")]
    metrics_bind_address: String,

    /// Health listener address (options.go:118; defaults.go:47-50).
    #[arg(long, default_value = "0.0.0.0:10256")]
    healthz_bind_address: String,

    /// CIDRs of node addresses NodePort services listen on (options.go
    /// `--nodeport-addresses`); empty means all local addresses.
    #[arg(long, value_delimiter = ',')]
    nodeport_addresses: Vec<String>,

    /// Create the route_localnet-based rules that make NodePorts reachable on
    /// 127.0.0.0/8 (`--iptables-localhost-nodeports`, options.go; default true).
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    iptables_localhost_nodeports: bool,

    /// Exit after a diagnostic listener bind failure (options.go:120).
    #[arg(long)]
    bind_address_hard_fail: bool,
}

/// `Options.Complete` (options.go:205-241) for `--config`: load the file,
/// apply `--hostname-override`, validate the fully resolved config
/// (`Options.Validate`) and let it replace the flag values it covers.
fn apply_config_file(args: &mut Args) -> Result<()> {
    use kubernetes_rs_kube_proxy::config as kpc;
    let Some(path) = args.config.clone() else {
        return Ok(());
    };
    let mut cfg = kpc::load_config_from_file(std::path::Path::new(&path))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    kpc::process_hostname_override(&mut cfg, &args.hostname_override)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let errs = kpc::validate(&kpc::convert_to_internal(&cfg), false);
    if !errs.is_empty() {
        let msgs: Vec<String> = errs
            .iter()
            .map(|e| format!("{}: {}", e.field, e.detail))
            .collect();
        anyhow::bail!("invalid configuration: {}", msgs.join("; "));
    }
    args.healthz_bind_address = cfg.healthz_bind_address.clone();
    args.metrics_bind_address = cfg.metrics_bind_address.clone();
    args.nodeport_addresses = cfg.node_port_addresses.clone();
    args.iptables_localhost_nodeports = cfg.iptables.localhost_node_ports.unwrap_or(true);
    args.bind_address_hard_fail = cfg.bind_address_hard_fail;
    if !cfg.cluster_cidr.is_empty() {
        args.cluster_cidr = cfg.cluster_cidr.clone();
    }
    if !cfg.feature_gates.is_empty() {
        let gates: Vec<String> = cfg
            .feature_gates
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        args.feature_gates = gates.join(",");
    }
    Ok(())
}

/// Apply `--feature-gates` process-wide. Port of `featureGate.Set`
/// (staging/src/k8s.io/component-base/featuregate/feature_gate.go:340-359).
fn apply_feature_gates(args: &Args) -> Result<()> {
    kubernetes_rs_common::feature_gates::set_from_str(&args.feature_gates)
        .map_err(|e| anyhow::anyhow!("invalid --feature-gates: {e}"))?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = <Args as clap::CommandFactory>::command();
    let matches = cmd.clone().get_matches();
    let mut args = <Args as clap::FromArgMatches>::from_arg_matches(&matches)
        .map_err(|e| e.exit())
        .unwrap();
    apply_config_file(&mut args)?;
    if let Some(path) = args.config.clone() {
        // options.go initWatcher/runLoop: exit with an error when the file changes.
        let watcher =
            kubernetes_rs_kube_proxy::config::init_config_file_watcher(std::path::Path::new(&path))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        tokio::spawn(async move {
            let msg = watcher.changed().await;
            eprintln!("kube-proxy: {msg}");
            std::process::exit(1);
        });
    }
    apply_feature_gates(&args)?;

    // options.go:244-251: after the feature gates are applied, the flagz
    // reader exists only when ComponentFlagz is enabled.
    if kubernetes_rs_common::feature_gates::enabled(
        kubernetes_rs_common::feature_gates::Feature::ComponentFlagz,
    ) {
        kubernetes_rs_kube_proxy::serve_metrics::set_flagz_reader(
            kubernetes_rs_common::flagz::flags_from_clap(&cmd, &matches),
        );
    }

    kubernetes_rs_common::tracing::init_basic_tracing("kube-proxy", &args.log_level)?;
    info!(
        "Starting kubernetes-rs kube-proxy {}",
        kubernetes_rs_common::build_info::version_line()
    );

    let config = KubeProxyConfig {
        node_name: args.node_name.clone(),
        sync_interval: args.sync_interval,
        cluster_cidr: args.cluster_cidr.clone(),
        // Accept hyphen form (`30000-32767`) as a convenience — k8s and Go
        // flags use the hyphen, iptables wants the colon. Normalize here.
        nodeport_range: args.node_port_range.replace('-', ":"),
        // Cluster pod CIDR and Service CIDR both keep their source IPs; only
        // egress off-cluster is masqueraded (kindnetd masq.go:105).
        node_net: args.configure_node_network.then(|| {
            kubernetes_rs_kube_proxy::node_net::NodeNetConfig {
                node_name: args.node_name.clone(),
                cni_conf_path: args.cni_conf_path.clone(),
                no_masq_cidrs: vec![args.pod_cidr.clone(), args.cluster_cidr.clone()],
            }
        }),
        // `--metrics-bind-address`; empty gets the defaults.go:52-56 default.
        metrics_bind_address: kubernetes_rs_kube_proxy::serve_metrics::default_metrics_bind_address(
            &args.metrics_bind_address,
        ),
        nodeport_addresses: args.nodeport_addresses.clone(),
        localhost_nodeports: args.iptables_localhost_nodeports,
    };

    let diagnostics = kubernetes_rs_kube_proxy::serve_metrics::DiagnosticsOptions {
        healthz_bind_address: if args.healthz_bind_address.is_empty() {
            String::new()
        } else {
            kubernetes_rs_kube_proxy::serve_metrics::append_port_if_needed(
                &args.healthz_bind_address,
                10256,
            )
        },
        bind_address_hard_fail: args.bind_address_hard_fail,
    };

    // In-cluster path: read cluster state from the api-server (no storage handle).
    if let Some(kubeconfig) = args.kubeconfig.as_deref() {
        use kubernetes_rs_client::http::ApiClient;
        use kubernetes_rs_client::kubeconfig::KubeConfig;

        let cfg = KubeConfig::load_from_file(&std::path::PathBuf::from(kubeconfig))?;
        let ca_pem = cfg.get_ca_cert_pem().ok().flatten();
        let insecure =
            args.insecure_skip_tls_verify || cfg.should_skip_tls_verify().unwrap_or(false);
        // Client cert/key for mTLS auth (#1578).
        let client_cert = cfg.get_client_cert_pem().ok().flatten();
        let client_key = cfg.get_client_key_pem().ok().flatten();
        info!(
            "Kube-proxy API mode: api-server={}, ca={}, insecure={}, client-cert={}",
            args.api_server_url,
            ca_pem.is_some(),
            insecure,
            client_cert.is_some()
        );
        // The CA validates the server's TLS cert; client cert/key (when the
        // kubeconfig provides them) authenticate this component via mTLS (#1578).
        let client = Arc::new(
            ApiClient::with_tls(
                &args.api_server_url,
                insecure,
                ca_pem,
                client_cert.map(|p| p.into_bytes()),
                client_key.map(|p| p.into_bytes()),
                None,
            )?
            // `rest.SetKubernetesDefaults`: the component's default User-Agent.
            .with_default_user_agent(
                kubernetes_rs_common::compatibility::default_kubernetes_user_agent(),
            ),
        );
        return kubernetes_rs_kube_proxy::run_with_api_and_diagnostics(client, config, diagnostics)
            .await;
    }

    let storage_config = match args.storage_backend.as_str() {
        #[cfg(feature = "sqlite")]
        "sqlite" => {
            info!("Using SQLite storage backend at: {}", args.data_dir);
            StorageConfig::Sqlite {
                path: args.data_dir,
            }
        }
        _ => {
            let endpoints: Vec<String> = args
                .etcd_servers
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            info!("Connecting to etcd at: {:?}", endpoints);
            StorageConfig::Etcd { endpoints }
        }
    };
    let storage = Arc::new(StorageBackend::new(storage_config).await?);

    kubernetes_rs_kube_proxy::run_with_diagnostics(storage, config, diagnostics).await
}

#[cfg(test)]
mod feature_gates_flag_tests {
    use super::*;
    use kubernetes_rs_common::feature_gates::{enabled, reset_to_defaults, Feature};
    use serial_test::serial;

    fn parse(gates: &str) -> Args {
        let mut v: Vec<&str> = vec!["kube-proxy", "--node-name", "n"];
        v.push(gates);
        Args::try_parse_from(v).unwrap()
    }

    /// Upstream `--feature-gates` (component-base featuregate `Set`,
    /// feature_gate.go:340-359) is applied process-wide from the flag.
    #[test]
    #[serial]
    fn feature_gates_flag_applies() {
        reset_to_defaults();
        apply_feature_gates(&parse("--feature-gates=StorageCapacityScoring=true")).unwrap();
        assert!(enabled(Feature::StorageCapacityScoring));
        reset_to_defaults();
    }

    /// feature_gate.go:385-389.
    #[test]
    #[serial]
    fn feature_gates_flag_rejects_unknown_gate() {
        reset_to_defaults();
        let err = apply_feature_gates(&parse("--feature-gates=fooBarBaz=true")).unwrap_err();
        assert!(
            err.to_string()
                .contains("unrecognized feature gate: fooBarBaz"),
            "{err}"
        );
    }
}
