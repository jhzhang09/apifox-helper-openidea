//! 将源码契约转换为 OpenAPI，上传前保留解析告警，避免静默覆盖不完整模型。
use crate::{
    schema::{Schemas, Type, decorate, typed_value},
    source::{self, Class, Method, annotation, has},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub file: String,
    pub line: u32,
    pub end_line: u32,
    pub operation: Value,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub module_path: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub controller: String,
    #[serde(default)]
    pub controller_name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    pub document: Value,
    pub endpoints: Vec<Endpoint>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub notices: Vec<String>,
}
#[derive(Default)]
pub struct Scope {
    pub paths: Vec<String>,
    pub method_line: Option<(String, u32)>,
}
impl Scope {
    fn accepts(&self, c: &Class, m: &Method) -> bool {
        if let Some((path, line)) = &self.method_line {
            return &c.path == path && m.line <= *line && m.end_line >= *line;
        }
        self.paths.is_empty()
            || self.paths.iter().any(|p| {
                c.path == *p || c.path.starts_with(&format!("{}/", p.trim_end_matches('/')))
            })
    }
}

fn mappings(anns: &[source::Annotation]) -> Option<(Vec<String>, Vec<String>)> {
    for a in anns {
        let methods = match a.name.as_str() {
            "GetMapping" | "GET" => vec!["GET".into()],
            "PostMapping" | "POST" => vec!["POST".into()],
            "PutMapping" | "PUT" => vec!["PUT".into()],
            "DeleteMapping" | "DELETE" => vec!["DELETE".into()],
            "PatchMapping" | "PATCH" => vec!["PATCH".into()],
            "HEAD" => vec!["HEAD".into()],
            "OPTIONS" => vec!["OPTIONS".into()],
            "RequestMapping" => {
                let m = a.values("method");
                if m.is_empty() {
                    vec!["GET".into()]
                } else {
                    m.iter()
                        .map(|s| s.rsplit('.').next().unwrap_or(s).to_string())
                        .collect()
                }
            }
            _ => continue,
        };
        let paths = route_values(a);
        return Some((methods, paths));
    }
    None
}
fn route_values(a: &source::Annotation) -> Vec<String> {
    let key = if a.values("path").is_empty() {
        "value"
    } else {
        "path"
    };
    if a.expressions.contains(key) {
        return vec![format!("!constant:{}", a.values(key).join(""))];
    }
    let values = a.values(key);
    if values.is_empty() {
        vec![String::new()]
    } else {
        values
    }
}
fn path_of(anns: &[source::Annotation]) -> Vec<String> {
    mappings(anns)
        .map(|(_, p)| p)
        .or_else(|| annotation(anns, "Path").map(route_values))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![String::new()])
}
fn media(anns: &[source::Annotation], class: &Class, key: &str, default: &str) -> Vec<String> {
    let vals = anns
        .iter()
        .chain(class.annotations.iter())
        .find_map(|a| {
            let v = a.values(key);
            if v.is_empty() { None } else { Some(v) }
        })
        .or_else(|| {
            annotation(
                anns,
                if key == "produces" {
                    "Produces"
                } else {
                    "Consumes"
                },
            )
            .or_else(|| {
                annotation(
                    &class.annotations,
                    if key == "produces" {
                        "Produces"
                    } else {
                        "Consumes"
                    },
                )
            })
            .map(|a| a.values("value"))
        })
        .unwrap_or_else(|| vec![default.into()]);
    vals.into_iter()
        .map(|v| match v.as_str() {
            "MediaType.APPLICATION_JSON_VALUE" | "MediaType.APPLICATION_JSON" => {
                "application/json".into()
            }
            "MediaType.TEXT_PLAIN_VALUE" | "MediaType.TEXT_PLAIN" => "text/plain".into(),
            "MediaType.MULTIPART_FORM_DATA_VALUE" | "MediaType.MULTIPART_FORM_DATA" => {
                "multipart/form-data".into()
            }
            "MediaType.TEXT_EVENT_STREAM_VALUE" | "MediaType.TEXT_EVENT_STREAM" => {
                "text/event-stream".into()
            }
            _ => v.split(';').next().unwrap_or(&v).trim().to_string(),
        })
        .collect()
}
fn join_path(a: &str, b: &str) -> String {
    let p = format!("{}/{}", a.trim_matches('/'), b.trim_matches('/'));
    format!("/{}", p.trim_matches('/'))
}
fn operation(class: &Class, m: &Method, schemas: &mut Schemas, path: &str) -> Value {
    let title = crate::documentation::operation_title(m);
    let folder = crate::documentation::controller_title(class);
    let description = crate::documentation::operation_description(m);
    let mut op = json!({"summary":title,"description":description,"tags":[folder],"x-apifox-folder":folder,"parameters":[],"responses":{}});
    if has(&m.annotations, "Deprecated") || source::tag(&m.doc, "deprecated").is_some() {
        op["deprecated"] = json!(true);
    }
    let mut params = Vec::new();
    let mut form = serde_json::Map::new();
    let mut form_required = Vec::new();
    let mut multipart = false;
    for p in &m.params {
        if let Some(a) = annotation(&p.annotations, "PathVariable")
            && a.value("required").as_deref() == Some("false")
        {
            let name = a
                .value("name")
                .or_else(|| a.value("value"))
                .unwrap_or_else(|| p.name.clone());
            if !path.contains(&format!("{{{name}}}")) && !path.contains(&format!("{{{name}:")) {
                continue;
            }
        }
        let ty = Type::parse(&p.ty);
        if has(&p.annotations, "RequestAttribute")
            || has(&p.annotations, "Context")
            || annotation(&p.annotations, "Parameter")
                .and_then(|a| a.value("hidden"))
                .as_deref()
                == Some("true")
            || matches!(
                ty.simple(),
                "HttpServletRequest"
                    | "HttpServletResponse"
                    | "ServletRequest"
                    | "ServletResponse"
                    | "Principal"
                    | "Authentication"
                    | "BindingResult"
                    | "Errors"
                    | "Model"
                    | "ModelMap"
                    | "WebRequest"
                    | "Continuation"
            )
        {
            continue;
        }
        let base = schemas.schema(&ty, class);
        let mut schema = if base.get("$ref").is_some() {
            json!({"allOf":[base]})
        } else {
            base
        };
        decorate(&mut schema, p);
        let desc = crate::documentation::parameter_description(p, m);
        if crate::documentation::spring_request_body(p).is_some()
            || has(&p.annotations, "Body")
            || (has(&class.annotations, "Path")
                && ![
                    "QueryParam",
                    "PathParam",
                    "HeaderParam",
                    "CookieParam",
                    "FormParam",
                    "BeanParam",
                ]
                .iter()
                .any(|a| has(&p.annotations, a)))
        {
            let required = crate::documentation::spring_request_body(p)
                .and_then(|a| a.value("required"))
                .as_deref()
                != Some("false");
            let mut content = serde_json::Map::new();
            for media in media(&m.annotations, class, "consumes", "application/json") {
                content.insert(media, json!({"schema":schema}));
            }
            let body_description = crate::documentation::request_body_description(p, m);
            op["requestBody"] =
                json!({"required":required,"description":body_description,"content":content});
            continue;
        }
        let ann = [
            ("PathVariable", "path"),
            ("PathParam", "path"),
            ("RequestHeader", "header"),
            ("HeaderParam", "header"),
            ("CookieValue", "cookie"),
            ("CookieParam", "cookie"),
            ("RequestParam", "query"),
            ("QueryParam", "query"),
            ("RequestPart", "form"),
            ("FormParam", "form"),
        ]
        .into_iter()
        .find_map(|(n, loc)| annotation(&p.annotations, n).map(|a| (a, loc)));
        let (name, loc, required, default) = if let Some((a, loc)) = ann {
            let default = a.value("defaultValue");
            (
                a.value("name")
                    .or_else(|| a.value("value"))
                    .unwrap_or_else(|| p.name.clone()),
                loc,
                loc == "path"
                    || (a.value("required").as_deref() != Some("false")
                        && default.is_none()
                        && !ty.nullable),
                default,
            )
        } else {
            (
                p.name.clone(),
                "query",
                has(&p.annotations, "NotNull"),
                None,
            )
        };
        if let Some(default) = default {
            schema["default"] = typed_value(&default, &schema);
        }
        if loc == "form"
            || ty.simple() == "MultipartFile"
            || ty.args.iter().any(|t| t.simple() == "MultipartFile")
        {
            multipart |= ty.simple() == "MultipartFile"
                || ty.args.iter().any(|t| t.simple() == "MultipartFile")
                || has(&p.annotations, "RequestPart");
            if !desc.is_empty() {
                schema["description"] = json!(desc);
            }
            form.insert(name.clone(), schema);
            if required {
                form_required.push(name);
            }
            continue;
        }
        if ann.is_none() && schemas.resolve(&ty.name, class).is_some() {
            let reference = schema
                .get("$ref")
                .or_else(|| schema.pointer("/allOf/0/$ref"))
                .unwrap_or(&Value::Null)
                .as_str()
                .and_then(|s| s.rsplit('/').next())
                .map(str::to_owned);
            if let Some(shape) = reference.and_then(|r| schemas.components.get(&r)).cloned()
                && let Some(fields) = shape["properties"].as_object()
            {
                for (n, s) in fields {
                    params.push(json!({"name":n,"in":"query","required":shape["required"].as_array().is_some_and(|v|v.contains(&json!(n))),"schema":s}));
                }
                continue;
            }
        }
        params.push(
            json!({"name":name,"in":loc,"required":required,"description":desc,"schema":schema}),
        );
    }
    if !form.is_empty() {
        let mut schema = json!({"type":"object","properties":form});
        if !form_required.is_empty() {
            schema["required"] = json!(form_required);
        }
        let content = if multipart {
            "multipart/form-data"
        } else {
            "application/x-www-form-urlencoded"
        };
        op["requestBody"] = json!({"required":true,"content":{content:{"schema":schema}}});
    }
    op["parameters"] = json!(params);
    let mut response =
        json!({"description":source::tag(&m.doc,"return").unwrap_or_else(||"成功".into())});
    let ty = Type::parse(&m.ty);
    let is_void = matches!(ty.simple(), "void" | "Void" | "Unit")
        || (matches!(ty.simple(), "ResponseEntity" | "Mono")
            && ty
                .args
                .first()
                .is_some_and(|t| matches!(t.simple(), "Void" | "Unit")));
    if !is_void {
        let schema = schemas.schema(&ty, class);
        let mut content = serde_json::Map::new();
        for media in media(&m.annotations, class, "produces", "application/json") {
            content.insert(media, json!({"schema":schema}));
        }
        response["content"] = json!(content);
    }
    let status = annotation(&m.annotations, "ResponseStatus")
        .and_then(|a| a.value("code").or_else(|| a.value("value")))
        .map(|s| match s.rsplit('.').next().unwrap_or(&s) {
            "CREATED" => "201".into(),
            "NO_CONTENT" => "204".into(),
            "ACCEPTED" => "202".into(),
            "OK" => "200".into(),
            n => n.to_string(),
        })
        .unwrap_or_else(|| "200".into());
    let descriptions = crate::documentation::response_descriptions(class, m);
    if let Some(Some(description)) = descriptions.get(&status) {
        response["description"] = json!(description);
    }
    op["responses"][&status] = response;
    for (code, description) in descriptions {
        if code != status {
            op["responses"][&code] =
                json!({"description":description.unwrap_or_else(||"响应".into())});
        }
    }
    op
}

/// Spring 正则可以包含量词、字符类和转义括号；只有变量最外层的括号进入 OpenAPI。
fn normalize_route(path: &str) -> Result<String, String> {
    let mut cleaned = String::new();
    let (mut variable, mut regex, mut escaped) = (false, false, false);
    let (mut braces, mut classes, mut name_len) = (0usize, 0usize, 0usize);
    for ch in path.chars() {
        if regex {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '[' => classes += 1,
                ']' if classes > 0 => classes -= 1,
                _ if classes > 0 => (),
                '{' => braces += 1,
                '}' if braces > 0 => braces -= 1,
                '}' => {
                    variable = false;
                    regex = false;
                    cleaned.push('}');
                }
                _ => (),
            }
        } else if variable {
            match ch {
                ':' if name_len > 0 => regex = true,
                '}' if name_len > 0 => {
                    variable = false;
                    cleaned.push('}');
                }
                '{' | '}' | ':' => return Err("变量名为空或包含嵌套括号".into()),
                _ => {
                    name_len += 1;
                    cleaned.push(ch);
                }
            }
        } else {
            match ch {
                '{' => {
                    variable = true;
                    name_len = 0;
                    cleaned.push(ch);
                }
                '}' => return Err("存在多余的闭合括号".into()),
                _ => cleaned.push(ch),
            }
        }
    }
    if variable {
        return Err("路径变量或正则未闭合".into());
    }
    Ok(cleaned)
}

pub fn generate(classes: &[Class], scope: &Scope, title: &str) -> Export {
    generate_with_project(
        classes,
        scope,
        title,
        &crate::project::ProjectContext::default(),
    )
}
pub fn generate_with_project(
    classes: &[Class],
    scope: &Scope,
    title: &str,
    project: &crate::project::ProjectContext,
) -> Export {
    let mut schemas = Schemas::with_project(classes, project);
    let mut endpoints = Vec::new();
    let mut paths = json!({});
    let mut operation_ids = std::collections::BTreeSet::new();
    for c in classes {
        // 仅声明方法映射不会注册 Spring 控制器；注释掉的类不能制造重复路由。
        if !["RestController", "Controller", "FeignClient", "Path"]
            .iter()
            .any(|a| has(&c.annotations, a))
        {
            continue;
        }
        for m in &c.methods {
            if !scope.accepts(c, m)
                || has(&m.annotations, "Hidden")
                || annotation(&m.annotations, "Operation")
                    .and_then(|a| a.value("hidden"))
                    .as_deref()
                    == Some("true")
            {
                continue;
            }
            let Some((verbs, mut method_paths)) = mappings(&m.annotations) else {
                continue;
            };
            if let Some(p) = annotation(&m.annotations, "Path") {
                method_paths = route_values(p);
            }
            for base in path_of(&c.annotations) {
                for path in &method_paths {
                    for verb in &verbs {
                        if [&base, path].iter().any(|p| p.starts_with("!constant:")) {
                            schemas.warnings.insert(format!(
                                "路由包含未解析的常量 {base} {path}（{}）",
                                c.path
                            ));
                            continue;
                        }
                        let placeholders = base.contains("${") || path.contains("${");
                        let resolved = project
                            .route(&base)
                            .and_then(|base| project.route(path).map(|path| (base, path)));
                        let (base, route) = match resolved {
                            Ok(value) => value,
                            Err(error) => {
                                schemas.warnings.insert(format!(
                                    "未解析路由 {base}{path}（{}）：{error}",
                                    c.path
                                ));
                                continue;
                            }
                        };
                        if placeholders {
                            schemas.notices.insert(format!("路由占位符按项目基础配置解析（{}）；运行时 profile 和环境变量覆盖值需核对",c.path));
                        }
                        let path = join_path(&base, &route);
                        if ![
                            "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE",
                        ]
                        .contains(&verb.as_str())
                        {
                            schemas
                                .warnings
                                .insert(format!("未解析 HTTP 方法 {verb}（{}）", c.path));
                            continue;
                        }
                        if path.contains('$') || !path.starts_with('/') {
                            schemas
                                .warnings
                                .insert(format!("未解析路由 {path}（{}）", c.path));
                            continue;
                        }
                        let cleaned = match normalize_route(&path) {
                            Ok(path) => path,
                            Err(error) => {
                                schemas.warnings.insert(format!(
                                    "无法规范化路由 {path}（{}）：{error}",
                                    c.path
                                ));
                                continue;
                            }
                        };
                        let mut op = operation(c, m, &mut schemas, &cleaned);
                        let id = format!("{}_{}_{}_{}", c.qualified(), m.name, verb, path);
                        let id = crate::schema::component_key(&id);
                        if !operation_ids.insert(id.clone())
                            || paths[&cleaned][verb.to_lowercase()].is_object()
                        {
                            schemas.warnings.insert(format!(
                                "重复路由 {verb} {cleaned}（原始路由 {path}），保留首个声明"
                            ));
                            continue;
                        }
                        op["operationId"] = json!(id);
                        if paths[&cleaned].is_null() {
                            paths[&cleaned] = json!({});
                        }
                        paths[&cleaned][verb.to_lowercase()] = op.clone();
                        endpoints.push(Endpoint {
                            method: verb.clone(),
                            path: cleaned,
                            handler: format!("{}.{}", c.name, m.name),
                            file: c.path.clone(),
                            line: m.line,
                            end_line: m.end_line,
                            module: c
                                .path
                                .rsplit_once("/src/")
                                .map(|(prefix, _)| prefix.rsplit('/').next().unwrap_or(prefix))
                                .unwrap_or("项目")
                                .to_string(),
                            module_path: c
                                .path
                                .rsplit_once("/src/")
                                .map(|(prefix, _)| prefix)
                                .unwrap_or("")
                                .to_string(),
                            package: c.package.clone(),
                            controller: c.name.clone(),
                            controller_name: op["tags"][0].as_str().unwrap_or(&c.name).to_string(),
                            operation: op,
                        });
                    }
                }
            }
        }
    }
    let names = schemas.export_names();
    crate::schema::rename_refs(&mut paths, &names);
    for endpoint in &mut endpoints {
        crate::schema::rename_refs(&mut endpoint.operation, &names);
    }
    let components: BTreeMap<_, _> = schemas
        .components
        .into_iter()
        .map(|(key, mut value)| {
            crate::schema::rename_refs(&mut value, &names);
            let name = names[&key].clone();
            if value
                .get("title")
                .and_then(Value::as_str)
                .and_then(source::nonempty)
                .is_none()
            {
                value["title"] = json!(name);
            }
            (name, value)
        })
        .collect();
    let mut warnings: Vec<_> = schemas.warnings.into_iter().collect();
    for ep in &endpoints {
        for p in ep.operation["parameters"].as_array().into_iter().flatten() {
            if p["in"] == "path"
                && !ep
                    .path
                    .contains(&format!("{{{}}}", p["name"].as_str().unwrap_or("")))
            {
                warnings.push(format!("路径参数 {} 与路由 {} 不匹配", p["name"], ep.path));
            }
        }
    }
    Export {
        document: json!({"openapi":"3.0.3","info":{"title":title,"version":"1.0.0"},"paths":paths,"components":{"schemas":components}}),
        endpoints,
        warnings,
        notices: schemas.notices.into_iter().collect(),
    }
}
