use anyhow::Result;
use kubernetes_rs_client::http::ApiClient;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiGroupList {
    groups: Vec<ApiGroup>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiGroup {
    preferred_version: GroupVersionForDiscovery,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupVersionForDiscovery {
    pub group_version: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApiResourceList {
    pub group_version: String,
    pub resources: Vec<ApiResource>,
}

/// `metav1.APIResource`.
#[derive(Deserialize, Serialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApiResource {
    pub name: String,
    #[serde(default)]
    pub singular_name: String,
    pub namespaced: bool,
    /// Set by `run_api_resources` from the list's group/version (the
    /// APIVERSION column concatenates them).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    pub kind: String,
    #[serde(default)]
    pub verbs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub short_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub storage_version_hash: String,
}

#[derive(Tabled, Serialize)]
pub struct ApiResourceRow {
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "SHORTNAMES")]
    pub short_names: String,
    #[tabled(rename = "APIVERSION")]
    pub api_version: String,
    #[tabled(rename = "NAMESPACED")]
    pub namespaced: String,
    #[tabled(rename = "KIND")]
    pub kind: String,
}

/// `APIResourceOptions` (`staging/src/k8s.io/kubectl/pkg/cmd/apiresources/apiresources.go`).
#[derive(Debug, Clone)]
pub struct ApiResourceOptions {
    pub sort_by: String,
    pub api_group: String,
    pub namespaced: bool,
    pub verbs: Vec<String>,
    /// Upstream reads the on-disk discovery cache when set; this client has
    /// none, so it only decides whether the cache is invalidated (a no-op).
    pub cached: bool,
    pub categories: Vec<String>,
    /// `cmd.Flags().Changed("api-group")`.
    pub group_changed: bool,
    /// `cmd.Flags().Changed("namespaced")`.
    pub ns_changed: bool,
    /// `--output`: "", "wide", "name", "json" or "yaml".
    pub output: String,
    pub no_headers: bool,
}

impl Default for ApiResourceOptions {
    /// `NewAPIResourceOptions`: `Namespaced: true`.
    fn default() -> Self {
        Self {
            sort_by: String::new(),
            api_group: String::new(),
            namespaced: true,
            verbs: Vec::new(),
            cached: false,
            categories: Vec::new(),
            group_changed: false,
            ns_changed: false,
            output: String::new(),
            no_headers: false,
        }
    }
}

impl ApiResourceOptions {
    /// `Validate` (apiresources.go).
    pub fn validate(&self) -> Result<()> {
        if !self.sort_by.is_empty() && self.sort_by != "name" && self.sort_by != "kind" {
            anyhow::bail!("--sort-by accepts only name or kind");
        }
        Ok(())
    }

    /// The printer lookup of `Complete` (`PrintFlags.ToPrinter`, print_flags.go):
    /// an unknown output format is an error listing the sorted allowed formats
    /// (`NoCompatiblePrinterError.Error`).
    pub fn validate_output(&self) -> Result<()> {
        match self.output.as_str() {
            "" | "wide" | "name" | "json" | "yaml" => Ok(()),
            other => anyhow::bail!(
                "unable to match a printer suitable for the output format {other:?}, allowed formats are: json,name,wide,yaml"
            ),
        }
    }

    /// `RunAPIResources` (apiresources.go) over the server's preferred
    /// resources: filter, flatten, stable sort, print. Returns the text that
    /// upstream writes to its tab writer.
    pub fn run_api_resources(&self, lists: &[ApiResourceList]) -> Result<String> {
        let mut flat: Vec<ApiResource> = Vec::new();
        for list in lists {
            if list.resources.is_empty() {
                continue;
            }
            // `schema.ParseGroupVersion`: "v1" is the core group.
            let (group, version) = match list.group_version.split_once('/') {
                None => (String::new(), list.group_version.clone()),
                Some((g, v)) if !v.contains('/') => (g.to_string(), v.to_string()),
                Some(_) => continue,
            };
            for resource in &list.resources {
                if resource.verbs.is_empty() {
                    continue;
                }
                // filter apiGroup
                if self.group_changed && self.api_group != group {
                    continue;
                }
                // filter namespaced
                if self.ns_changed && self.namespaced != resource.namespaced {
                    continue;
                }
                // filter to resources that support the specified verbs
                if !self.verbs.iter().all(|v| resource.verbs.contains(v)) {
                    continue;
                }
                // filter to resources that belong to the specified categories
                if !self
                    .categories
                    .iter()
                    .all(|c| resource.categories.contains(c))
                {
                    continue;
                }
                let mut r = resource.clone();
                r.group = group.clone();
                r.version = version.clone();
                flat.push(r);
            }
        }
        if flat.is_empty() {
            return Ok(String::new());
        }
        // `sort.Stable(sortableResource{..})`: by the sort key (group by
        // default), ties by name.
        let key = |r: &ApiResource| match self.sort_by.as_str() {
            "name" => r.name.clone(),
            "kind" => r.kind.clone(),
            _ => r.group.clone(),
        };
        flat.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.name.cmp(&b.name)));
        self.print_obj(&flat)
    }

    fn print_obj(&self, flat: &[ApiResource]) -> Result<String> {
        match self.output.as_str() {
            "json" | "yaml" => {
                let list = serde_json::json!({
                    "apiVersion": "v1",
                    "kind": "APIResourceList",
                    "groupVersion": "",
                    "resources": flat,
                });
                return Ok(if self.output == "json" {
                    format!("{}\n", serde_json::to_string_pretty(&list)?)
                } else {
                    serde_yaml::to_string(&list)?
                });
            }
            "name" => {
                // `NamePrinter.PrintObj`.
                let mut out = String::new();
                for r in flat {
                    out.push_str(&r.name);
                    if !r.group.is_empty() {
                        out.push('.');
                        out.push_str(&r.group);
                    }
                    out.push('\n');
                }
                return Ok(out);
            }
            _ => {}
        }
        // `HumanReadablePrinter.PrintObj` with `printContextHeaders`.
        let wide = self.output == "wide";
        let mut out = String::new();
        if !self.no_headers {
            let mut cols = vec!["NAME", "SHORTNAMES", "APIVERSION", "NAMESPACED", "KIND"];
            if wide {
                cols.extend(["VERBS", "CATEGORIES"]);
            }
            out.push_str(&cols.join("\t"));
            out.push('\n');
        }
        for r in flat {
            let gv = if r.group.is_empty() {
                r.version.clone()
            } else {
                format!("{}/{}", r.group, r.version)
            };
            let mut cells = vec![
                r.name.clone(),
                r.short_names.join(","),
                gv,
                r.namespaced.to_string(),
                r.kind.clone(),
            ];
            if wide {
                cells.push(r.verbs.join(","));
                cells.push(r.categories.join(","));
            }
            out.push_str(&cells.join("\t"));
            out.push('\n');
        }
        Ok(crate::printers::table::tab_format(&out))
    }
}

/// `ServerPreferredResources` (`discovery_client.go`): the core group's
/// resources and each group's preferred version, subresources skipped.
async fn server_preferred_resources(client: &ApiClient) -> Result<Vec<ApiResourceList>> {
    let mut lists = Vec::new();
    let core: ApiResourceList = client.get("/api/v1").await.map_err(|e| match e {
        kubernetes_rs_client::http::GetError::NotFound(_) => anyhow::anyhow!("Core API not found"),
        kubernetes_rs_client::http::GetError::Other(e) => e,
    })?;
    lists.push(core);
    let groups: ApiGroupList = client.get("/apis").await.map_err(|e| match e {
        kubernetes_rs_client::http::GetError::NotFound(_) => {
            anyhow::anyhow!("API groups not found")
        }
        kubernetes_rs_client::http::GetError::Other(e) => e,
    })?;
    for group in groups.groups {
        let path = format!("/apis/{}", group.preferred_version.group_version);
        if let Ok(list) = client.get::<ApiResourceList>(&path).await {
            lists.push(list);
        }
    }
    for list in &mut lists {
        // ignore subresources
        list.resources.retain(|r| !r.name.contains('/'));
    }
    Ok(lists)
}

/// Run `kubectl api-resources` with full options.
pub async fn execute_with_options(client: &ApiClient, opts: &ApiResourceOptions) -> Result<()> {
    opts.validate()?;
    opts.validate_output()?;
    let lists = server_preferred_resources(client).await?;
    print!("{}", opts.run_api_resources(&lists)?);
    Ok(())
}

/// Display available API resources (the flags `main.rs` wires today; the
/// remaining flags are `execute_with_options`).
pub async fn execute(
    client: &ApiClient,
    namespaced: Option<bool>,
    api_group: Option<&str>,
    no_headers: bool,
    output: Option<&str>,
) -> Result<()> {
    let opts = ApiResourceOptions {
        namespaced: namespaced.unwrap_or(true),
        ns_changed: namespaced.is_some(),
        api_group: api_group.unwrap_or_default().to_string(),
        group_changed: api_group.is_some(),
        no_headers,
        output: output.unwrap_or_default().to_string(),
        ..Default::default()
    };
    execute_with_options(client, &opts).await
}
