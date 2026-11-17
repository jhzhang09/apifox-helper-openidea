//! OpenIDEA 宿主适配。解析只读项目，写文件、上传和调试均由显式动作触发。
use crate::{
    apifox::{self, Config},
    openapi::{self, Export, Scope},
    source,
};
use openidea_plugin::views::{Body, View, ViewEvent, Viewport, Views};
use openidea_plugin::{
    Error, HttpRequest, Invocation, NotificationLevel, PickItem, Plugin, host, register_plugin,
};
use serde_json::{Value, json};
const VIEW: &str = "apifox";
const FILE_LIMIT: u32 = 10_000;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
#[derive(Default)]
pub struct ApifoxHelper;
fn project(ctx: &Invocation) -> Result<host::Project, Error> {
    ctx.project().ok_or_else(|| "请先打开项目".into())
}
fn key(ctx: &Invocation) -> Result<String, Error> {
    Ok(format!("config:{}", project(ctx)?.root()?))
}
pub fn load_config(ctx: &Invocation) -> Result<Config, Error> {
    match host::storage::get(&key(ctx)?)? {
        Some(s) => {
            serde_json::from_str(&s).map_err(|e| Error::Invalid(format!("配置解析失败：{e}")))
        }
        None => Ok(Config::default()),
    }
}
fn save_config(ctx: &Invocation, v: &Value) -> Result<(), Error> {
    let config: Config =
        serde_json::from_value(v.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
    config.validate()?;
    host::storage::set(
        &key(ctx)?,
        &serde_json::to_string(&config).map_err(|e| Error::Invalid(e.to_string()))?,
    )
}
fn config_state(ctx: &Invocation) -> Result<Value, Error> {
    Ok(
        json!({"op":"config","config":load_config(ctx)?,"tokenSet":host::secret_is_set("apifox_token"),"projectRoot":project(ctx)?.root()?}),
    )
}
fn relative(root: &str, path: &str) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .trim_start_matches('/')
        .replace('\\', "/")
}
pub fn scan(ctx: &Invocation, scope_name: &str) -> Result<Export, Error> {
    let project = project(ctx)?;
    let root = project.root()?;
    let config = load_config(ctx)?;
    let mut paths = project.find_files("**/*.java", FILE_LIMIT)?;
    let kt = project.find_files("**/*.kt", FILE_LIMIT)?;
    if paths.len() >= FILE_LIMIT as usize || kt.len() >= FILE_LIMIT as usize {
        return Err("项目源码超过扫描上限，请拆分项目后导出，避免产生不完整文档".into());
    }
    paths.extend(kt);
    paths.sort();
    paths.dedup();
    let editors = host::open_editors()?;
    let mut classes = Vec::new();
    let mut settings = crate::project::ProjectSettings::default();
    let mut bytes = 0;
    let task = host::begin_task("解析 API 和数据模型", true);
    for (i, path) in paths
        .iter()
        .filter(|p| source::production_source(&relative(&root, p)))
        .enumerate()
    {
        if task.is_cancelled() {
            return Err(Error::Cancelled);
        }
        // document() 优先使用未保存的编辑缓冲区，DTO 与 Controller 使用同一快照来源。
        let text = if editors
            .iter()
            .any(|e| e.path == *path || relative(&root, &e.path) == relative(&root, path))
        {
            project.document(path)?.text
        } else {
            project.read_file(path)?
        };
        bytes += text.len();
        if bytes > MAX_SOURCE_BYTES {
            return Err("源码大小超过 64 MiB，已停止导出".into());
        }
        let issues = source::issues(&text);
        if !issues.is_empty() {
            return Err(
                format!("{}：{}，请完成源码编辑后重新扫描", path, issues.join("；")).into(),
            );
        }
        classes.extend(source::parse(&relative(&root, path), &text));
        settings.java(&text);
        task.report(i as u32, Some(paths.len() as u32), Some(path));
    }
    let mut scope = Scope::default();
    match scope_name {
        "project" => (),
        "method" => {
            let editor = ctx
                .editor()
                .ok_or("请先打开 Controller 并将光标放在接口方法内")?;
            let selections = editor.selections()?;
            let line = selections.first().map(|s| s.start.line).unwrap_or(0);
            scope.method_line = Some((relative(&root, &editor.path()), line));
        }
        "selection" => {
            scope.paths = ctx
                .selected_paths()
                .iter()
                .map(|p| relative(&root, p))
                .collect();
            if scope.paths.is_empty() {
                if let Some(e) = ctx.editor() {
                    scope.paths.push(relative(&root, &e.path()));
                } else {
                    return Err("请选中 Controller 文件或目录".into());
                }
            }
        }
        _ => return Err("无效的扫描范围".into()),
    }
    let mut warnings = Vec::new();
    if project.exists(".apifox-helper.properties")? {
        warnings = crate::rules::apply(
            &mut classes,
            &project.read_file(".apifox-helper.properties")?,
        );
    }
    let mut config_notices = Vec::new();
    for glob in [
        "**/application.yml",
        "**/application.yaml",
        "**/application.properties",
    ] {
        let files = project.find_files(glob, 1000)?;
        if files.len() >= 1000 {
            return Err("基础配置文件超过扫描上限".into());
        }
        for path in files
            .into_iter()
            .filter(|p| !source::ignored_path(&relative(&root, p)))
        {
            let text = project.read_file(&path)?;
            if let Err(error) = settings.config(&path, &text) {
                config_notices.push(error);
            }
        }
    }
    let mut export =
        openapi::generate_with_project(&classes, &scope, &config.title, &settings.finish());
    export.notices.extend(config_notices);
    let mut modules = std::collections::BTreeMap::new();
    for endpoint in &mut export.endpoints {
        if !modules.contains_key(&endpoint.module_path) {
            let pom = if endpoint.module_path.is_empty() {
                "pom.xml".to_string()
            } else {
                format!("{}/pom.xml", endpoint.module_path)
            };
            let name = if project.exists(&pom)? {
                crate::navigation::pom_artifact_id(&project.read_file(&pom)?)
            } else {
                None
            };
            modules.insert(
                endpoint.module_path.clone(),
                name.unwrap_or_else(|| endpoint.module.clone()),
            );
        }
        if let Some(name) = modules.get(&endpoint.module_path) {
            endpoint.module = name.clone();
        }
    }
    export.warnings.extend(warnings);
    if export.endpoints.is_empty() {
        return Err(format!(
            "当前范围未识别到接口，请检查 Spring/JAX-RS 注解和光标位置。{}",
            export.warnings.first().map(String::as_str).unwrap_or("")
        )
        .into());
    }
    Ok(export)
}
fn write_export(
    ctx: &Invocation,
    export: &Export,
    format: crate::export::Format,
) -> Result<String, Error> {
    let p = project(ctx)?;
    let path = format.path();
    if p.exists(path)?
        && !host::confirm(
            "覆盖 OpenAPI 文件",
            &format!("将覆盖项目中的 {path}，是否继续？"),
        )?
    {
        return Err(Error::Cancelled);
    }
    p.write_file(path, &format.render(&export.document)?)?;
    Ok(format!(
        "已导出 {} 个接口到 {path}；{} 条错误，{} 条结构提示",
        export.endpoints.len(),
        export.warnings.len(),
        export.notices.len()
    ))
}
fn confirm_upload(ctx: &Invocation, export: &Export) -> Result<Option<String>, Error> {
    if !export.warnings.is_empty() {
        return Err(format!("存在 {} 条未解决的解析提示，已阻止上传。请在侧栏预览并修正，或导出 OpenAPI 手动检查。首条：{}",export.warnings.len(),export.warnings[0]).into());
    }
    let config = load_config(ctx)?;
    config.validate()?;
    if !host::confirm(
        "同步到 Apifox",
        &format!(
            "将 {} 个接口及其数据模型同步到项目 {}，目标接口目录 {}，覆盖策略 {}。有 {} 条结构提示，请核对预览；源码未确定的动态字段或泛型保持开放结构。是否继续？",
            export.endpoints.len(),
            config.project_id,
            config
                .endpoint_folder
                .map(|n| n.to_string())
                .unwrap_or_else(|| "根目录".into()),
            config.overwrite,
            export.notices.len()
        ),
    )? {
        return Ok(None);
    }
    Ok(Some(apifox::upload(&config, &export.document)?))
}
fn configure(ctx: &Invocation) -> Result<Option<String>, Error> {
    let mut c = load_config(ctx)?;
    let Some(id) = host::input("Apifox 项目 ID（项目设置 → 基本设置）", Some(&c.project_id))?
    else {
        return Ok(None);
    };
    c.project_id = id.trim().into();
    let Some(folder) = host::input(
        "接口目录 ID（留空为根目录）",
        c.endpoint_folder.map(|v| v.to_string()).as_deref(),
    )?
    else {
        return Ok(None);
    };
    c.endpoint_folder = if folder.trim().is_empty() {
        None
    } else {
        Some(
            folder
                .trim()
                .parse()
                .map_err(|_| Error::Invalid("目录 ID 必须是数字".into()))?,
        )
    };
    c.validate()?;
    host::storage::set(
        &key(ctx)?,
        &serde_json::to_string(&c).map_err(|e| Error::Invalid(e.to_string()))?,
    )?;
    if !host::secret_is_set("apifox_token") {
        host::notify(
            NotificationLevel::Info,
            "Apifox Helper",
            "项目配置已保存。请在 Settings → Plugins → Apifox Helper → Secrets 中设置 Apifox Access Token。",
        );
    }
    Ok(Some("Apifox 项目配置已保存".into()))
}
pub fn debug_request(v: &Value) -> Result<HttpRequest, Error> {
    let url = v["url"].as_str().ok_or("缺少请求地址")?;
    let parsed = url::Url::parse(url).map_err(|_| Error::Invalid("无效的请求地址".into()))?;
    if !["http", "https"].contains(&parsed.scheme())
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("调试仅支持不含内嵌凭据的 HTTP(S) URL".into());
    }
    let method = v["method"].as_str().unwrap_or("GET").to_uppercase();
    if ![
        "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
    ]
    .contains(&method.as_str())
    {
        return Err("不支持的 HTTP 方法".into());
    }
    let headers: Vec<(String, String)> =
        serde_json::from_value(v.get("headers").cloned().unwrap_or(json!([])))
            .map_err(|_| Error::Invalid("请求头必须是名称和值的数组".into()))?;
    if headers.iter().any(|(k, v)| {
        k.contains(['\r', '\n'])
            || v.contains(['\r', '\n'])
            || v.contains("{{secret:apifox_token}}")
    }) {
        return Err("请求头无效，或试图将 Apifox 访问令牌用于接口调试".into());
    }
    let body = v["body"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.as_bytes().to_vec());
    if body.as_ref().is_some_and(|b| b.len() > 2 * 1024 * 1024) {
        return Err("请求体超过 2 MiB".into());
    }
    Ok(HttpRequest {
        method,
        url: parsed.to_string(),
        headers,
        body,
        timeout_ms: Some(30_000),
    })
}
fn send_debug(v: &Value) -> Result<Value, Error> {
    let req = debug_request(v)?;
    if !matches!(req.method.as_str(), "GET" | "HEAD" | "OPTIONS")
        && !host::confirm(
            "发送调试请求",
            &format!(
                "{} {} 可能修改目标服务数据。是否发送？",
                req.method, req.url
            ),
        )?
    {
        return Err(Error::Cancelled);
    }
    let response = host::fetch(&req)?;
    Ok(json!({"status":response.status,"headers":response.headers,"body":response.text()}))
}
fn post(value: Value) -> Result<(), Error> {
    host::post_view_message(VIEW, &value.to_string())
}
impl Plugin for ApifoxHelper {
    fn run_command(
        &mut self,
        id: &str,
        ctx: &Invocation,
        _arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        match id {
            "configure" => configure(ctx),
            "open-panel" => {
                host::reveal_view(VIEW)?;
                Ok(None)
            }
            "export-openapi" | "export-project" | "upload" | "upload-method" | "export-method" => {
                let scope = match id {
                    "export-project" => "project",
                    "upload-method" | "export-method" => "method",
                    _ => "selection",
                };
                let export = scan(ctx, scope)?;
                if id.starts_with("upload") {
                    confirm_upload(ctx, &export)
                } else {
                    Ok(Some(write_export(
                        ctx,
                        &export,
                        crate::export::Format::Json,
                    )?))
                }
            }
            "debug" => {
                let export = scan(ctx, "selection")?;
                let items: Vec<_> = export
                    .endpoints
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        PickItem::new(&i.to_string(), &format!("{} {}", e.method, e.path))
                            .description(&e.handler)
                    })
                    .collect();
                let Some(index) = host::pick("选择调试接口", &items)? else {
                    return Ok(None);
                };
                let i = index
                    .parse::<usize>()
                    .map_err(|_| Error::Invalid("无效的接口选择".into()))?;
                let ep = export.endpoints.get(i).ok_or("无效的接口选择")?;
                let config = load_config(ctx)?;
                let Some(url) = host::input(
                    "请求 URL（请替换路径参数）",
                    Some(&format!(
                        "{}{}",
                        config.base_url.trim_end_matches('/'),
                        ep.path
                    )),
                )?
                else {
                    return Ok(None);
                };
                let body = if ep.operation.get("requestBody").is_some() {
                    host::input_multiline("请求体 JSON", Some("{}"))?
                } else {
                    None
                };
                let response = send_debug(
                    &json!({"url":url,"method":ep.method,"headers":[["Content-Type","application/json"]],"body":body}),
                )?;
                Ok(Some(
                    serde_json::to_string_pretty(&response)
                        .map_err(|e| Error::Invalid(e.to_string()))?,
                ))
            }
            _ => Err(Error::NotFound(id.into())),
        }
    }
}
impl Views for ApifoxHelper {
    fn render(
        &mut self,
        view: &str,
        _viewport: Option<Viewport>,
        ctx: &Invocation,
    ) -> Result<View, Error> {
        if view != VIEW {
            return Err(Error::NotFound(view.into()));
        }
        // 配置随页面渲染同步传入，不依赖 iframe 初始化消息的回复时机。
        let initial = if ctx.project().is_some() {
            config_state(ctx)?
        } else {
            Value::Null
        };
        let initial = initial
            .to_string()
            .replace('<', "\\u003c")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let page = include_str!("panel.html").replacen("\"__APIFOX_CONFIG__\"", &initial, 1);
        Ok(View::new(Body::Html(page)))
    }
    fn handle(&mut self, view: &str, event: ViewEvent, ctx: &Invocation) -> Result<(), Error> {
        if view != VIEW {
            return Err(Error::NotFound(view.into()));
        }
        if let ViewEvent::Message(message) = event {
            let result = (|| -> Result<Value, Error> {
                let v: Value =
                    serde_json::from_str(&message).map_err(|e| Error::Invalid(e.to_string()))?;
                match v["op"].as_str().unwrap_or("") {
                    "init" => config_state(ctx),
                    "list-projects" => {
                        let list = apifox::list_projects()?;
                        Ok(json!({"op":"projects","projects":list.projects,"notices":list.notices}))
                    }
                    "save-config" => {
                        save_config(ctx, &v["config"])?;
                        let mut state = config_state(ctx)?;
                        state["message"] = json!("配置已保存");
                        state["saved"] = json!(true);
                        Ok(state)
                    }
                    "scan" => {
                        let scope = v["scope"].as_str().unwrap_or("project");
                        let export = scan(ctx, scope)?;
                        Ok(json!({"op":"scan","export":export}))
                    }
                    "export" | "upload" => {
                        let export = scan(ctx, v["scope"].as_str().unwrap_or("project"))?;
                        let selected = v["selected"].as_array().ok_or("请先选择接口")?;
                        let ids: Vec<String> = selected
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect();
                        let export = filter_export(export, &ids)?;
                        let result = if v["op"] == "upload" {
                            confirm_upload(ctx, &export)?.unwrap_or_else(|| "已取消同步".into())
                        } else {
                            write_export(
                                ctx,
                                &export,
                                crate::export::Format::parse(
                                    v["format"].as_str().unwrap_or("json"),
                                )?,
                            )?
                        };
                        Ok(json!({"op":"notice","message":result}))
                    }
                    "debug" => Ok(json!({"op":"response","response":send_debug(&v)?})),
                    "navigate" => {
                        let export = scan(ctx, "project")?;
                        // 通过稳定标识重新定位当前声明，避免扫描后编辑源码导致旧行号失效。
                        let id = v["operationId"].as_str().ok_or("缺少接口标识")?;
                        let endpoint = export
                            .endpoints
                            .iter()
                            .find(|e| e.operation["operationId"] == id)
                            .ok_or("接口已变化，请重新扫描后跳转")?;
                        host::open_location(
                            &format!("{}/{}", project(ctx)?.root()?, endpoint.file),
                            endpoint.line,
                            0,
                        )?;
                        Ok(json!({"op":"notice","message":"已定位源码"}))
                    }
                    _ => Err("未知的侧栏动作".into()),
                }
            })();
            match result {
                Ok(value) => post(value)?,
                Err(e) => post(json!({"op":"error","message":e.to_string()}))?,
            }
        }
        Ok(())
    }
}
pub fn filter_export(mut export: Export, ids: &[String]) -> Result<Export, Error> {
    let endpoints: Vec<_> = export
        .endpoints
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.operation["operationId"]
                .as_str()
                .is_some_and(|id| ids.iter().any(|s| s == id))
        })
        .map(|(_, e)| e.clone())
        .collect();
    if endpoints.len() != ids.len() {
        return Err("接口范围已变更，请重新扫描后选择接口".into());
    }
    if endpoints.is_empty() {
        return Err("请至少选择一个接口".into());
    }
    let mut paths = json!({});
    for ep in &endpoints {
        if paths[&ep.path].is_null() {
            paths[&ep.path] = json!({});
        }
        paths[&ep.path][ep.method.to_lowercase()] = ep.operation.clone();
    }
    // 只同步所选接口可达的模型，避免改变其他接口使用的模型。
    let mut needed = std::collections::BTreeSet::new();
    let mut pending = vec![paths.clone()];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(map) => {
                if let Some(name) = map
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|r| r.strip_prefix("#/components/schemas/"))
                    && needed.insert(name.to_string())
                    && let Some(schema) = export.document["components"]["schemas"].get(name)
                {
                    pending.push(schema.clone());
                }
                pending.extend(map.into_values());
            }
            Value::Array(items) => pending.extend(items),
            _ => (),
        }
    }
    if let Some(schemas) = export.document["components"]["schemas"].as_object_mut() {
        schemas.retain(|name, _| needed.contains(name));
    }
    export.document["paths"] = paths;
    export.endpoints = endpoints;
    Ok(export)
}
register_plugin!(ApifoxHelper, views);
