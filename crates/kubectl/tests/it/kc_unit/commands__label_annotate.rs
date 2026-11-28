//! Ported from `kubectl/pkg/cmd/label/label_test.go` (TestValidateLabels,
//! TestParseLabels, TestLabelFunc, TestLabelMsg) and
//! `annotate/annotate_test.go` (TestValidateAnnotationOverwrites,
//! TestParseAnnotations, TestValidateAnnotations, TestUpdateAnnotations).
use kubernetes_rs_kubectl::commands::annotate::*;
use kubernetes_rs_kubectl::commands::label::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}
fn m(v: &[(&str, &str)]) -> BTreeMap<String, String> {
    v.iter()
        .map(|(k, x)| (k.to_string(), x.to_string()))
        .collect()
}
fn pod(field: &str, kv: &[(&str, &str)]) -> Value {
    let mut meta = json!({"name": "a"});
    if !kv.is_empty() {
        meta[field] = serde_json::to_value(m(kv)).unwrap();
    }
    json!({"metadata": meta})
}

#[test]
fn label_annotate_parse_labels() {
    let (l, r) = parse_labels(&s(&["a=b", "c-"])).unwrap();
    assert_eq!(l, m(&[("a", "b")]));
    assert_eq!(r, s(&["c"]));
    assert_eq!(
        parse_labels(&s(&["a=b=c"])).unwrap_err(),
        "invalid label spec: a=b=c"
    );
    assert_eq!(
        parse_labels(&s(&["ab"])).unwrap_err(),
        "unknown label spec: ab"
    );
    assert_eq!(
        parse_labels(&s(&["a=b", "a-"])).unwrap_err(),
        "can not both modify and remove a label in the same command"
    );
    assert!(parse_labels(&s(&["a=-b"]))
        .unwrap_err()
        .starts_with("invalid label value: \"a=-b\": "));
}

#[test]
fn label_annotate_validate_labels() {
    let obj = pod("labels", &[("a", "b")]);
    assert!(validate_no_overwrites(&obj, &m(&[("a", "b")])).is_ok());
    assert!(validate_no_overwrites(&obj, &m(&[("c", "d")])).is_ok());
    assert_eq!(
        validate_no_overwrites(&obj, &m(&[("a", "c")])).unwrap_err(),
        "'a' already has a value (b), and --overwrite is false"
    );
}

#[test]
fn label_annotate_label_func() {
    let cases: Vec<(Value, Vec<(&str, &str)>, Vec<&str>, bool, &str, Value)> = vec![
        (
            pod("labels", &[]),
            vec![("a", "b")],
            vec![],
            false,
            "",
            pod("labels", &[("a", "b")]),
        ),
        (
            pod("labels", &[("a", "b")]),
            vec![("a", "c")],
            vec![],
            true,
            "",
            pod("labels", &[("a", "c")]),
        ),
        (
            pod("labels", &[("a", "b")]),
            vec![],
            vec!["a"],
            false,
            "",
            pod("labels", &[]),
        ),
        (
            pod("labels", &[("a", "b"), ("c", "d")]),
            vec![],
            vec!["c"],
            false,
            "",
            pod("labels", &[("a", "b")]),
        ),
    ];
    for (mut obj, add, rm, ow, rv, want) in cases {
        label_func(&mut obj, ow, rv, &m(&add), &s(&rm)).unwrap();
        assert_eq!(obj, want);
    }
    let mut obj = pod("labels", &[("a", "b")]);
    assert!(label_func(&mut obj, false, "", &m(&[("a", "c")]), &[]).is_err());
    let mut obj = pod("labels", &[]);
    label_func(&mut obj, false, "7", &m(&[("a", "b")]), &[]).unwrap();
    assert_eq!(obj["metadata"]["resourceVersion"], "7");
}

#[test]
fn label_annotate_label_msg() {
    assert_eq!(
        update_data_change_msg(b"abc", b"abc", false),
        MSG_NOT_LABELED
    );
    assert_eq!(
        update_data_change_msg(b"abcd", b"abc", false),
        MSG_UNLABELED
    );
    assert_eq!(update_data_change_msg(b"abcd", b"abc", true), MSG_LABELED);
    assert_eq!(update_data_change_msg(b"abc", b"abcd", false), MSG_LABELED);
}

#[test]
fn label_annotate_merge_patch() {
    let old = json!({"metadata": {"labels": {"a": "b", "c": "d"}, "name": "x"}});
    let new = json!({"metadata": {"labels": {"a": "z", "e": "f"}, "name": "x"}});
    assert_eq!(
        create_merge_patch(&old, &new),
        json!({"metadata": {"labels": {"a": "z", "c": null, "e": "f"}}})
    );
    // Last label removed: the whole key is dropped.
    let new = json!({"metadata": {"name": "x"}});
    assert_eq!(
        create_merge_patch(&old, &new),
        json!({"metadata": {"labels": null}})
    );
}

#[test]
fn label_annotate_parse_annotations() {
    let (a, r) = parse_annotations(&s(&["a=b", "c=", "d-"])).unwrap();
    assert_eq!(a, m(&[("a", "b"), ("c", "")]));
    assert_eq!(r, s(&["d"]));
    assert_eq!(
        parse_annotations(&s(&["a", "=b"])).unwrap_err(),
        "invalid annotation format: a, =b"
    );
}

#[test]
fn label_annotate_validate_annotations() {
    assert!(validate_annotations(&s(&["a"]), &m(&[("b", "c")])).is_ok());
    assert_eq!(
        validate_annotations(&s(&["a", "b"]), &m(&[("a", "x"), ("b", "y")])).unwrap_err(),
        "can not both modify and remove the following annotation(s) in the same command: a, b"
    );
}

#[test]
fn label_annotate_validate_annotation_overwrites() {
    let obj = pod("annotations", &[("a", "b")]);
    assert!(validate_no_annotation_overwrites(&obj, &m(&[("a", "b")])).is_ok());
    assert!(validate_no_annotation_overwrites(&obj, &m(&[("c", "d")])).is_ok());
    assert_eq!(
        validate_no_annotation_overwrites(&obj, &m(&[("a", "c")])).unwrap_err(),
        "--overwrite is false but found the following declared annotation(s): 'a' already has a value (b)"
    );
    // change-cause is exempt.
    let obj = pod("annotations", &[(CHANGE_CAUSE_ANNOTATION, "old")]);
    assert!(
        validate_no_annotation_overwrites(&obj, &m(&[(CHANGE_CAUSE_ANNOTATION, "new")])).is_ok()
    );
}

#[test]
fn label_annotate_update_annotations() {
    let mut obj = pod("annotations", &[("a", "b")]);
    update_annotations(&mut obj, true, "9", &m(&[("a", "c"), ("d", "e")]), &[]).unwrap();
    assert_eq!(obj["metadata"]["annotations"], json!({"a": "c", "d": "e"}));
    assert_eq!(obj["metadata"]["resourceVersion"], "9");
    update_annotations(&mut obj, false, "", &BTreeMap::new(), &s(&["a", "d"])).unwrap();
    assert!(obj["metadata"].get("annotations").is_none());
    let mut obj = pod("annotations", &[("a", "b")]);
    assert!(update_annotations(&mut obj, false, "", &m(&[("a", "c")]), &[]).is_err());
}
