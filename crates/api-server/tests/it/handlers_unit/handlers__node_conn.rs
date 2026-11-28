//! Unit tests moved from `src/handlers/node_conn.rs`.
#![allow(unused_imports, dead_code)]

mod __parent {
    pub use kubernetes_rs_common::resources::Node;
}

mod tests {
    use super::__parent::*;
    use kubernetes_rs_api_server::handlers::node_conn::*;
    use kubernetes_rs_common::resources::{
        DaemonEndpoint, Node, NodeAddress, NodeDaemonEndpoints, NodeStatus,
    };

    fn make_node(addresses: Vec<NodeAddress>, kubelet_port: Option<i32>) -> Node {
        let mut node = Node::new("test-node");
        node.status = Some(NodeStatus {
            addresses: Some(addresses),
            daemon_endpoints: kubelet_port.map(|p| NodeDaemonEndpoints {
                kubelet_endpoint: Some(DaemonEndpoint { port: p }),
            }),
            ..Default::default()
        });
        node
    }

    #[test]
    fn prefers_internal_ip_and_uses_advertised_port() {
        let node = make_node(
            vec![
                NodeAddress {
                    address_type: "ExternalIP".to_string(),
                    address: "1.2.3.4".to_string(),
                },
                NodeAddress {
                    address_type: "InternalIP".to_string(),
                    address: "192.168.1.10".to_string(),
                },
            ],
            Some(10255),
        );

        let conn = node_conn(&node, None).expect("should resolve");
        assert_eq!(conn.host, "192.168.1.10");
        assert_eq!(conn.port, 10255);
        assert_eq!(conn.scheme, "https");
    }

    #[test]
    fn falls_back_to_external_ip_when_no_internal() {
        let node = make_node(
            vec![NodeAddress {
                address_type: "ExternalIP".to_string(),
                address: "5.6.7.8".to_string(),
            }],
            None,
        );

        let conn = node_conn(&node, None).expect("should resolve");
        assert_eq!(conn.host, "5.6.7.8");
        assert_eq!(conn.port, 10250); // default
    }

    #[test]
    fn port_override_wins_over_advertised_port() {
        let node = make_node(
            vec![NodeAddress {
                address_type: "InternalIP".to_string(),
                address: "10.0.0.1".to_string(),
            }],
            Some(10255),
        );

        let conn = node_conn(&node, Some(9090)).expect("should resolve");
        assert_eq!(conn.port, 9090);
    }

    #[test]
    fn no_address_returns_plain_error() {
        let node = make_node(vec![], None);
        let err = node_conn(&node, None).unwrap_err();
        assert!(
            matches!(err, kubernetes_rs_common::Error::Internal(_)),
            "expected Internal, got: {err:?}"
        );
    }

    #[test]
    fn no_status_returns_plain_error() {
        let node = Node::new("bare-node");
        let err = node_conn(&node, None).unwrap_err();
        assert!(matches!(err, kubernetes_rs_common::Error::Internal(_)));
    }

    #[test]
    fn scheme_is_always_https() {
        let node = make_node(
            vec![NodeAddress {
                address_type: "InternalIP".to_string(),
                address: "10.0.0.2".to_string(),
            }],
            None,
        );
        let conn = node_conn(&node, None).expect("should resolve");
        assert_eq!(conn.scheme, "https");
    }

    /// Port of `TestGetPreferredAddress` (`pkg/util/node/node_test.go:31`).
    #[test]
    fn preferred_address_cases() {
        let a = |t: &str, v: &str| NodeAddress {
            address_type: t.to_string(),
            address: v.to_string(),
        };
        let get = |addrs: Vec<NodeAddress>, prefs: &[&str]| {
            let mut node = make_node(addrs, None);
            // "label address ignored": the hostname label is not an address.
            node.metadata
                .labels
                .get_or_insert_with(Default::default)
                .insert("kubernetes.io/hostname".into(), "label-hostname".into());
            get_preferred_node_address(&node, prefs)
        };
        assert_eq!(
            get(vec![], &[]).unwrap_err().to_string(),
            "no preferred addresses found; known addresses: []"
        );
        assert_eq!(
            get(vec![a("InternalIP", "1.2.3.4")], &["Hostname"])
                .unwrap_err()
                .to_string(),
            "no preferred addresses found; known addresses: [{InternalIP 1.2.3.4}]"
        );
        assert_eq!(
            get(
                vec![
                    a("InternalIP", "1.2.3.4"),
                    a("ExternalIP", "1.2.3.5"),
                    a("ExternalIP", "1.2.3.7")
                ],
                &["Hostname", "ExternalIP"]
            )
            .unwrap(),
            "1.2.3.5"
        );
        assert_eq!(
            get(
                vec![a("ExternalIP", "1.2.3.5"), a("Hostname", "status-hostname")],
                &["Hostname", "ExternalIP"]
            )
            .unwrap(),
            "status-hostname"
        );
        assert_eq!(
            get(
                vec![a("ExternalIP", "1.2.3.5")],
                &["Hostname", "ExternalIP"]
            )
            .unwrap(),
            "1.2.3.5"
        );
    }

    /// The default order reaches nodes that report only a hostname or DNS
    /// name (`options.go:75-85`), which InternalIP/ExternalIP alone did not.
    #[test]
    fn preferred_address_default_order_in_node_conn() {
        let a = |t: &str, v: &str| NodeAddress {
            address_type: t.to_string(),
            address: v.to_string(),
        };
        let node = make_node(vec![a("InternalIP", "10.0.0.1"), a("Hostname", "h")], None);
        assert_eq!(node_conn(&node, None).unwrap().host, "h");
        let node = make_node(
            vec![a("ExternalDNS", "x.example"), a("ExternalIP", "1.1.1.1")],
            None,
        );
        assert_eq!(node_conn(&node, None).unwrap().host, "x.example");
        let node = make_node(vec![a("InternalDNS", "n.local")], None);
        assert_eq!(node_conn(&node, None).unwrap().host, "n.local");
    }

    /// `--kubelet-preferred-address-types` reorders the lookup; no match is
    /// the plain NoMatchError text (`pkg/util/node/node.go:36-44`).
    #[test]
    fn preferred_address_flag_order_applies() {
        let a = |t: &str, v: &str| NodeAddress {
            address_type: t.to_string(),
            address: v.to_string(),
        };
        let node = make_node(
            vec![a("InternalIP", "10.0.0.1"), a("ExternalIP", "1.1.1.1")],
            None,
        );
        let c = node_conn_with_types(&node, None, &["ExternalIP", "InternalIP"]).unwrap();
        assert_eq!(c.host, "1.1.1.1");
        let err = node_conn_with_types(&node, None, &["Hostname"]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Internal error: no preferred addresses found; known addresses: [{InternalIP 10.0.0.1} {ExternalIP 1.1.1.1}]"
        );
    }
}
