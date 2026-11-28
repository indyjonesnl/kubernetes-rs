//! `kubectl label`.
//!
//! Port of `staging/src/k8s.io/kubectl/pkg/cmd/label/label.go` (release-1.35):
//! `parseLabels` (:423), `validateNoOverwrites` (:411), `labelFunc` (:446),
//! `updateDataChangeMsg` (:401) and the patch step of `RunLabel` (:262-350):
//! the object is read, edited by `labelFunc`, and a JSON merge patch of the
//! difference is sent. The flags that need the resource builder (`-f`, `-R`,
//! `-l`, `--all`, `-A`, `--field-selector`, `--local`, `--dry-run`, `-o`,
//! `--resource-version`, `--list`, several resources) are backlog
//! kubectl-P2-35b.

use anyhow::{Context, Result};
use kubernetes_rs_client::http::ApiClient;
use kubernetes_rs_common::validation::metav1::is_valid_label_value;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// `MsgNotLabeled` / `MsgLabeled` / `MsgUnLabeled` (label.go:50-52).
pub const MSG_NOT_LABELED: &str = "not labeled";
pub const MSG_LABELED: &str = "labeled";
pub const MSG_UNLABELED: &str = "unlabeled";

/// `parseLabels` (label.go:423): `(labels to set, labels to remove)`.
pub fn parse_labels(spec: &[String]) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let mut labels = BTreeMap::new();
    let mut remove = Vec::new();
    for label_spec in spec {
        if label_spec.contains('=') {
            let parts: Vec<&str> = label_spec.split('=').collect();
            if parts.len() != 2 {
                return Err(format!("invalid label spec: {label_spec}"));
            }
            let errs = is_valid_label_value(parts[1]);
            if !errs.is_empty() {
                return Err(format!(
                    "invalid label value: {label_spec:?}: {}",
                    errs.join(";")
                ));
            }
            labels.insert(parts[0].to_string(), parts[1].to_string());
        } else if let Some(key) = label_spec.strip_suffix('-') {
            remove.push(key.to_string());
        } else {
            return Err(format!("unknown label spec: {label_spec}"));
        }
    }
    for remove_label in &remove {
        if labels.contains_key(remove_label) {
            return Err("can not both modify and remove a label in the same command".into());
        }
    }
    Ok((labels, remove))
}

/// The `metadata.<field>` string map of an object (`accessor.GetLabels()`).
pub(crate) fn string_map(obj: &Value, field: &str) -> BTreeMap<String, String> {
    obj.pointer(&format!("/metadata/{field}"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// `accessor.SetLabels` / `SetAnnotations`: an empty map is dropped from the
/// serialised object (`omitempty`).
pub(crate) fn set_string_map(obj: &mut Value, field: &str, map: BTreeMap<String, String>) {
    let meta = obj
        .as_object_mut()
        .map(|o| o.entry("metadata").or_insert_with(|| json!({})));
    if let Some(Value::Object(m)) = meta {
        if map.is_empty() {
            m.remove(field);
        } else {
            let v: Map<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            m.insert(field.to_string(), Value::Object(v));
        }
    }
}

/// `errors.NewAggregate(...).Error()`: one message as is, several as `[a, b]`.
pub(crate) fn aggregate(errs: Vec<String>) -> Option<String> {
    match errs.len() {
        0 => None,
        1 => Some(errs.into_iter().next().unwrap()),
        _ => Some(format!("[{}]", errs.join(", "))),
    }
}

/// `validateNoOverwrites` (label.go:411).
pub fn validate_no_overwrites(
    obj: &Value,
    labels: &BTreeMap<String, String>,
) -> Result<(), String> {
    let current = string_map(obj, "labels");
    let errs: Vec<String> = labels
        .iter()
        .filter_map(|(k, v)| match current.get(k) {
            Some(cur) if cur != v => Some(format!(
                "'{k}' already has a value ({cur}), and --overwrite is false"
            )),
            _ => None,
        })
        .collect();
    aggregate(errs).map_or(Ok(()), Err)
}

/// `labelFunc` (label.go:446).
pub fn label_func(
    obj: &mut Value,
    overwrite: bool,
    resource_version: &str,
    labels: &BTreeMap<String, String>,
    remove: &[String],
) -> Result<(), String> {
    if !overwrite {
        validate_no_overwrites(obj, labels)?;
    }
    let mut obj_labels = string_map(obj, "labels");
    for (k, v) in labels {
        obj_labels.insert(k.clone(), v.clone());
    }
    for label in remove {
        obj_labels.remove(label);
    }
    set_string_map(obj, "labels", obj_labels);
    if !resource_version.is_empty() {
        if let Some(Value::Object(m)) = obj.get_mut("metadata") {
            m.insert(
                "resourceVersion".into(),
                Value::String(resource_version.into()),
            );
        }
    }
    Ok(())
}

/// `updateDataChangeMsg` (label.go:401): compares the serialised objects.
pub fn update_data_change_msg(old: &[u8], new: &[u8], overwrite: bool) -> &'static str {
    if old == new {
        return MSG_NOT_LABELED;
    }
    if !overwrite && new.len() < old.len() {
        MSG_UNLABELED
    } else {
        MSG_LABELED
    }
}

/// `jsonpatch.CreateMergePatch(old, new)` (RFC 7386): changed or added keys
/// carry the new value, removed keys are `null`, objects recurse, anything
/// else (arrays included) is replaced.
pub fn create_merge_patch(old: &Value, new: &Value) -> Value {
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            let mut patch = Map::new();
            for (k, nv) in n {
                match o.get(k) {
                    None => {
                        patch.insert(k.clone(), nv.clone());
                    }
                    Some(ov) if ov != nv => {
                        patch.insert(k.clone(), create_merge_patch(ov, nv));
                    }
                    _ => {}
                }
            }
            for k in o.keys() {
                if !n.contains_key(k) {
                    patch.insert(k.clone(), Value::Null);
                }
            }
            Value::Object(patch)
        }
        _ => new.clone(),
    }
}

/// The API path pieces and name-printer prefix for a resource type.
pub(crate) struct Target {
    pub api_path: &'static str,
    pub resource: &'static str,
    /// `pod`, `deployment.apps`: what the name printer shows before `/name`.
    pub print_name: &'static str,
}

/// The resource types this command resolves.
pub(crate) fn resolve_target(resource_type: &str) -> Result<Target> {
    let t = |api_path, resource, print_name| Target {
        api_path,
        resource,
        print_name,
    };
    Ok(match resource_type {
        "pod" | "pods" => t("api/v1", "pods", "pod"),
        "service" | "services" | "svc" => t("api/v1", "services", "service"),
        "deployment" | "deployments" | "deploy" => {
            t("apis/apps/v1", "deployments", "deployment.apps")
        }
        "daemonset" | "daemonsets" | "ds" => t("apis/apps/v1", "daemonsets", "daemonset.apps"),
        "statefulset" | "statefulsets" | "sts" => {
            t("apis/apps/v1", "statefulsets", "statefulset.apps")
        }
        "replicaset" | "replicasets" | "rs" => t("apis/apps/v1", "replicasets", "replicaset.apps"),
        "configmap" | "configmaps" | "cm" => t("api/v1", "configmaps", "configmap"),
        "secret" | "secrets" => t("api/v1", "secrets", "secret"),
        "node" | "nodes" => t("api/v1", "nodes", "node"),
        _ => anyhow::bail!("Unsupported resource type: {}", resource_type),
    })
}

pub(crate) fn object_path(t: &Target, namespace: &str, name: &str) -> String {
    if t.resource == "nodes" {
        format!("/{}/{}/{}", t.api_path, t.resource, name)
    } else {
        format!(
            "/{}/namespaces/{}/{}/{}",
            t.api_path, namespace, t.resource, name
        )
    }
}

/// Update labels on a resource (`RunLabel`, label.go:262, for one named object).
pub async fn execute(
    client: &ApiClient,
    resource_type: &str,
    name: &str,
    namespace: &str,
    labels: &[String],
    overwrite: bool,
) -> Result<()> {
    let (new_labels, remove_labels) = parse_labels(labels).map_err(anyhow::Error::msg)?;
    // Validate (label.go:213): at least one update.
    if new_labels.is_empty() && remove_labels.is_empty() {
        anyhow::bail!("at least one label update is required");
    }
    let target = resolve_target(resource_type)?;
    let path = object_path(&target, namespace, name);

    let mut obj: Value = client
        .get(&path)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("Failed to get {}/{name}", target.print_name))?;
    let old_data = serde_json::to_vec(&obj)?;
    let old = obj.clone();

    let current = string_map(&obj, "labels");
    for label in &remove_labels {
        if !current.contains_key(label) {
            println!("label {label:?} not found.");
        }
    }
    label_func(&mut obj, overwrite, "", &new_labels, &remove_labels).map_err(anyhow::Error::msg)?;
    let new_data = serde_json::to_vec(&obj)?;
    let msg = update_data_change_msg(&old_data, &new_data, overwrite);

    let patch = create_merge_patch(&old, &obj);
    let _: Value = client
        .patch(&path, &patch, "application/merge-patch+json")
        .await
        .context("Failed to update labels")?;

    println!("{}/{} {}", target.print_name, name, msg);
    Ok(())
}
