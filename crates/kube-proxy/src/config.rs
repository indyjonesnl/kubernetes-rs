//! `KubeProxyConfiguration`: the v1alpha1 wire type, its defaults, the
//! internal type it converts to and the validation of that.
//!
//! Port of release-1.35:
//! - `staging/src/k8s.io/kube-proxy/config/v1alpha1/types.go` (wire types)
//! - `pkg/proxy/apis/config/types.go` (internal types, `ProxyMode`/`LocalMode`)
//! - `pkg/proxy/apis/config/v1alpha1/defaults.go`
//!   (`SetDefaults_KubeProxyConfiguration`, `getDefaultAddresses`)
//! - `pkg/proxy/apis/config/v1alpha1/conversion.go`
//!   (`Convert_v1alpha1_KubeProxyConfiguration_To_config_KubeProxyConfiguration`)
//! - `pkg/proxy/apis/config/validation/validation.go` (`Validate`)
//!
//! Flag and config-file wiring (`cmd/kube-proxy/app/options.go`) is separate.

use crate::serve_metrics::{append_port_if_needed, PROXY_STATUS_PORT};
use crate::service_map::{parse_cidr_sloppy, parse_ip_sloppy};
use kubernetes_rs_common::go_duration::{format_go_duration, parse_go_duration};
use kubernetes_rs_common::validation::field::{Error, ErrorList, Path};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::net::IpAddr;

/// `ports.ProxyHealthzPort` (pkg/cluster/ports/ports.go).
pub const PROXY_HEALTHZ_PORT: u16 = 10256;
/// `qos.KubeProxyOOMScoreAdj` (pkg/kubelet/qos/policy.go).
pub const KUBE_PROXY_OOM_SCORE_ADJ: i32 = -999;
/// `config.NodePortAddressesPrimary`.
pub const NODE_PORT_ADDRESSES_PRIMARY: &str = "primary";

/// `metav1.Duration`: a Go duration string on the wire, held as nanoseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Duration(pub i64);

impl Duration {
    pub const fn secs(s: i64) -> Self {
        Duration(s * 1_000_000_000)
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format_go_duration(self.0))
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_go_duration(&s)
            .map(Duration)
            .map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for Duration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format_go_duration(self.0))
    }
}

/// `ProxyMode` constants (pkg/proxy/apis/config/types.go).
pub const PROXY_MODE_IPTABLES: &str = "iptables";
pub const PROXY_MODE_IPVS: &str = "ipvs";
pub const PROXY_MODE_NFTABLES: &str = "nftables";
pub const PROXY_MODE_KERNELSPACE: &str = "kernelspace";

/// `LocalMode` constants.
pub const LOCAL_MODE_CLUSTER_CIDR: &str = "ClusterCIDR";
pub const LOCAL_MODE_NODE_CIDR: &str = "NodeCIDR";
pub const LOCAL_MODE_BRIDGE_INTERFACE: &str = "BridgeInterface";
pub const LOCAL_MODE_INTERFACE_NAME_PREFIX: &str = "InterfaceNamePrefix";

/// `logsapi.LogFlushFreqDefault`.
const LOG_FLUSH_FREQ_DEFAULT: Duration = Duration::secs(5);

/// `componentbaseconfigv1alpha1.ClientConnectionConfiguration`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientConnectionConfiguration {
    pub kubeconfig: String,
    pub accept_content_types: String,
    pub content_type: String,
    pub qps: f32,
    pub burst: i32,
}

/// The parts of `logsapi.LoggingConfiguration` the validation reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoggingConfiguration {
    pub format: String,
    pub flush_frequency: Duration,
    pub verbosity: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeProxyIPTablesConfiguration {
    pub masquerade_bit: Option<i32>,
    pub masquerade_all: bool,
    pub localhost_node_ports: Option<bool>,
    pub sync_period: Duration,
    pub min_sync_period: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeProxyIPVSConfiguration {
    pub sync_period: Duration,
    pub min_sync_period: Duration,
    pub scheduler: String,
    #[serde(rename = "excludeCIDRs")]
    pub exclude_cidrs: Vec<String>,
    #[serde(rename = "strictARP")]
    pub strict_arp: bool,
    pub tcp_timeout: Duration,
    pub tcp_fin_timeout: Duration,
    pub udp_timeout: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeProxyNFTablesConfiguration {
    pub masquerade_bit: Option<i32>,
    pub masquerade_all: bool,
    pub sync_period: Duration,
    pub min_sync_period: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeProxyConntrackConfiguration {
    pub max_per_core: Option<i32>,
    pub min: Option<i32>,
    pub tcp_established_timeout: Option<Duration>,
    pub tcp_close_wait_timeout: Option<Duration>,
    pub tcp_be_liberal: bool,
    pub udp_timeout: Duration,
    pub udp_stream_timeout: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeProxyWinkernelConfiguration {
    pub network_name: String,
    pub source_vip: String,
    #[serde(rename = "enableDSR")]
    pub enable_dsr: bool,
    pub root_hns_endpoint_name: String,
    pub forward_health_check_vip: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DetectLocalConfiguration {
    pub bridge_interface: String,
    pub interface_name_prefix: String,
}

/// `v1alpha1.KubeProxyConfiguration`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeProxyConfiguration {
    pub kind: String,
    pub api_version: String,
    pub feature_gates: BTreeMap<String, bool>,
    pub client_connection: ClientConnectionConfiguration,
    pub logging: LoggingConfiguration,
    pub hostname_override: String,
    pub bind_address: String,
    pub healthz_bind_address: String,
    pub metrics_bind_address: String,
    pub bind_address_hard_fail: bool,
    pub enable_profiling: bool,
    pub show_hidden_metrics_for_version: String,
    pub mode: String,
    pub iptables: KubeProxyIPTablesConfiguration,
    pub ipvs: KubeProxyIPVSConfiguration,
    pub nftables: KubeProxyNFTablesConfiguration,
    pub winkernel: KubeProxyWinkernelConfiguration,
    pub detect_local_mode: String,
    pub detect_local: DetectLocalConfiguration,
    #[serde(rename = "clusterCIDR")]
    pub cluster_cidr: String,
    pub node_port_addresses: Vec<String>,
    pub oom_score_adj: Option<i32>,
    pub conntrack: KubeProxyConntrackConfiguration,
    pub config_sync_period: Duration,
    pub port_range: String,
    pub windows_run_as_service: bool,
}

/// `getDefaultAddresses` (defaults.go:143-149): IPv6 addresses are bracketed
/// so a port can be appended.
fn get_default_addresses(bind_address: &str) -> (&'static str, &'static str) {
    match parse_ip_sloppy(bind_address) {
        Some(IpAddr::V4(_)) => ("0.0.0.0", "127.0.0.1"),
        _ => ("[::]", "[::1]"),
    }
}

/// `SetDefaults_KubeProxyConfiguration` (defaults.go:36-137).
pub fn set_defaults(obj: &mut KubeProxyConfiguration) {
    if obj.bind_address.is_empty() {
        obj.bind_address = "0.0.0.0".to_string();
    }
    let (healthz_addr, metrics_addr) = get_default_addresses(&obj.bind_address);

    obj.healthz_bind_address = if obj.healthz_bind_address.is_empty() {
        format!("{healthz_addr}:{PROXY_HEALTHZ_PORT}")
    } else {
        append_port_if_needed(&obj.healthz_bind_address, PROXY_HEALTHZ_PORT)
    };
    obj.metrics_bind_address = if obj.metrics_bind_address.is_empty() {
        format!("{metrics_addr}:{PROXY_STATUS_PORT}")
    } else {
        append_port_if_needed(&obj.metrics_bind_address, PROXY_STATUS_PORT)
    };

    obj.oom_score_adj.get_or_insert(KUBE_PROXY_OOM_SCORE_ADJ);
    let zero_to = |d: &mut Duration, v: Duration| {
        if d.0 == 0 {
            *d = v;
        }
    };
    zero_to(&mut obj.iptables.sync_period, Duration::secs(30));
    zero_to(&mut obj.iptables.min_sync_period, Duration::secs(1));
    obj.iptables.localhost_node_ports.get_or_insert(true);
    zero_to(&mut obj.ipvs.sync_period, Duration::secs(30));
    zero_to(&mut obj.nftables.sync_period, Duration::secs(30));
    zero_to(&mut obj.nftables.min_sync_period, Duration::secs(1));

    obj.conntrack.max_per_core.get_or_insert(32 * 1024);
    obj.conntrack.min.get_or_insert(128 * 1024);
    obj.iptables.masquerade_bit.get_or_insert(14);
    obj.nftables.masquerade_bit.get_or_insert(14);
    // 1 day (1/5 default).
    obj.conntrack
        .tcp_established_timeout
        .get_or_insert(Duration::secs(24 * 3600));
    // CLOSE_WAIT is one hour by default to better match typical server
    // timeouts (https://github.com/kubernetes/kubernetes/issues/32551).
    obj.conntrack
        .tcp_close_wait_timeout
        .get_or_insert(Duration::secs(3600));
    zero_to(&mut obj.config_sync_period, Duration::secs(15 * 60));

    if obj.client_connection.content_type.is_empty() {
        obj.client_connection.content_type = "application/vnd.kubernetes.protobuf".to_string();
    }
    if obj.client_connection.qps == 0.0 {
        obj.client_connection.qps = 5.0;
    }
    if obj.client_connection.burst == 0 {
        obj.client_connection.burst = 10;
    }
    // logsapi.SetRecommendedLoggingConfiguration.
    if obj.logging.format.is_empty() {
        obj.logging.format = "text".to_string();
    }
    zero_to(&mut obj.logging.flush_frequency, LOG_FLUSH_FREQ_DEFAULT);
}

// ---------------------------------------------------------------------------
// Internal type (pkg/proxy/apis/config/types.go), reduced to what Validate reads.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InternalLinux {
    pub conntrack: KubeProxyConntrackConfiguration,
    pub masquerade_all: bool,
    pub oom_score_adj: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InternalDetectLocal {
    pub bridge_interface: String,
    pub cluster_cidrs: Vec<String>,
    pub interface_name_prefix: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InternalKubeProxyConfiguration {
    pub linux: InternalLinux,
    pub feature_gates: BTreeMap<String, bool>,
    pub client_connection: ClientConnectionConfiguration,
    pub logging: LoggingConfiguration,
    pub bind_address: String,
    pub healthz_bind_address: String,
    pub metrics_bind_address: String,
    pub show_hidden_metrics_for_version: String,
    pub mode: String,
    pub iptables_masquerade_bit: Option<i32>,
    pub ipvs: KubeProxyIPVSConfiguration,
    pub nftables_masquerade_bit: Option<i32>,
    pub detect_local_mode: String,
    pub detect_local: InternalDetectLocal,
    pub node_port_addresses: Vec<String>,
    pub sync_period: Duration,
    pub min_sync_period: Duration,
    pub config_sync_period: Duration,
}

/// `Convert_v1alpha1_KubeProxyConfiguration_To_config_KubeProxyConfiguration`
/// (conversion.go:63-95): the sync periods come from the section of the
/// selected mode, `clusterCIDR` is split on commas.
pub fn convert_to_internal(input: &KubeProxyConfiguration) -> InternalKubeProxyConfiguration {
    let (sync_period, min_sync_period) = match input.mode.as_str() {
        PROXY_MODE_IPVS => (input.ipvs.sync_period, input.ipvs.min_sync_period),
        PROXY_MODE_NFTABLES => (input.nftables.sync_period, input.nftables.min_sync_period),
        _ => (input.iptables.sync_period, input.iptables.min_sync_period),
    };
    let masquerade_all = match input.mode.as_str() {
        PROXY_MODE_NFTABLES => input.nftables.masquerade_all,
        _ => input.iptables.masquerade_all,
    };
    InternalKubeProxyConfiguration {
        linux: InternalLinux {
            conntrack: input.conntrack.clone(),
            masquerade_all,
            oom_score_adj: input.oom_score_adj,
        },
        feature_gates: input.feature_gates.clone(),
        client_connection: input.client_connection.clone(),
        logging: input.logging.clone(),
        bind_address: input.bind_address.clone(),
        healthz_bind_address: input.healthz_bind_address.clone(),
        metrics_bind_address: input.metrics_bind_address.clone(),
        show_hidden_metrics_for_version: input.show_hidden_metrics_for_version.clone(),
        mode: input.mode.clone(),
        iptables_masquerade_bit: input.iptables.masquerade_bit,
        ipvs: input.ipvs.clone(),
        nftables_masquerade_bit: input.nftables.masquerade_bit,
        detect_local_mode: input.detect_local_mode.clone(),
        detect_local: InternalDetectLocal {
            bridge_interface: input.detect_local.bridge_interface.clone(),
            cluster_cidrs: if input.cluster_cidr.is_empty() {
                Vec::new()
            } else {
                input.cluster_cidr.split(',').map(str::to_string).collect()
            },
            interface_name_prefix: input.detect_local.interface_name_prefix.clone(),
        },
        node_port_addresses: input.node_port_addresses.clone(),
        sync_period,
        min_sync_period,
        config_sync_period: input.config_sync_period,
    }
}

// ---------------------------------------------------------------------------
// Validation (pkg/proxy/apis/config/validation/validation.go)
// ---------------------------------------------------------------------------

const GE_ZERO: &str = "must be greater than or equal to 0";

/// `Validate` (validation.go:36-92). `windows` selects
/// `validateProxyModeWindows` (upstream branches on `runtime.GOOS`).
pub fn validate(config: &InternalKubeProxyConfiguration, windows: bool) -> ErrorList {
    let mut errs = ErrorList::new();
    let new_path = Path::new("KubeProxyConfiguration");

    // `effectiveFeatures.SetFromMap`: an unknown gate is an error
    // (feature_gate.go:385-389).
    for name in config.feature_gates.keys() {
        if kubernetes_rs_common::feature_gates::upstream_gate(name).is_none() {
            errs.push(Error::invalid(
                &new_path.child("featureGates"),
                format!("{:?}", config.feature_gates),
                format!("unrecognized feature gate: {name}"),
            ));
        }
    }

    errs.extend(validate_masquerade_bit(
        config.iptables_masquerade_bit,
        &new_path
            .child("KubeProxyIPTablesConfiguration")
            .child("MasqueradeBit"),
    ));
    match config.mode.as_str() {
        PROXY_MODE_IPVS => errs.extend(validate_ipvs(
            &config.ipvs,
            &new_path.child("KubeProxyIPVSConfiguration"),
        )),
        PROXY_MODE_NFTABLES => errs.extend(validate_masquerade_bit(
            config.nftables_masquerade_bit,
            &new_path
                .child("KubeProxyNFTablesConfiguration")
                .child("MasqueradeBit"),
        )),
        _ => {}
    }
    errs.extend(validate_linux(
        &config.linux,
        &new_path.child("KubeProxyLinuxConfiguration"),
    ));
    errs.extend(validate_proxy_mode(
        &config.mode,
        &new_path.child("Mode"),
        windows,
    ));
    if config.client_connection.burst < 0 {
        errs.push(Error::invalid(
            &new_path.child("ClientConnection").child("Burst"),
            i64::from(config.client_connection.burst),
            GE_ZERO,
        ));
    }

    if config.config_sync_period.0 <= 0 {
        errs.push(Error::invalid(
            &new_path.child("ConfigSyncPeriod"),
            config.config_sync_period.to_string(),
            "must be greater than 0",
        ));
    }
    if config.sync_period.0 <= 0 {
        errs.push(Error::invalid(
            &new_path.child("SyncPeriod"),
            config.sync_period.to_string(),
            "must be greater than 0",
        ));
    }
    if config.min_sync_period.0 < 0 {
        errs.push(Error::invalid(
            &new_path.child("MinSyncPeriod"),
            config.min_sync_period.to_string(),
            GE_ZERO,
        ));
    }
    if config.min_sync_period.0 > config.sync_period.0 {
        errs.push(Error::invalid(
            &new_path.child("SyncPeriod"),
            config.min_sync_period.to_string(),
            format!(
                "must be greater than or equal to {}",
                new_path.child("MinSyncPeriod")
            ),
        ));
    }

    if parse_ip_sloppy(&config.bind_address).is_none() {
        errs.push(Error::invalid(
            &new_path.child("BindAddress"),
            config.bind_address.clone(),
            "not a valid textual representation of an IP address",
        ));
    }
    if !config.healthz_bind_address.is_empty() {
        errs.extend(validate_host_port(
            &config.healthz_bind_address,
            &new_path.child("HealthzBindAddress"),
        ));
    }
    errs.extend(validate_host_port(
        &config.metrics_bind_address,
        &new_path.child("MetricsBindAddress"),
    ));

    errs.extend(validate_node_port_address(
        &config.node_port_addresses,
        &new_path.child("NodePortAddresses"),
    ));
    errs.extend(validate_show_hidden_metrics_version(
        &config.show_hidden_metrics_for_version,
        &new_path.child("ShowHiddenMetricsForVersion"),
    ));

    errs.extend(validate_detect_local_mode(
        &config.detect_local_mode,
        &new_path.child("DetectLocalMode"),
    ));
    errs.extend(validate_detect_local_configuration(
        &config.detect_local_mode,
        &config.detect_local,
        &new_path.child("DetectLocalConfiguration"),
    ));
    errs.extend(validate_logging(
        &config.logging,
        &new_path.child("logging"),
    ));
    errs
}

/// The `MasqueradeBit` arm of `validateKubeProxyIPTablesConfiguration` and
/// `validateKubeProxyNFTablesConfiguration` (:94-116).
fn validate_masquerade_bit(bit: Option<i32>, fld_path: &Path) -> ErrorList {
    match bit {
        Some(b) if !(0..=31).contains(&b) => vec![Error::invalid(
            fld_path,
            i64::from(b),
            "must be within the range [0, 31]",
        )],
        _ => ErrorList::new(),
    }
}

fn nonneg(d: Duration, fld_path: &Path) -> Option<Error> {
    (d.0 < 0).then(|| Error::invalid(fld_path, d.to_string(), GE_ZERO))
}

/// `validateKubeProxyIPVSConfiguration` (:104-111), `validateIPVSTimeout`,
/// `validateIPVSExcludeCIDRs`.
fn validate_ipvs(config: &KubeProxyIPVSConfiguration, fld_path: &Path) -> ErrorList {
    let mut errs = ErrorList::new();
    errs.extend(nonneg(config.tcp_timeout, &fld_path.child("TCPTimeout")));
    errs.extend(nonneg(
        config.tcp_fin_timeout,
        &fld_path.child("TCPFinTimeout"),
    ));
    errs.extend(nonneg(config.udp_timeout, &fld_path.child("UDPTimeout")));
    for (i, cidr) in config.exclude_cidrs.iter().enumerate() {
        if parse_cidr_sloppy(cidr).is_none() {
            errs.push(Error::invalid(
                &fld_path.child("ExcludeCidrs").index(i),
                cidr.clone(),
                "must be a valid CIDR",
            ));
        }
    }
    errs
}

/// `validateKubeProxyLinuxConfiguration` + conntrack (:118-168).
fn validate_linux(config: &InternalLinux, fld_path: &Path) -> ErrorList {
    let mut errs = ErrorList::new();
    let ct_path = fld_path.child("KubeProxyConntrackConfiguration");
    let ct = &config.conntrack;
    if let Some(v) = ct.max_per_core.filter(|v| *v < 0) {
        errs.push(Error::invalid(
            &ct_path.child("MaxPerCore"),
            i64::from(v),
            GE_ZERO,
        ));
    }
    if let Some(v) = ct.min.filter(|v| *v < 0) {
        errs.push(Error::invalid(&ct_path.child("Min"), i64::from(v), GE_ZERO));
    }
    // TCPEstablishedTimeout / TCPCloseWaitTimeout have defaults, so they are
    // not nil after defaulting; an unset (nil) value is treated as 0.
    errs.extend(nonneg(
        ct.tcp_established_timeout.unwrap_or_default(),
        &ct_path.child("TCPEstablishedTimeout"),
    ));
    errs.extend(nonneg(
        ct.tcp_close_wait_timeout.unwrap_or_default(),
        &ct_path.child("TCPCloseWaitTimeout"),
    ));
    errs.extend(nonneg(ct.udp_timeout, &ct_path.child("UDPTimeout")));
    errs.extend(nonneg(
        ct.udp_stream_timeout,
        &ct_path.child("UDPStreamTimeout"),
    ));
    if let Some(v) = config.oom_score_adj.filter(|v| !(-1000..=1000).contains(v)) {
        errs.push(Error::invalid(
            &fld_path.child("OOMScoreAdj"),
            i64::from(v),
            "must be within the range [-1000, 1000]",
        ));
    }
    errs
}

/// `validateProxyMode` (:170-206).
fn validate_proxy_mode(mode: &str, fld_path: &Path, windows: bool) -> ErrorList {
    let (valid, hint): (&[&str], &str) = if windows {
        (
            &[PROXY_MODE_KERNELSPACE],
            "blank means the most-available proxy [currently 'kernelspace']",
        )
    } else {
        (
            &[PROXY_MODE_IPTABLES, PROXY_MODE_IPVS, PROXY_MODE_NFTABLES],
            "blank means the best-available proxy [currently iptables]",
        )
    };
    if mode.is_empty() || valid.contains(&mode) {
        return ErrorList::new();
    }
    let msg = format!("must be {} or blank ({hint})", valid.join(", "));
    vec![Error::invalid(&fld_path.child("ProxyMode"), mode, msg)]
}

/// `validateDetectLocalMode` (:208-222).
fn validate_detect_local_mode(mode: &str, fld_path: &Path) -> ErrorList {
    const VALID: [&str; 5] = [
        LOCAL_MODE_CLUSTER_CIDR,
        LOCAL_MODE_NODE_CIDR,
        LOCAL_MODE_BRIDGE_INTERFACE,
        LOCAL_MODE_INTERFACE_NAME_PREFIX,
        "",
    ];
    if VALID.contains(&mode) {
        return ErrorList::new();
    }
    vec![Error::not_supported(fld_path, mode, &VALID)]
}

/// `validateHostPort` (:233-253). Go's `net.SplitHostPort`.
fn validate_host_port(input: &str, fld_path: &Path) -> ErrorList {
    let split = if let Some(rest) = input.strip_prefix('[') {
        rest.split_once("]:")
    } else if input.matches(':').count() == 1 {
        input.split_once(':')
    } else {
        None
    };
    let Some((host, port)) = split else {
        return vec![Error::invalid(fld_path, input, "must be IP:port")];
    };
    let mut errs = ErrorList::new();
    if parse_ip_sloppy(host).is_none() {
        errs.push(Error::invalid(fld_path, host, "must be a valid IP"));
    }
    match port.parse::<i64>() {
        Ok(p) if (1..=65535).contains(&p) => {}
        _ => errs.push(Error::invalid(fld_path, port, "must be a valid port")),
    }
    errs
}

/// `validateKubeProxyNodePortAddress` (:255-275).
fn validate_node_port_address(addrs: &[String], fld_path: &Path) -> ErrorList {
    let mut errs = ErrorList::new();
    for (i, a) in addrs.iter().enumerate() {
        if a == NODE_PORT_ADDRESSES_PRIMARY {
            if i != 0 || addrs.len() != 1 {
                errs.push(Error::invalid(
                    &fld_path.index(i),
                    a.clone(),
                    "can't use both 'primary' and CIDRs",
                ));
            }
            break;
        }
        if parse_cidr_sloppy(a).is_none() {
            errs.push(Error::invalid(
                &fld_path.index(i),
                a.clone(),
                "must be a valid CIDR",
            ));
        }
    }
    errs
}

/// `validateShowHiddenMetricsVersion` -> `metrics.ValidateShowHiddenMetricsVersion`
/// (component-base/metrics/options.go:110): only the previous minor of the
/// emulated version is allowed.
fn validate_show_hidden_metrics_version(version: &str, fld_path: &Path) -> ErrorList {
    if version.is_empty() {
        return ErrorList::new();
    }
    let current = kubernetes_rs_common::feature_gates::emulation_version();
    let valid = format!("{}.{}", current.major, current.minor.saturating_sub(1));
    if version == valid {
        return ErrorList::new();
    }
    vec![Error::invalid(
        fld_path,
        version,
        format!(
            "--show-hidden-metrics-for-version must be omitted or have the value '{valid}'. Only the previous minor version is allowed"
        ),
    )]
}

/// `validateInterface` (:299-305).
fn validate_interface(iface: &str, fld_path: &Path) -> ErrorList {
    if iface.is_empty() {
        vec![Error::invalid(fld_path, iface, "must not be empty")]
    } else {
        ErrorList::new()
    }
}

/// `validateDualStackCIDRStrings` (:307-332).
fn validate_dual_stack_cidr_strings(cidrs: &[String], fld_path: &Path) -> ErrorList {
    const PAIR_MSG: &str = "must be a either a single CIDR or dual-stack pair of CIDRs (e.g. [10.100.0.0/16, fde4:8dba:82e1::/48]";
    let mut errs = ErrorList::new();
    let shown = format!("{cidrs:?}");
    match cidrs.len() {
        0 => errs.push(Error::invalid(
            fld_path,
            shown,
            "must contain at least one CIDR",
        )),
        1 | 2 => {
            let mut parsed = Vec::new();
            for (i, c) in cidrs.iter().enumerate() {
                match parse_cidr_sloppy(c) {
                    Some(n) => parsed.push(n),
                    None => errs.push(Error::invalid(
                        &fld_path.index(i),
                        c.clone(),
                        "must be a valid CIDR block (e.g. 10.100.0.0/16 or fde4:8dba:82e1::/48)",
                    )),
                }
            }
            // `IsDualStackCIDRStrings` is only consulted when both parsed.
            if cidrs.len() == 2
                && parsed.len() == 2
                && parsed[0].addr.is_ipv6() == parsed[1].addr.is_ipv6()
            {
                errs.push(Error::invalid(fld_path, shown, PAIR_MSG));
            }
        }
        _ => errs.push(Error::invalid(fld_path, shown, PAIR_MSG)),
    }
    errs
}

/// `validateDetectLocalConfiguration` (:334-345).
fn validate_detect_local_configuration(
    mode: &str,
    config: &InternalDetectLocal,
    fld_path: &Path,
) -> ErrorList {
    match mode {
        LOCAL_MODE_BRIDGE_INTERFACE => {
            validate_interface(&config.bridge_interface, &fld_path.child("InterfaceName"))
        }
        LOCAL_MODE_INTERFACE_NAME_PREFIX => validate_interface(
            &config.interface_name_prefix,
            &fld_path.child("InterfacePrefix"),
        ),
        LOCAL_MODE_CLUSTER_CIDR if !config.cluster_cidrs.is_empty() => {
            validate_dual_stack_cidr_strings(&config.cluster_cidrs, &fld_path.child("ClusterCIDRs"))
        }
        _ => ErrorList::new(),
    }
}

/// `logsapi.Validate` (component-base/logs/api/v1/options.go:139-172): flush
/// frequency, supported format and the int32 verbosity bound.
fn validate_logging(c: &LoggingConfiguration, fld_path: &Path) -> ErrorList {
    let mut errs = ErrorList::new();
    if c.flush_frequency.0 <= 0 {
        errs.push(Error::invalid(
            &fld_path.child("flushFrequency"),
            c.flush_frequency.to_string(),
            "Must be greater than zero",
        ));
    }
    if c.format != "text" && c.format != "json" {
        errs.push(Error::invalid(
            &fld_path.child("format"),
            c.format.clone(),
            "Unsupported log format",
        ));
    }
    if c.verbosity > i32::MAX as u32 {
        errs.push(Error::invalid(
            &fld_path.child("verbosity"),
            i64::from(c.verbosity),
            format!("Must be <= {}", i32::MAX),
        ));
    }
    errs
}

// ---------------------------------------------------------------------------
// Loading (cmd/kube-proxy/app/options.go)
// ---------------------------------------------------------------------------

/// `GroupVersion` of the only registered config version
/// (`staging/src/k8s.io/kube-proxy/config/v1alpha1/register.go`).
pub const API_VERSION: &str = "kubeproxy.config.k8s.io/v1alpha1";
pub const KIND: &str = "KubeProxyConfiguration";

/// `Options.loadConfig` (options.go:474-506): decode a serialized
/// `KubeProxyConfiguration` (YAML or JSON) and apply the v1alpha1 defaults the
/// decoder runs. Unknown and duplicate fields are tolerated: upstream retries
/// a strict-decoding failure with the lenient codec (options.go:481-498),
/// which is the only outcome with a single registered version.
pub fn load_config(data: &str) -> Result<KubeProxyConfiguration, String> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(data).map_err(|e| format!("failed to decode: {e}"))?;
    let field = |k: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let (kind, api_version) = (field("kind"), field("apiVersion"));
    if kind.is_empty() {
        return Err("failed to decode: Object 'Kind' is missing".to_string());
    }
    if api_version.is_empty() {
        return Err("failed to decode: Object 'apiVersion' is missing".to_string());
    }
    if api_version != API_VERSION || kind != KIND {
        return Err(format!(
            "failed to decode: no kind \"{kind}\" is registered for version \"{api_version}\" in scheme"
        ));
    }
    let mut cfg: KubeProxyConfiguration =
        serde_yaml::from_value(value).map_err(|e| format!("failed to decode: {e}"))?;
    set_defaults(&mut cfg);
    Ok(cfg)
}

/// `Options.loadConfigFromFile` (options.go:464-471).
pub fn load_config_from_file(path: &std::path::Path) -> Result<KubeProxyConfiguration, String> {
    let data = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    load_config(&data)
}

/// `Options.processHostnameOverrideFlag` (options.go:~430-440): a non-empty
/// flag value is trimmed, lower-cased and wins over the config file.
pub fn process_hostname_override(
    config: &mut KubeProxyConfiguration,
    flag: &str,
) -> Result<(), String> {
    if !flag.is_empty() {
        let host = flag.trim();
        if host.is_empty() {
            return Err("empty hostname-override is invalid".to_string());
        }
        config.hostname_override = host.to_lowercase();
    }
    Ok(())
}

/// `Options.initWatcher` (options.go:~285): watch the config file. A missing
/// file fails here, as `AddWatch` does.
pub struct ConfigFileWatcher {
    events: inotify::EventStream<Vec<u8>>,
}

pub fn init_config_file_watcher(path: &std::path::Path) -> Result<ConfigFileWatcher, String> {
    use inotify::{Inotify, WatchMask};
    let inotify = Inotify::init().map_err(|e| e.to_string())?;
    inotify
        .watches()
        .add(
            path,
            WatchMask::MODIFY
                | WatchMask::CLOSE_WRITE
                | WatchMask::MOVE_SELF
                | WatchMask::DELETE_SELF,
        )
        .map_err(|e| format!("unable to watch {}: {e}", path.display()))?;
    let events = inotify
        .into_event_stream(vec![0u8; 4096])
        .map_err(|e| e.to_string())?;
    Ok(ConfigFileWatcher { events })
}

impl ConfigFileWatcher {
    /// `eventHandler`/`errorHandler`: resolves with upstream's error text on a
    /// write or rename of the file (`runLoop` returns it and the process exits).
    pub async fn changed(mut self) -> String {
        use futures::StreamExt;
        use inotify::EventMask;
        while let Some(ev) = self.events.next().await {
            match ev {
                Ok(ev)
                    if ev.mask.intersects(
                        EventMask::MODIFY
                            | EventMask::CLOSE_WRITE
                            | EventMask::MOVE_SELF
                            | EventMask::DELETE_SELF,
                    ) =>
                {
                    break
                }
                Ok(_) => {}
                Err(e) => return e.to_string(),
            }
        }
        "content of the proxy server's configuration file was updated".to_string()
    }
}
