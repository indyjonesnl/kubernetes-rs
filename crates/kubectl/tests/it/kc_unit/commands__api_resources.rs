//! Unit tests moved from `src/commands/api_resources.rs`.
#![allow(unused_imports, dead_code)]

mod __parent {
    pub use anyhow::Result;
    pub use kubernetes_rs_client::http::ApiClient;
    pub use serde::{Deserialize, Serialize};
    pub use tabled::{settings::Style, Table, Tabled};
}

mod tests {
    use super::__parent::*;
    use kubernetes_rs_kubectl::commands::api_resources::*;

    #[test]
    fn test_api_resource_deserialization() {
        let json = r#"{
            "name": "pods",
            "singularName": "pod",
            "namespaced": true,
            "kind": "Pod",
            "shortNames": ["po"],
            "verbs": ["get", "list", "create", "delete"]
        }"#;
        let resource: ApiResource = serde_json::from_str(json).unwrap();
        assert_eq!(resource.name, "pods");
        assert_eq!(resource.kind, "Pod");
        assert!(resource.namespaced);
        assert_eq!(resource.short_names, vec!["po"]);
    }

    #[test]
    fn test_api_resource_row_construction() {
        let row = ApiResourceRow {
            name: "deployments".to_string(),
            short_names: "deploy".to_string(),
            api_version: "apps/v1".to_string(),
            namespaced: "true".to_string(),
            kind: "Deployment".to_string(),
        };
        assert_eq!(row.name, "deployments");
        assert_eq!(row.api_version, "apps/v1");
    }

    #[test]
    fn test_api_resource_list_deserialization() {
        let json = r#"{
            "groupVersion": "v1",
            "resources": [
                {"name": "pods", "namespaced": true, "kind": "Pod", "verbs": ["get"]},
                {"name": "pods/log", "namespaced": true, "kind": "Pod", "verbs": ["get"]}
            ]
        }"#;
        let list: ApiResourceList = serde_json::from_str(json).unwrap();
        assert_eq!(list.group_version, "v1");
        assert_eq!(list.resources.len(), 2);
        // Subresources (containing /) should be filtered in the execute function
        assert!(list.resources[1].name.contains('/'));
    }

    #[test]
    fn test_api_resource_subresource_filtering() {
        let resources = [
            ("pods", false),
            ("pods/log", true),
            ("pods/status", true),
            ("deployments", false),
            ("deployments/scale", true),
        ];
        let filtered: Vec<_> = resources
            .iter()
            .filter(|(name, _)| !name.contains('/'))
            .collect();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].0, "pods");
        assert_eq!(filtered[1].0, "deployments");
    }

    // ---- ports of apiresources_test.go (TestAPIResourcesValidate/Run/RunJsonYaml) ----

    fn res(
        name: &str,
        namespaced: bool,
        kind: &str,
        verbs: &[&str],
        short: &[&str],
        cats: &[&str],
    ) -> ApiResource {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        ApiResource {
            name: name.into(),
            namespaced,
            kind: kind.into(),
            verbs: v(verbs),
            short_names: v(short),
            categories: v(cats),
            ..Default::default()
        }
    }

    /// `dc.PreferredResources` of `TestAPIResourcesRun`.
    fn preferred() -> Vec<ApiResourceList> {
        vec![
            ApiResourceList {
                group_version: "v1".into(),
                resources: vec![
                    res(
                        "foos",
                        false,
                        "Foo",
                        &["get", "list"],
                        &["f", "fo"],
                        &["some-category"],
                    ),
                    res("bars", true, "Bar", &["get", "list", "create"], &[], &[]),
                ],
            },
            ApiResourceList {
                group_version: "somegroup/v1".into(),
                resources: vec![
                    res(
                        "bazzes",
                        true,
                        "Baz",
                        &["get", "list", "create", "delete"],
                        &["b"],
                        &["some-category", "another-category"],
                    ),
                    res("NoVerbs", true, "NoVerbs", &[], &["b"], &[]),
                ],
            },
            ApiResourceList {
                group_version: "someothergroup/v1".into(),
                resources: vec![],
            },
        ]
    }

    fn run(f: impl FnOnce(&mut ApiResourceOptions)) -> String {
        let mut o = ApiResourceOptions::default();
        f(&mut o);
        o.run_api_resources(&preferred()).unwrap()
    }

    #[test]
    fn api_resources_validate() {
        assert!(ApiResourceOptions::default().validate().is_ok());
        let o = ApiResourceOptions {
            sort_by: "foo".into(),
            ..Default::default()
        };
        assert_eq!(
            o.validate().unwrap_err().to_string(),
            "--sort-by accepts only name or kind"
        );
        let o = ApiResourceOptions {
            output: "foo".into(),
            ..Default::default()
        };
        assert_eq!(
            o.validate_output().unwrap_err().to_string(),
            "unable to match a printer suitable for the output format \"foo\", allowed formats are: json,name,wide,yaml"
        );
    }

    #[test]
    fn api_resources_run_defaults_and_no_headers() {
        let hdr = "NAME     SHORTNAMES   APIVERSION     NAMESPACED   KIND\n";
        assert_eq!(
            run(|_| {}),
            format!(
                "{hdr}bars                  v1             true         Bar\nfoos     f,fo         v1             false        Foo\nbazzes   b            somegroup/v1   true         Baz\n"
            )
        );
        assert_eq!(
            run(|o| o.no_headers = true),
            "bars            v1             true    Bar\nfoos     f,fo   v1             false   Foo\nbazzes   b      somegroup/v1   true    Baz\n"
        );
    }

    #[test]
    fn api_resources_run_filters() {
        let hdr = "NAME     SHORTNAMES   APIVERSION     NAMESPACED   KIND\n";
        let bazzes = "bazzes   b            somegroup/v1   true         Baz\n";
        let bars = "bars                  v1             true         Bar\n";
        let foos = "foos     f,fo         v1             false        Foo\n";
        assert_eq!(
            run(|o| {
                o.api_group = "somegroup".into();
                o.group_changed = true;
            }),
            format!("{hdr}{bazzes}")
        );
        assert_eq!(
            run(|o| {
                o.namespaced = true;
                o.ns_changed = true;
            }),
            format!("{hdr}{bars}{bazzes}")
        );
        assert_eq!(
            run(|o| o.verbs = vec!["create".into()]),
            format!("{hdr}{bars}{bazzes}")
        );
        assert_eq!(
            run(|o| o.verbs = vec!["create".into(), "delete".into()]),
            format!("{hdr}{bazzes}")
        );
        assert_eq!(
            run(|o| o.categories = vec!["some-category".into()]),
            format!("{hdr}{foos}{bazzes}")
        );
        assert_eq!(
            run(|o| o.categories = vec!["some-category".into(), "another-category".into()]),
            format!("{hdr}{bazzes}")
        );
    }

    #[test]
    fn api_resources_run_sort_wide_name_cached() {
        let hdr = "NAME     SHORTNAMES   APIVERSION     NAMESPACED   KIND\n";
        let sorted = format!(
            "{hdr}bars                  v1             true         Bar\nbazzes   b            somegroup/v1   true         Baz\nfoos     f,fo         v1             false        Foo\n"
        );
        assert_eq!(run(|o| o.sort_by = "name".into()), sorted);
        assert_eq!(run(|o| o.sort_by = "kind".into()), sorted);
        assert_eq!(
            run(|o| o.output = "wide".into()),
            "NAME     SHORTNAMES   APIVERSION     NAMESPACED   KIND   VERBS                    CATEGORIES\nbars                  v1             true         Bar    get,list,create          \nfoos     f,fo         v1             false        Foo    get,list                 some-category\nbazzes   b            somegroup/v1   true         Baz    get,list,create,delete   some-category,another-category\n"
        );
        assert_eq!(
            run(|o| o.output = "name".into()),
            "bars\nfoos\nbazzes.somegroup\n"
        );
        // `--cached` only skips the cache invalidation: same output.
        assert_eq!(run(|o| o.cached = true), run(|_| {}));
    }

    #[test]
    fn api_resources_run_json_yaml_roundtrip() {
        let foo = res(
            "foos",
            false,
            "Foo",
            &["get", "list"],
            &["f", "fo"],
            &["some-category"],
        );
        let lists = vec![ApiResourceList {
            group_version: "somegroup/v1".into(),
            resources: vec![foo.clone()],
        }];
        for fmt in ["json", "yaml"] {
            let o = ApiResourceOptions {
                output: fmt.into(),
                ..Default::default()
            };
            let out = o.run_api_resources(&lists).unwrap();
            let v: serde_json::Value = if fmt == "json" {
                serde_json::from_str(&out).unwrap()
            } else {
                serde_yaml::from_str(&out).unwrap()
            };
            let mut got: ApiResource = serde_json::from_value(v["resources"][0].clone()).unwrap();
            // Undo the Group/Version set by RunAPIResources.
            assert_eq!(
                (got.group.as_str(), got.version.as_str()),
                ("somegroup", "v1")
            );
            got.group.clear();
            got.version.clear();
            assert_eq!(got, foo, "{fmt}");
        }
    }
}
