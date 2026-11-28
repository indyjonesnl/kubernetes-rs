//! Ports of `TestValidateLimitRange` / `TestValidateLimitRangeForLocalStorage`
//! (`pkg/apis/core/validation/validation_test.go:20936-21400`, release-1.35)
//! cases for `validateLimitRangeTypeName` and
//! `validateLimitRangeResourceName`.

use kubernetes_rs_common::resources::policy::LimitRange;
use kubernetes_rs_common::validation::limitrange::validate_limit_range;
use serde_json::{json, Value};

fn lr(limits: Value) -> LimitRange {
    serde_json::from_value(json!({
        "apiVersion": "v1", "kind": "LimitRange",
        "metadata": {"name": "abc", "namespace": "foo"},
        "spec": {"limits": limits}
    }))
    .unwrap()
}

fn msgs(limits: Value) -> Vec<(String, String)> {
    validate_limit_range(&lr(limits))
        .into_iter()
        .map(|e| (e.field, e.detail))
        .collect()
}

#[test]
fn limitrange_validation_third_party_type_with_standard_and_storage_resources() {
    // thirdparty-fields-all-valid-standard-container-resources
    assert!(msgs(json!([{
        "type": "thirdparty.com/foo",
        "max": {"cpu": "100m", "memory": "10000T"},
        "min": {"cpu": "5m", "memory": "100Mi"},
        "default": {"cpu": "50m", "memory": "500Mi"},
        "defaultRequest": {"cpu": "10m", "memory": "200Mi"},
        "maxLimitRequestRatio": {"cpu": "10"}
    }]))
    .is_empty());
    // thirdparty-fields-all-valid-storage-resources
    assert!(msgs(json!([{
        "type": "thirdparty.com/foo",
        "max": {"storage": "10000T"}, "min": {"storage": "100Mi"},
        "default": {"storage": "500Mi"}, "defaultRequest": {"storage": "200Mi"}
    }]))
    .is_empty());
}

#[test]
fn limitrange_validation_local_storage_for_pod_and_container() {
    // TestValidateLimitRangeForLocalStorage: ephemeral-storage is a standard
    // container resource.
    assert!(msgs(json!([
        {"type": "Pod", "max": {"ephemeral-storage": "10000Mi"}, "min": {"ephemeral-storage": "100Mi"}},
        {"type": "Container", "max": {"ephemeral-storage": "10000Mi"},
         "min": {"ephemeral-storage": "100Mi"}, "default": {"ephemeral-storage": "500Mi"},
         "defaultRequest": {"ephemeral-storage": "200Mi"}}
    ]))
    .is_empty());
}

#[test]
fn limitrange_validation_invalid_non_standard_limit_type() {
    let m = msgs(json!([{
        "type": "foo",
        "max": {"storage": "10000T"}, "min": {"storage": "100Mi"},
        "default": {"storage": "500Mi"}, "defaultRequest": {"storage": "200Mi"}
    }]));
    assert!(
        m.iter().any(|(f, d)| f == "spec.limits[0].type"
            && d == "must be a standard limit type or fully qualified"),
        "{m:?}"
    );
}

#[test]
fn limitrange_validation_type_must_be_a_qualified_name() {
    let m = msgs(json!([{"type": "bad type!", "max": {"cpu": "1"}}]));
    assert!(m.iter().any(|(f, _)| f == "spec.limits[0].type"), "{m:?}");
    // A qualified-name failure short-circuits the "standard type" message.
    assert!(
        !m.iter().any(|(_, d)| d.contains("standard limit type")),
        "{m:?}"
    );
}

#[test]
fn limitrange_validation_resource_names_follow_the_limit_type() {
    // Container: unprefixed names must be standard container resources.
    let m = msgs(json!([{"type": "Container", "max": {"storage": "1Gi"}}]));
    assert!(
        m.iter().any(|(f, d)| f == "spec.limits[0].max[storage]"
            && d == "must be a standard resource for containers"),
        "{m:?}"
    );
    // A non-standard unprefixed name, in every map.
    for map in [
        "max",
        "min",
        "default",
        "defaultRequest",
        "maxLimitRequestRatio",
    ] {
        let m = msgs(json!([{"type": "Container", map: {"bogus": "1"}}]));
        assert!(
            m.iter()
                .any(|(f, d)| f == &format!("spec.limits[0].{map}[bogus]")
                    && d == "must be a standard resource type or fully qualified"),
            "{map}: {m:?}"
        );
    }
    // Container: a prefixed non-extended name.
    let m = msgs(json!([{"type": "Container", "max": {"kubernetes.io/x": "1"}}]));
    assert!(m.is_empty(), "native is fine: {m:?}");
    // Other types take any valid, standard-or-qualified resource name.
    let m = msgs(json!([{"type": "example.com/t", "max": {"bogus": "1"}}]));
    assert!(
        m.iter().any(|(f, _)| f == "spec.limits[0].max[bogus]"),
        "{m:?}"
    );
    let m = msgs(json!([{"type": "example.com/t", "max": {"example.com/gpu": "1"}}]));
    assert!(m.is_empty(), "{m:?}");
}

#[test]
fn limitrange_validation_pod_type_skips_default_name_checks() {
    // Pod: default/defaultRequest are Forbidden, their names are not checked.
    let m = msgs(json!([{"type": "Pod", "default": {"bogus": "1"}}]));
    assert!(
        m.iter().all(|(f, _)| !f.contains("default[bogus]")),
        "{m:?}"
    );
}
