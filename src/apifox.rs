//! Apifox 项目查询与 OpenAPI 同步；令牌由宿主密钥库替换，不进入插件存储。
use openidea_plugin::{Error, HttpRequest, host};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub project_id: String,
    pub project_name: String,
    pub team_name: String,
    pub endpoint_folder: Option<u64>,
    pub schema_folder: Option<u64>,
    pub branch: Option<u64>,
    pub module: Option<u64>,
    pub overwrite: String,
    pub title: String,
    pub base_url: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            project_name: String::new(),
            team_name: String::new(),
            endpoint_folder: None,
            schema_folder: None,
            branch: None,
            module: None,
            overwrite: "AUTO_MERGE".into(),
            title: "API 文档".into(),
            base_url: "http://localhost:8080".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteProject {
    pub id: String,
    pub name: String,
    pub team_id: String,
    pub team_name: String,
}
#[derive(Debug, Serialize)]
pub struct ProjectList {
    pub projects: Vec<RemoteProject>,
    pub notices: Vec<String>,
}

/// 协议来源：本地官方 ApifoxHelper 2.1.2-262 的资源配置和字节码，不是公开 API 文档。
pub fn project_list_request(teams: bool) -> HttpRequest {
    HttpRequest {
        timeout_ms: Some(30_000),
        ..HttpRequest::get(if teams {
            "https://api.apifox.com/api/v1/user-teams?locale=zh-CN"
        } else {
            "https://api.apifox.com/api/v1/user-projects"
        })
        .header("X-Apifox-Version", "2024-03-28")
        .header("Authorization", "Bearer {{secret:apifox_token}}")
        .header("Accept", "application/json")
    }
}
fn list_data(status: u16, body: &str) -> Result<Vec<Value>, Error> {
    match status {
        200..=299 => (),
        401 => return Err("Apifox 令牌无效或已过期，请更新插件密钥后重试".into()),
        403 => return Err("Apifox 拒绝访问项目列表，请检查账号和令牌权限".into()),
        429 => return Err("Apifox 项目查询过于频繁，请稍后重试".into()),
        _ => {
            return Err(
                format!("Apifox 项目查询返回 HTTP {status}，请稍后重试或手动填写项目 ID").into(),
            );
        }
    }
    let v: Value = serde_json::from_str(body)
        .map_err(|_| Error::Invalid("Apifox 项目列表返回非 JSON 数据".into()))?;
    if v.get("success") == Some(&Value::Bool(false))
        || v.get("errorCode")
            .is_some_and(|code| !code.is_null() && code != &json!(0) && code != &json!("0"))
    {
        return Err("Apifox 未能返回可用项目列表，请检查令牌权限或稍后重试".into());
    }
    let items = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Invalid("Apifox 项目列表结构已变化，暂时请手动填写项目 ID".into()))?;
    if items.len() > 20_000 {
        return Err("Apifox 项目列表超过展示上限".into());
    }
    Ok(items.clone())
}
fn remote_id(value: &Value) -> Option<String> {
    value.as_u64().map(|n| n.to_string()).or_else(|| {
        value
            .as_str()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_owned)
    })
}
fn required_name(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
pub fn parse_projects(status: u16, body: &str) -> Result<Vec<RemoteProject>, Error> {
    let mut projects = std::collections::BTreeMap::new();
    for item in list_data(status, body)? {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .ok_or("Apifox 项目缺少类型，无法判断是否支持 OpenAPI 同步")?;
        if kind != "HTTP" {
            continue;
        }
        let id = remote_id(&item["id"]).ok_or("Apifox 项目 ID 格式无效")?;
        let name = required_name(&item["name"]).ok_or("Apifox 项目名称格式无效")?;
        let team_id = remote_id(&item["teamId"]).ok_or("Apifox 项目团队 ID 格式无效")?;
        let project = RemoteProject {
            id: id.clone(),
            name,
            team_id,
            team_name: String::new(),
        };
        if projects.get(&id).is_some_and(|old| old != &project) {
            return Err("Apifox 返回了冲突的项目数据，请重试".into());
        }
        projects.insert(id, project);
    }
    Ok(projects.into_values().collect())
}
pub fn parse_teams(
    status: u16,
    body: &str,
) -> Result<std::collections::BTreeMap<String, String>, Error> {
    let mut teams = std::collections::BTreeMap::new();
    for item in list_data(status, body)? {
        let id = remote_id(&item["id"]).ok_or("Apifox 团队 ID 格式无效")?;
        let name = required_name(&item["name"]).ok_or("Apifox 团队名称格式无效")?;
        teams.insert(id, name);
    }
    Ok(teams)
}
pub fn list_projects() -> Result<ProjectList, Error> {
    if !host::secret_is_set("apifox_token") {
        return Err("请先在插件 Secrets 中配置 Apifox Access Token，并允许 secrets 与 net:api.apifox.com 权限".into());
    }
    let response = host::fetch(&project_list_request(false))?;
    let mut projects = parse_projects(response.status, &response.text())?;
    let mut notices = Vec::new();
    if !projects.is_empty() {
        let teams =
            host::fetch(&project_list_request(true)).and_then(|r| parse_teams(r.status, &r.text()));
        match teams {
            Ok(teams) => {
                for project in &mut projects {
                    project.team_name = teams
                        .get(&project.team_id)
                        .cloned()
                        .unwrap_or_else(|| format!("团队 {}", project.team_id));
                }
            }
            Err(_) => {
                notices.push("项目已加载，团队名称暂时无法获取，按团队 ID 显示".into());
                for project in &mut projects {
                    project.team_name = format!("团队 {}", project.team_id);
                }
            }
        }
        projects
            .sort_by(|a, b| (&a.team_name, &a.name, &a.id).cmp(&(&b.team_name, &b.name, &b.id)));
    }
    Ok(ProjectList { projects, notices })
}
impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        if self.project_id.is_empty() || !self.project_id.bytes().all(|c| c.is_ascii_digit()) {
            return Err("Apifox 项目 ID 必须是数字".into());
        }
        if ![
            "AUTO_MERGE",
            "OVERWRITE_EXISTING",
            "KEEP_EXISTING",
            "CREATE_NEW",
        ]
        .contains(&self.overwrite.as_str())
        {
            return Err("无效的覆盖策略".into());
        }
        Ok(())
    }
}
pub fn import_request(config: &Config, document: &Value) -> Result<HttpRequest, Error> {
    config.validate()?;
    let mut options = json!({"endpointOverwriteBehavior":config.overwrite,"schemaOverwriteBehavior":config.overwrite,"updateFolderOfChangedEndpoint":false,"prependBasePath":false,"deleteUnmatchedResources":false});
    for (key, value) in [
        ("targetEndpointFolderId", config.endpoint_folder),
        ("targetSchemaFolderId", config.schema_folder),
        ("targetBranchId", config.branch),
        ("moduleId", config.module),
    ] {
        if let Some(v) = value {
            options[key] = json!(v);
        }
    }
    let body=json!({"input":serde_json::to_string(document).map_err(|e|Error::Invalid(e.to_string()))?,"options":options}).to_string();
    Ok(HttpRequest {
        timeout_ms: Some(60_000),
        ..HttpRequest::post(
            &format!(
                "https://api.apifox.com/v1/projects/{}/import-openapi?locale=zh-CN",
                config.project_id
            ),
            body,
        )
        .header("X-Apifox-Api-Version", "2024-03-28")
        .header("Authorization", "Bearer {{secret:apifox_token}}")
        .header("Content-Type", "application/json")
    })
}
pub fn outcome(status: u16, body: &str) -> Result<String, Error> {
    if !(200..300).contains(&status) {
        return Err(format!("Apifox 返回 HTTP {status}，请检查令牌权限、项目 ID 和网络").into());
    }
    let v: Value = serde_json::from_str(body)
        .map_err(|_| Error::Invalid("Apifox 返回了非 JSON 数据".into()))?;
    let c = v
        .pointer("/data/counters")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Invalid("Apifox 未返回导入统计，不能确认同步完成".into()))?;
    let count = |name: &str| c.get(name).and_then(Value::as_u64).unwrap_or(0);
    let failures: [&str; 4] = [
        "endpointFailed",
        "schemaFailed",
        "endpointFolderFailed",
        "schemaFolderFailed",
    ];
    let errors = v
        .pointer("/data/errors")
        .filter(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()));
    let summary = format!(
        "接口新增 {}，更新 {}，忽略 {}；模型新增 {}，更新 {}",
        count("endpointCreated"),
        count("endpointUpdated"),
        count("endpointIgnored"),
        count("schemaCreated"),
        count("schemaUpdated")
    );
    if failures.iter().any(|k| count(k) > 0) || errors.is_some() {
        return Err(format!("Apifox 部分导入失败：{summary}；请到 Apifox 检查导入结果").into());
    }
    if !c.contains_key("endpointCreated") || !c.contains_key("endpointUpdated") {
        return Err("Apifox 返回的统计不完整，不能确认同步完成".into());
    }
    Ok(summary)
}
pub fn upload(config: &Config, doc: &Value) -> Result<String, Error> {
    if !host::secret_is_set("apifox_token") {
        return Err("请在插件详情的密钥配置中设置 Apifox Access Token".into());
    }
    let resp = host::fetch(&import_request(config, doc)?)?;
    outcome(resp.status, &resp.text())
}
