//! OpenIDEA 宿主适配。解析只读项目，写文件、上传和调试均由显式动作触发。
use crate::{
    apifox::{self, Config},
    openapi::{self, Export, Scope},
    source,
};
use openidea_plugin::endpoints::{Endpoint as HostEndpoint, EndpointsProvider};
use openidea_plugin::providers::{Document as ProviderDocument, LineMarker, Providers};
use openidea_plugin::views::{Body, View, ViewEvent, Viewport, Views};
use openidea_plugin::{
    Error, HttpRequest, Invocation, Location, NotificationLevel, PickItem, Plugin, Position, Range,
    host, register_plugin,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeSet;
const VIEW: &str = "apifox";
const FILE_LIMIT: u32 = 10_000;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;

thread_local! {
    static PENDING_FOCUS: RefCell<Option<(String, String, Option<String>)>> = const { RefCell::new(None) };
}

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
    let mut config: Config =
        serde_json::from_value(v.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
    let stored = load_config(ctx)?;
    config.module_urls = stored.module_urls;
    config.global_headers = stored.global_headers;
    config.base_url = stored.base_url;
    config.validate()?;
    host::storage::set(
        &key(ctx)?,
        &serde_json::to_string(&config).map_err(|e| Error::Invalid(e.to_string()))?,
    )
}
fn config_state(ctx: &Invocation) -> Result<Value, Error> {
    Ok(
        json!({"op":"config","config":load_config(ctx)?,"tokenSet":host::secret_is_set("apifox_token"),"privateTokenSet":host::secret_is_set("apifox_private_token"),"projectRoot":project(ctx)?.root()?}),
    )
}
fn take_pending_focus(root: &str) -> Option<(String, Option<String>)> {
    PENDING_FOCUS.with(|slot| {
        let mut guard = slot.borrow_mut();
        if guard.as_ref().is_some_and(|(r, _, _)| r == root) {
            let (_, op_id, tab) = guard.take().unwrap();
            Some((op_id, tab))
        } else {
            None
        }
    })
}

fn panel_context(ctx: &Invocation) -> Result<Value, Error> {
    if ctx.project().is_some() {
        let mut state = config_state(ctx)?;
        let root = project(ctx)?.root()?;
        match crate::snapshot::load(&root) {
            Ok(snapshot) => state["snapshot"] = json!(snapshot),
            Err(error) => {
                state["snapshot"] = Value::Null;
                state["snapshotError"] = json!(error.to_string());
            }
        }
        if let Some((op_id, tab)) = take_pending_focus(&root) {
            state["focusOperationId"] = json!(op_id);
            if let Some(tab) = tab {
                state["focusTab"] = json!(tab);
            }
        }
        Ok(state)
    } else {
        Ok(json!({"op":"config","config":Config::default(),"tokenSet":false,"projectRoot":null}))
    }
}
fn relative(root: &str, path: &str) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .trim_start_matches('/')
        .replace('\\', "/")
}
struct ProjectSources {
    classes: Vec<source::Class>,
    settings: crate::project::ProjectSettings,
    warnings: Vec<String>,
    config_notices: Vec<String>,
}

fn load_project_sources(
    project: &host::Project,
    root: &str,
    strict_syntax: bool,
    report_progress: bool,
) -> Result<ProjectSources, Error> {
    let mut paths = project.find_files("**/*.java", FILE_LIMIT)?;
    let kt = project.find_files("**/*.kt", FILE_LIMIT)?;
    if paths.len() >= FILE_LIMIT as usize || kt.len() >= FILE_LIMIT as usize {
        return Err("项目源码超过扫描上限，请拆分项目后导出，避免产生不完整文档".into());
    }
    paths.extend(kt);
    paths.sort();
    paths.dedup();
    let production_paths: Vec<String> = paths
        .into_iter()
        .filter(|p| source::production_source(&relative(root, p)))
        .collect();
    let total = production_paths.len();
    let mut classes = Vec::new();
    let mut settings = crate::project::ProjectSettings::default();
    let mut bytes = 0;
    let task = report_progress.then(|| host::begin_task("解析 API 和数据模型", true));
    for (i, path) in production_paths.iter().enumerate() {
        if task.as_ref().is_some_and(|t| t.is_cancelled()) {
            return Err(Error::Cancelled);
        }
        // read_file 已优先读取未保存缓冲区；恢复的标签页不一定有版本化内存文档。
        let text = project.read_file(path)?;
        bytes += text.len();
        if bytes > MAX_SOURCE_BYTES {
            return Err("源码大小超过 64 MiB，已停止导出".into());
        }
        let rel = relative(root, path);
        match source::parse_checked(&rel, &text) {
            Ok(parsed) => classes.extend(parsed),
            Err(issues) => {
                if strict_syntax {
                    return Err(format!(
                        "{}：{}，请完成源码编辑后重新扫描",
                        path,
                        issues.join("；")
                    )
                    .into());
                }
            }
        }
        settings.java(&text);
        if let Some(task) = &task
            && (i % 32 == 0 || i + 1 == total)
        {
            task.report(i as u32, Some(total as u32), Some(path));
        }
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
            .filter(|p| !source::ignored_path(&relative(root, p)))
        {
            let text = project.read_file(&path)?;
            if let Err(error) = settings.config(&path, &text) {
                config_notices.push(error);
            }
        }
    }
    Ok(ProjectSources {
        classes,
        settings,
        warnings,
        config_notices,
    })
}

pub fn scan(ctx: &Invocation, scope_name: &str) -> Result<Export, Error> {
    let project = project(ctx)?;
    let root = project.root()?;
    let config = load_config(ctx)?;
    let ProjectSources {
        classes,
        settings,
        warnings,
        config_notices,
    } = load_project_sources(&project, &root, true, true)?;
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
                let gradle_files: &[&str] = if endpoint.module_path.is_empty() {
                    &[
                        "settings.gradle.kts",
                        "settings.gradle",
                        "build.gradle.kts",
                        "build.gradle",
                    ]
                } else {
                    &["build.gradle.kts", "build.gradle"]
                };
                let mut found = None;
                for gf in gradle_files {
                    let candidate = if endpoint.module_path.is_empty() {
                        (*gf).to_string()
                    } else {
                        format!("{}/{gf}", endpoint.module_path)
                    };
                    if project.exists(&candidate)? {
                        found =
                            crate::navigation::gradle_project_name(&project.read_file(&candidate)?);
                        if found.is_some() {
                            break;
                        }
                    }
                }
                found
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
fn is_unresolved_type_warning(msg: &str) -> bool {
    msg.starts_with("无法解析类型 ") || msg.starts_with("无法解析父类型 ")
}

fn confirm_upload(ctx: &Invocation, export: &Export) -> Result<Option<String>, Error> {
    let (unresolved, blocking): (Vec<&String>, Vec<&String>) = export
        .warnings
        .iter()
        .partition(|w| is_unresolved_type_warning(w));
    if !blocking.is_empty() {
        return Err(format!(
            "存在 {} 条未解决的解析提示，已阻止上传。请在侧栏预览并修正，或导出 OpenAPI 手动检查。首条：{}",
            blocking.len(),
            blocking[0]
        )
        .into());
    }
    let config = load_config(ctx)?;
    config.validate()?;
    let unresolved_note = if unresolved.is_empty() {
        String::new()
    } else {
        format!(
            " 注意：包含 {} 条未解析的外部/依赖类型（无本地源码，将按开放对象结构上传，首条：{}），若 Apifox 服务端已有手工字段且开启覆盖策略可能被覆盖。",
            unresolved.len(),
            unresolved[0]
        )
    };
    if !host::confirm(
        "同步到 Apifox",
        &format!(
            "目标服务器：{}。将 {} 个接口及其数据模型同步到项目 {}，目标接口目录 {}，覆盖策略 {}。有 {} 条结构提示，请核对预览；源码未确定的动态字段或泛型保持开放结构。{}是否继续？",
            config.server()?,
            export.endpoints.len(),
            config.project_id,
            config
                .endpoint_folder
                .map(|n| n.to_string())
                .unwrap_or_else(|| "根目录".into()),
            config.overwrite,
            export.notices.len(),
            unresolved_note
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
    c.set_project_id(&id);
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
    if !host::secret_is_set(c.token_name()) {
        host::notify(
            NotificationLevel::Info,
            "Apifox Helper",
            &format!(
                "项目配置已保存。请在插件 Secrets 中设置 {}。",
                c.token_name()
            ),
        );
    }
    Ok(Some("Apifox 项目配置已保存".into()))
}
pub fn schema_example(doc: &Value, schema: &Value) -> Value {
    fn inner(doc: &Value, schema: &Value, seen: &mut BTreeSet<String>, depth: usize) -> Value {
        if depth > 12 {
            return Value::Null;
        }
        if let Some(ex) = schema.get("example") {
            return ex.clone();
        }
        if let Some(def) = schema.get("default") {
            return def.clone();
        }
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            if !seen.insert(r.to_string()) {
                return Value::Null;
            }
            let out = r
                .strip_prefix('#')
                .and_then(|ptr| doc.pointer(ptr))
                .map(|target| inner(doc, target, seen, depth + 1))
                .unwrap_or(Value::Null);
            seen.remove(r);
            return out;
        }
        if let Some(items) = schema.get("allOf").and_then(Value::as_array) {
            let mut merged = serde_json::Map::new();
            for item in items {
                if let Value::Object(map) = inner(doc, item, seen, depth + 1) {
                    merged.extend(map);
                }
            }
            return Value::Object(merged);
        }
        if let Some(vals) = schema.get("enum").and_then(Value::as_array)
            && let Some(first) = vals.first()
        {
            return first.clone();
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            let mut out = serde_json::Map::new();
            for (k, v) in props {
                if v.get("readOnly") == Some(&Value::Bool(true)) {
                    continue;
                }
                out.insert(k.clone(), inner(doc, v, seen, depth + 1));
            }
            return Value::Object(out);
        }
        if let Some(item_schema) = schema.get("items") {
            return json!([inner(doc, item_schema, seen, depth + 1)]);
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("string") => json!(""),
            Some("integer") | Some("number") => json!(0),
            Some("boolean") => json!(false),
            Some("object") => json!({}),
            _ => Value::Null,
        }
    }
    inner(doc, schema, &mut BTreeSet::new(), 0)
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
        k.contains(['\r', '\n']) || v.contains(['\r', '\n']) || v.contains("{{secret:")
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
    let started = std::time::SystemTime::now();
    let response = host::fetch(&req)?;
    let duration_ms = started.elapsed().map(|d| d.as_millis() as u64).ok();
    Ok(json!({
        "status": response.status,
        "headers": response.headers,
        "body": response.text(),
        "durationMs": duration_ms
    }))
}
fn post(value: Value) -> Result<(), Error> {
    host::post_view_message(VIEW, &value.to_string())
}
fn cursor_endpoint_id(ctx: &Invocation) -> Option<(String, String)> {
    let proj = ctx.project()?;
    let root = proj.root().ok()?;
    let editor = ctx.editor()?;
    let rel = relative(&root, &editor.path());
    if !source::production_source(&rel) {
        return None;
    }
    let line = editor
        .selections()
        .ok()?
        .first()
        .map(|s| s.start.line)
        .unwrap_or(0);
    let text = editor
        .document()
        .map(|d| d.text)
        .or_else(|_| proj.read_file(&rel))
        .ok()?;
    let classes = source::parse(&rel, &text);
    if classes.is_empty() {
        return None;
    }
    let single = openapi::generate(
        &classes,
        &Scope {
            method_line: Some((rel.clone(), line)),
            ..Default::default()
        },
        "",
    );
    if let Some(ep) = single.endpoints.first() {
        return ep.operation["operationId"]
            .as_str()
            .map(|id| (root, id.to_string()));
    }
    if let Ok(Some(snap)) = crate::snapshot::load(&root) {
        for c in &classes {
            for m in &c.methods {
                if m.line <= line && m.end_line >= line {
                    let handler = format!("{}.{}", c.name, m.name);
                    if let Some(ep) = snap
                        .export
                        .endpoints
                        .iter()
                        .find(|e| e.file == rel && e.handler == handler)
                    {
                        return ep.operation["operationId"]
                            .as_str()
                            .map(|id| (root, id.to_string()));
                    }
                }
            }
        }
    }
    None
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
                if let Some((root, op_id)) = cursor_endpoint_id(ctx) {
                    PENDING_FOCUS.with(|slot| {
                        *slot.borrow_mut() = Some((root.clone(), op_id.clone(), None));
                    });
                    host::reveal_view(VIEW)?;
                    let _ =
                        post(json!({"op":"focus-endpoint","operationId":op_id,"projectRoot":root}));
                } else {
                    host::reveal_view(VIEW)?;
                }
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
                let (export, ep_index) = if ctx.editor().is_some()
                    && let Ok(method_export) = scan(ctx, "method")
                    && method_export.endpoints.len() == 1
                {
                    (method_export, 0)
                } else {
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
                    if i >= export.endpoints.len() {
                        return Err("无效的接口选择".into());
                    }
                    (export, i)
                };
                let ep = &export.endpoints[ep_index];
                if let Ok(root) = project(ctx).and_then(|p| p.root())
                    && let Some(op_id) = ep.operation["operationId"].as_str()
                {
                    PENDING_FOCUS.with(|slot| {
                        *slot.borrow_mut() =
                            Some((root.clone(), op_id.to_string(), Some("debug".into())));
                    });
                    let _ = host::reveal_view(VIEW);
                    let _ = post(
                        json!({"op":"focus-endpoint","operationId":op_id,"tab":"debug","projectRoot":root}),
                    );
                }
                let config = load_config(ctx)?;
                let manual = config
                    .module_urls
                    .get(&ep.module_path)
                    .map(String::as_str)
                    .unwrap_or_else(|| {
                        if config.base_url == "http://localhost:8080" {
                            ""
                        } else {
                            &config.base_url
                        }
                    });
                let base = if manual.is_empty() {
                    &export.detected_url
                } else {
                    manual
                };
                let Some(url) = host::input(
                    "请求 URL（请替换路径参数）",
                    Some(&format!("{}{}", base.trim_end_matches('/'), ep.path)),
                )?
                else {
                    return Ok(None);
                };
                let body = if let Some(content) = ep.operation.pointer("/requestBody/content")
                    && let Some((_, media_obj)) = content.as_object().and_then(|m| m.iter().next())
                {
                    let default_body = media_obj
                        .get("schema")
                        .map(|s| schema_example(&export.document, s))
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                        .unwrap_or_else(|| "{}".into());
                    host::input_multiline("请求体 JSON", Some(&default_body))?
                } else {
                    None
                };
                let mut headers = Vec::new();
                if !config.global_headers.trim().is_empty()
                    && let Ok(Value::Object(map)) =
                        serde_json::from_str::<Value>(config.global_headers.trim())
                {
                    for (k, v) in map {
                        let s = v
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string());
                        headers.push([k, s]);
                    }
                }
                headers.push(["Content-Type".into(), "application/json".into()]);
                let response = send_debug(
                    &json!({"url":url,"method":ep.method,"headers":headers,"body":body}),
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
        let mut initial = panel_context(ctx)?;
        // 宿主会截断超过 4 MiB 的 HTML；快照通过独立消息恢复，不能嵌入页面脚本。
        initial["snapshotAvailable"] = json!(initial["snapshot"].is_object());
        initial.as_object_mut().unwrap().remove("snapshot");
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
            let request: Value =
                serde_json::from_str(&message).map_err(|e| Error::Invalid(e.to_string()))?;
            let root = ctx.project().map(|p| p.root()).transpose()?;
            let result = (|| -> Result<Value, Error> {
                let v = &request;
                let action = v["op"].as_str().unwrap_or("");
                // 面板可能由宿主复用；读取、写入和网络动作都必须属于当前本地项目。
                if !matches!(action, "init" | "context")
                    && (root.is_none() || v["projectRoot"].as_str() != root.as_deref())
                {
                    let mut state = panel_context(ctx)?;
                    state["contextChanged"] = json!(true);
                    state["message"] =
                        json!("本地项目已切换，已阻止旧面板动作，请在当前项目重新操作");
                    return Ok(state);
                }
                match v["op"].as_str().unwrap_or("") {
                    "init" => panel_context(ctx),
                    "context" => {
                        if v["projectRoot"].as_str() == root.as_deref() {
                            Ok(json!({"op":"context"}))
                        } else {
                            let mut state = panel_context(ctx)?;
                            state["contextChanged"] = json!(true);
                            Ok(state)
                        }
                    }
                    "list-projects" => {
                        let mut config = load_config(ctx)?;
                        if let Some(address) = v["private_api_url"].as_str() {
                            config.private_api_url = address.into();
                        }
                        let list = apifox::list_projects(&config)?;
                        Ok(json!({"op":"projects","projects":list.projects,"notices":list.notices}))
                    }
                    "save-config" => {
                        save_config(ctx, &v["config"])?;
                        let mut state = config_state(ctx)?;
                        state["message"] = json!("配置已保存");
                        state["saved"] = json!(true);
                        Ok(state)
                    }
                    "save-debug" => {
                        let mut config = load_config(ctx)?;
                        let module = v["modulePath"].as_str().ok_or("缺少模块路径")?;
                        let base = v["baseUrl"].as_str().ok_or("缺少前置 URL")?.trim();
                        if !base.is_empty() {
                            let parsed = url::Url::parse(base)
                                .map_err(|_| Error::Invalid("无效的前置 URL".into()))?;
                            if !matches!(parsed.scheme(), "http" | "https")
                                || parsed.host().is_none()
                                || !parsed.username().is_empty()
                                || parsed.password().is_some()
                                || parsed.query().is_some()
                                || parsed.fragment().is_some()
                            {
                                return Err(
                                    "前置 URL 仅支持无凭据、查询参数和片段的 HTTP(S) 地址".into()
                                );
                            }
                        }
                        config.module_urls.insert(module.into(), base.into());
                        config.global_headers =
                            v["headers"].as_str().ok_or("缺少默认请求头")?.to_owned();
                        config.validate_headers()?;
                        host::storage::set(
                            &key(ctx)?,
                            &serde_json::to_string(&config)
                                .map_err(|e| Error::Invalid(e.to_string()))?,
                        )?;
                        Ok(
                            json!({"op":"debug-saved","module_urls":config.module_urls,"global_headers":config.global_headers}),
                        )
                    }
                    "scan" => {
                        // 侧栏扫描固定整个项目，局部导出命令不会覆盖完整快照。
                        let export = scan(ctx, "project")?;
                        let snapshot =
                            crate::snapshot::Snapshot::new(project(ctx)?.root()?, export)?;
                        let error = snapshot.save().err().map(|e| e.to_string());
                        Ok(
                            json!({"op":"scan","export":snapshot.export,"scannedAtMs":snapshot.scanned_at_ms,"snapshotSaved":error.is_none(),"snapshotError":error}),
                        )
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
                    "debug" => Ok(json!({"op":"response","response":send_debug(v)?})),
                    "copy" => {
                        let text = v["text"].as_str().ok_or("缺少复制内容")?;
                        host::copy_to_clipboard(text).map_err(|e| match e {
                            Error::Denied(_) => Error::Failed(
                                "请在插件权限中允许 clipboard 权限，或直接从预览框复制".into(),
                            ),
                            other => other,
                        })?;
                        let label = v["label"].as_str().unwrap_or("内容");
                        Ok(json!({"op":"notice","message":format!("已复制{label}到剪贴板")}))
                    }
                    "navigate" => {
                        let proj = project(ctx)?;
                        let root_dir = proj.root()?;
                        if let Some(file) = v["file"].as_str() {
                            let rel = file.trim_start_matches('/');
                            if rel.is_empty() || rel.contains("..") {
                                return Err("无效的文件路径".into());
                            }
                            if !proj.exists(rel)? {
                                return Err(format!("文件不存在：{rel}").into());
                            }
                            let line = v["line"].as_u64().unwrap_or(0) as u32;
                            host::open_location(&format!("{root_dir}/{rel}"), line, 0)?;
                            return Ok(json!({"op":"notice","message":"已定位源码"}));
                        }
                        let id = v["operationId"].as_str().ok_or("缺少接口标识")?;
                        // 优先基于快照定位单文件，避免无关文件语法未闭合或大项目全量重扫阻断跳转。
                        if let Ok(Some(snap)) = crate::snapshot::load(&root_dir)
                            && let Some(snap_ep) = snap
                                .export
                                .endpoints
                                .iter()
                                .find(|e| e.operation["operationId"] == id)
                            && snap_ep.declaration_file.is_none()
                            && proj.exists(&snap_ep.file)?
                        {
                            let text = proj.read_file(&snap_ep.file)?;
                            if let Ok(classes) = source::parse_checked(&snap_ep.file, &text) {
                                let title = load_config(ctx).map(|c| c.title).unwrap_or_default();
                                let single = openapi::generate(&classes, &Scope::default(), &title);
                                if let Some(ep) = single
                                    .endpoints
                                    .iter()
                                    .find(|e| e.operation["operationId"] == id)
                                {
                                    host::open_location(
                                        &format!(
                                            "{root_dir}/{}",
                                            ep.declaration_file.as_ref().unwrap_or(&ep.file)
                                        ),
                                        ep.line,
                                        0,
                                    )?;
                                    return Ok(json!({"op":"notice","message":"已定位源码"}));
                                }
                                let matched: Vec<u32> = classes
                                    .iter()
                                    .flat_map(|c| {
                                        c.methods
                                            .iter()
                                            .filter(|m| {
                                                format!("{}.{}", c.name, m.name) == snap_ep.handler
                                            })
                                            .map(|m| m.line)
                                    })
                                    .collect();
                                if matched.len() == 1 {
                                    host::open_location(
                                        &format!("{root_dir}/{}", snap_ep.file),
                                        matched[0],
                                        0,
                                    )?;
                                    return Ok(json!({"op":"notice","message":"已定位源码"}));
                                }
                            }
                        }
                        let export = scan(ctx, "project")?;
                        // 通过稳定标识重新定位当前声明，避免扫描后编辑源码导致旧行号失效。
                        let endpoint = export
                            .endpoints
                            .iter()
                            .find(|e| e.operation["operationId"] == id)
                            .ok_or("接口已变化，请重新扫描后跳转")?;
                        host::open_location(
                            &format!(
                                "{root_dir}/{}",
                                endpoint.declaration_file.as_ref().unwrap_or(&endpoint.file)
                            ),
                            endpoint.line,
                            0,
                        )?;
                        Ok(json!({"op":"notice","message":"已定位源码"}))
                    }
                    _ => Err("未知的侧栏动作".into()),
                }
            })();
            let mut response = match result {
                Ok(value) => value,
                Err(e) => json!({"op":"error","message":e.to_string()}),
            };
            response["projectRoot"] = json!(root);
            response["requestId"] = request["requestId"].clone();
            response["clientId"] = request["clientId"].clone();
            post(response)?;
        }
        Ok(())
    }
}
impl EndpointsProvider for ApifoxHelper {
    fn endpoints(&mut self, ctx: &Invocation) -> Result<Vec<HostEndpoint>, Error> {
        let Some(proj) = ctx.project() else {
            return Ok(Vec::new());
        };
        let root = proj.root()?;
        let sources = load_project_sources(&proj, &root, false, false)?;
        let endpoints = openapi::list_endpoints(&sources.classes, &sources.settings.finish());
        Ok(endpoints
            .into_iter()
            .map(|ep| HostEndpoint {
                method: Some(ep.method),
                path: ep.path,
                framework: "Spring / JAX-RS".into(),
                handler: Some(ep.handler),
                location: Location {
                    path: format!("{root}/{}", ep.declaration_file.unwrap_or(ep.file)),
                    range: Range::new(Position::new(ep.line, 0), Position::new(ep.line, 0)),
                },
            })
            .collect())
    }
}
impl Providers for ApifoxHelper {
    fn line_markers(&mut self, doc: &ProviderDocument) -> Vec<LineMarker> {
        if !source::production_source(&doc.path) {
            return Vec::new();
        }
        let classes = source::parse(&doc.path, &doc.text);
        if classes.is_empty() {
            return Vec::new();
        }
        let export = openapi::generate(&classes, &Scope::default(), "API");
        let mut markers = Vec::new();
        let mut marked_lines = BTreeSet::new();
        for ep in export.endpoints {
            if marked_lines.insert(ep.line) {
                markers.push(LineMarker {
                    line: ep.line,
                    icon: Some("run".into()),
                    tooltip: format!(
                        "Apifox: {} {} ({})",
                        ep.method,
                        ep.path,
                        ep.operation["summary"].as_str().unwrap_or(&ep.handler)
                    ),
                    target: None,
                });
            }
        }
        for c in &classes {
            if !["RestController", "Controller", "FeignClient", "Path"]
                .iter()
                .any(|a| source::has(&c.annotations, a))
            {
                continue;
            }
            for m in &c.methods {
                if openapi::is_endpoint_method(m) && marked_lines.insert(m.line) {
                    markers.push(LineMarker {
                        line: m.line,
                        icon: Some("run".into()),
                        tooltip: format!("Apifox 接口方法：{}.{}", c.name, m.name),
                        target: None,
                    });
                }
            }
        }
        markers
    }
}
pub fn filter_export(mut export: Export, ids: &[String]) -> Result<Export, Error> {
    let total = export.endpoints.len();
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
    let mut needed = BTreeSet::new();
    let mut pending: Vec<&Value> = vec![&paths];
    let schemas_map = export.document["components"]["schemas"].as_object();
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(map) => {
                if let Some(name) = map
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|r| r.strip_prefix("#/components/schemas/"))
                    && needed.insert(name.to_string())
                    && let Some(schema) = schemas_map.and_then(|m| m.get(name))
                {
                    pending.push(schema);
                }
                pending.extend(map.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => (),
        }
    }
    if let Some(schemas) = export.document["components"]["schemas"].as_object_mut() {
        schemas.retain(|name, _| needed.contains(name));
    }
    if endpoints.len() < total {
        let scoped_warnings: BTreeSet<_> = export
            .endpoints
            .iter()
            .flat_map(|ep| &ep.warnings)
            .collect();
        let scoped_notices: BTreeSet<_> =
            export.endpoints.iter().flat_map(|ep| &ep.notices).collect();
        let mut filtered_warnings: BTreeSet<_> = export
            .warnings
            .into_iter()
            .filter(|msg| !scoped_warnings.contains(msg))
            .collect();
        let mut filtered_notices: BTreeSet<_> = export
            .notices
            .into_iter()
            .filter(|msg| !scoped_notices.contains(msg))
            .collect();
        for ep in &endpoints {
            filtered_warnings.extend(ep.warnings.iter().cloned());
            filtered_notices.extend(ep.notices.iter().cloned());
        }
        export.warnings = filtered_warnings.into_iter().collect();
        export.notices = filtered_notices.into_iter().collect();
    }
    export.document["paths"] = paths;
    export.endpoints = endpoints;
    Ok(export)
}
register_plugin!(ApifoxHelper, views, endpoints, providers);
