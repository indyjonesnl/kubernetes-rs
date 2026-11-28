//! `kubectl annotate`.
//!
//! Port of `staging/src/k8s.io/kubectl/pkg/cmd/annotate/annotate.go`
//! (release-1.35): `parseAnnotations` (`cmdutil.ParsePairs`),
//! `validateAnnotations` (:445), `validateNoAnnotationOverwrites` (:459),
//! `updateAnnotations` (:477) and the patch step of `RunAnnotate`. The flags
//! that need the resource builder are backlog kubectl-P2-35b.

use anyhow::{Context, Result};
use kubernetes_rs_client::http::ApiClient;
use serde_json::Value;
use std::collections::BTreeMap;

use super::label::{create_merge_patch, object_path, resolve_target, set_string_map, string_map};

/// `polymorphichelpers.ChangeCauseAnnotation` (history.go:45).
pub const CHANGE_CAUSE_ANNOTATION: &str = "kubernetes.io/change-cause";

/// `cmdutil.ParsePairs(args, "annotation", true)` (helpers.go:778).
pub fn parse_annotations(
    args: &[String],
) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let mut new_pairs = BTreeMap::new();
    let mut remove = Vec::new();
    let mut invalid: Vec<&str> = Vec::new();
    for arg in args {
        if arg.contains('=') && !arg.starts_with('=') {
            let (k, v) = arg.split_once('=').unwrap();
            new_pairs.insert(k.to_string(), v.to_string());
        } else if arg.ends_with('-') && arg != "-" {
            remove.push(arg[..arg.len() - 1].to_string());
        } else {
            invalid.push(arg);
        }
    }
    if !invalid.is_empty() {
        return Err(format!("invalid annotation format: {}", invalid.join(", ")));
    }
    Ok((new_pairs, remove))
}

/// `validateAnnotations` (annotate.go:445).
pub fn validate_annotations(
    remove: &[String],
    new_annotations: &BTreeMap<String, String>,
) -> Result<(), String> {
    let both: Vec<&str> = remove
        .iter()
        .filter(|r| new_annotations.contains_key(*r))
        .map(String::as_str)
        .collect();
    if both.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "can not both modify and remove the following annotation(s) in the same command: {}",
            both.join(", ")
        ))
    }
}

/// `validateNoAnnotationOverwrites` (annotate.go:459).
pub fn validate_no_annotation_overwrites(
    obj: &Value,
    annotations: &BTreeMap<String, String>,
) -> Result<(), String> {
    let current = string_map(obj, "annotations");
    let found: Vec<String> = annotations
        .iter()
        .filter(|(k, _)| k.as_str() != CHANGE_CAUSE_ANNOTATION)
        .filter_map(|(k, v)| match current.get(k) {
            Some(cur) if cur != v => Some(format!("'{k}' already has a value ({cur})")),
            _ => None,
        })
        .collect();
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "--overwrite is false but found the following declared annotation(s): {}",
            found.join("; ")
        ))
    }
}

/// `AnnotateOptions.updateAnnotations` (annotate.go:477).
pub fn update_annotations(
    obj: &mut Value,
    overwrite: bool,
    resource_version: &str,
    new_annotations: &BTreeMap<String, String>,
    remove: &[String],
) -> Result<(), String> {
    if !overwrite {
        validate_no_annotation_overwrites(obj, new_annotations)?;
    }
    let mut annotations = string_map(obj, "annotations");
    for (k, v) in new_annotations {
        annotations.insert(k.clone(), v.clone());
    }
    for a in remove {
        annotations.remove(a);
    }
    set_string_map(obj, "annotations", annotations);
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

/// Update annotations on a resource (`RunAnnotate`, for one named object).
pub async fn execute(
    client: &ApiClient,
    resource_type: &str,
    name: &str,
    namespace: &str,
    annotations: &[String],
    overwrite: bool,
) -> Result<()> {
    let (new_annotations, remove) = parse_annotations(annotations).map_err(anyhow::Error::msg)?;
    // Validate (annotate.go:276): at least one update, no modify+remove clash.
    if new_annotations.is_empty() && remove.is_empty() {
        anyhow::bail!("at least one annotation update is required");
    }
    validate_annotations(&remove, &new_annotations).map_err(anyhow::Error::msg)?;
    let target = resolve_target(resource_type)?;
    let path = object_path(&target, namespace, name);

    let mut obj: Value = client
        .get(&path)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("Failed to get {}/{name}", target.print_name))?;
    let old = obj.clone();
    update_annotations(&mut obj, overwrite, "", &new_annotations, &remove)
        .map_err(anyhow::Error::msg)?;

    let patch = create_merge_patch(&old, &obj);
    let _: Value = client
        .patch(&path, &patch, "application/merge-patch+json")
        .await
        .context("Failed to update annotations")?;

    println!("{}/{} annotated", target.print_name, name);
    Ok(())
}
