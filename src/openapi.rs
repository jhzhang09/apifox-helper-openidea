//! 将源码契约转换为 OpenAPI，上传前保留解析告警，避免静默覆盖不完整模型。
use crate::{
    schema::{ClassIndex, Schemas, Type, decorate, typed_value},
    source::{self, Class, Method, annotation, has},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_file: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    #[serde(default)]
    pub detected_url: String,
    pub document: Value,
    pub endpoints: Vec<Endpoint>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub notices: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointLocation {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub file: String,
    pub declaration_file: Option<String>,
    pub line: u32,
}
#[derive(Default)]
pub struct Scope {
    pub paths: Vec<String>,
    pub method_line: Option<(String, u32)>,
}
fn path_matches(target: &str, scope_path: &str) -> bool {
    let prefix = scope_path.trim_end_matches('/');
    target == scope_path
        || (target.starts_with(prefix) && target.as_bytes().get(prefix.len()) == Some(&b'/'))
}
impl Scope {
    fn accepts_class(&self, c: &Class) -> bool {
        if let Some((path, _)) = &self.method_line {
            return &c.path == path;
        }
        self.paths.is_empty() || self.paths.iter().any(|p| path_matches(&c.path, p))
    }
    fn accepts(&self, c: &Class, m: &Method) -> bool {
        if let Some((path, line)) = &self.method_line {
            return &c.path == path && m.line <= *line && m.end_line >= *line;
        }
        self.paths.is_empty() || self.paths.iter().any(|p| path_matches(&c.path, p))
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
pub fn is_endpoint_method(m: &Method) -> bool {
    mappings(&m.annotations).is_some()
}
fn route_values(a: &source::Annotation) -> Vec<String> {
    let key = if a.values("path").is_empty() {
        "value"
    } else {
        "path"
    };
    let values = a.values(key);
    if values.is_empty() {
        return if a.expressions.contains(key) {
            vec!["!constant:".into()]
        } else {
            vec![String::new()]
        };
    }
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            if a.is_expr_item(key, i) {
                format!("!constant:{v}")
            } else {
                v
            }
        })
        .collect()
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
fn operation(
    class: &Class,
    declaration: &Class,
    m: &Method,
    schemas: &mut Schemas,
    path: &str,
) -> Value {
    schemas.begin_operation();
    let title = crate::documentation::operation_title(m);
    let folder = crate::documentation::controller_title(class);
    let tags = crate::documentation::operation_tags(class, m);
    let description = crate::documentation::operation_description(m);
    let mut op = json!({"summary":title,"description":description,"tags":tags,"x-apifox-folder":folder,"parameters":[],"responses":{}});
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
                    | "HttpSession"
                    | "ServletRequest"
                    | "ServletResponse"
                    | "NativeWebRequest"
                    | "ServerWebExchange"
                    | "ServerHttpRequest"
                    | "ServerHttpResponse"
                    | "RedirectAttributes"
                    | "SessionStatus"
                    | "UriComponentsBuilder"
                    | "ServletUriComponentsBuilder"
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
        let base = schemas.schema(&ty, declaration);
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
            let swagger_param = crate::documentation::parameter_annotation(p, m);
            let param_in = swagger_param
                .and_then(|a| a.value("in"))
                .and_then(|v| match v.rsplit('.').next().unwrap_or(&v) {
                    "HEADER" | "header" => Some("header"),
                    "PATH" | "path" => Some("path"),
                    "COOKIE" | "cookie" => Some("cookie"),
                    "QUERY" | "query" => Some("query"),
                    _ => None,
                })
                .unwrap_or("query");
            let req = param_in == "path"
                || has(&p.annotations, "NotNull")
                || swagger_param.and_then(|a| a.value("required")).as_deref() == Some("true");
            (
                swagger_param
                    .and_then(|a| a.text("name"))
                    .unwrap_or_else(|| p.name.clone()),
                param_in,
                req,
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
        if ann.is_none() && loc == "query" && schemas.resolve(&ty.name, declaration).is_some() {
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
                    let mut param = json!({
                        "name": n,
                        "in": "query",
                        "required": shape["required"].as_array().is_some_and(|v| v.contains(&json!(n))),
                        "schema": s
                    });
                    if let Some(d) = s
                        .get("description")
                        .and_then(Value::as_str)
                        .and_then(source::nonempty)
                    {
                        param["description"] = json!(d);
                    }
                    params.push(param);
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
        let schema = schemas.schema(&ty, declaration);
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
    let (mut variable, mut regex, mut escaped, mut catch_all) = (false, false, false, false);
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
                '*' if name_len == 0 && !catch_all => catch_all = true,
                '*' if name_len == 0 => return Err("变量名为空或包含嵌套括号".into()),
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
                    catch_all = false;
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

fn find_constant_in_hierarchy<'a>(
    name: &str,
    ctx: &'a Class,
    index: &ClassIndex<'a>,
    seen: &mut BTreeSet<String>,
) -> Option<(&'a Class, &'a str)> {
    if !seen.insert(ctx.qualified()) {
        return None;
    }
    if let Some(val) = ctx.constants.get(name) {
        return Some((ctx, val.as_str()));
    }
    let mut enclosing = ctx.name.as_str();
    while let Some((parent_name, _)) = enclosing.rsplit_once('.') {
        let candidate = if ctx.package.is_empty() {
            parent_name.to_string()
        } else {
            format!("{}.{parent_name}", ctx.package)
        };
        if let Some(parent) = index.get_qualified(&candidate)
            && let Some(found) = find_constant_in_hierarchy(name, parent, index, seen)
        {
            return Some(found);
        }
        enclosing = parent_name;
    }
    for base in ctx.bases.iter().chain(ctx.interfaces.iter()) {
        let ty = Type::parse(base);
        if let Some(parent) = index.resolve_quiet(&ty.name, ctx)
            && let Some(found) = find_constant_in_hierarchy(name, parent, index, seen)
        {
            return Some(found);
        }
    }
    None
}

fn find_constant<'a>(
    ident: &str,
    ctx: &'a Class,
    index: &ClassIndex<'a>,
    ambiguity: &mut Option<String>,
) -> Option<(&'a Class, String, &'a str)> {
    if let Some((holder, const_name)) = ident.rsplit_once('.') {
        let holder_class = index.resolve_quiet(holder, ctx)?;
        let (owner, val) =
            find_constant_in_hierarchy(const_name, holder_class, index, &mut BTreeSet::new())?;
        return Some((owner, const_name.to_string(), val));
    }
    if let Some((owner, val)) = find_constant_in_hierarchy(ident, ctx, index, &mut BTreeSet::new())
    {
        return Some((owner, ident.to_string(), val));
    }
    for suffix in [format!(".{ident}"), ".*".to_owned()] {
        let mut candidates = BTreeMap::new();
        let mut explicit = false;
        for holder_name in ctx
            .static_imports
            .iter()
            .chain(ctx.imports.iter())
            .filter_map(|imp| imp.strip_suffix(&suffix))
        {
            explicit |= suffix != ".*";
            if let Some(holder) = index.get_qualified(holder_name)
                && let Some((owner, value)) =
                    find_constant_in_hierarchy(ident, holder, index, &mut BTreeSet::new())
            {
                candidates.insert(owner.qualified(), (owner, ident.to_string(), value));
            }
        }
        if candidates.len() > 1 {
            *ambiguity = Some(format!(
                "路由常量 {ident} 导入歧义：{}",
                candidates.keys().cloned().collect::<Vec<_>>().join("、")
            ));
            return None;
        }
        if explicit || !candidates.is_empty() {
            return if candidates.len() == 1 {
                candidates.into_values().next()
            } else {
                None
            };
        }
    }
    None
}

fn split_plus_outside_quotes(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '+' if !in_quotes => {
                parts.push(s[start..idx].trim());
                start = idx + 1;
            }
            _ => (),
        }
    }
    parts.push(s[start..].trim());
    parts
}

fn resolve_route_expr(
    expr: &str,
    ctx: &Class,
    index: &ClassIndex,
    visited: &mut BTreeSet<(String, String)>,
    depth: usize,
    ambiguity: &mut Option<String>,
) -> Option<String> {
    if depth > 16 || expr.is_empty() {
        return None;
    }
    let mut out = String::new();
    for part in split_plus_outside_quotes(expr) {
        if part.is_empty() {
            return None;
        }
        if part.starts_with('"') && part.ends_with('"') && part.len() >= 2 {
            out.push_str(&serde_json::from_str::<String>(part).ok()?);
        } else if part
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
            && part
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$'))
            && !part.starts_with('.')
            && !part.ends_with('.')
        {
            let (owner, name, raw_val) = find_constant(part, ctx, index, ambiguity)?;
            let key = (owner.qualified(), name);
            if !visited.insert(key.clone()) {
                return None;
            }
            let resolved = resolve_route_expr(raw_val, owner, index, visited, depth + 1, ambiguity);
            visited.remove(&key);
            out.push_str(&resolved?);
        } else {
            return None;
        }
    }
    Some(out)
}

fn resolve_route_candidate(
    raw: &str,
    contexts: &[&Class],
    index: &ClassIndex,
    controller_path: &str,
) -> Result<String, String> {
    let Some(expr) = raw.strip_prefix("!constant:") else {
        return Ok(raw.to_string());
    };
    for ctx in contexts {
        let mut ambiguity = None;
        if let Some(resolved) =
            resolve_route_expr(expr, ctx, index, &mut BTreeSet::new(), 0, &mut ambiguity)
        {
            return Ok(resolved);
        }
        if let Some(reason) = ambiguity {
            return Err(format!("{reason}（{controller_path}）"));
        }
    }
    Err(format!("路由包含未解析的常量 {expr}（{controller_path}）"))
}

struct EffectiveMethod<'a> {
    method: Method,
    is_inherited: bool,
    owner: &'a Class,
}

fn bound_type(
    ty: &Type,
    ctx: &Class,
    index: &ClassIndex,
    bindings: &BTreeMap<String, Type>,
) -> Type {
    if bindings.contains_key(&ty.name) {
        return ty.substitute(bindings);
    }
    Type {
        name: index
            .resolve_quiet(&ty.name, ctx)
            .map(|c| c.qualified())
            .unwrap_or_else(|| Schemas::external_name(&ty.name, ctx)),
        args: ty
            .args
            .iter()
            .map(|arg| bound_type(arg, ctx, index, bindings))
            .collect(),
        ..ty.clone()
    }
}

fn method_key(
    m: &Method,
    owner: &Class,
    index: &ClassIndex,
    bindings: &BTreeMap<String, Type>,
) -> (String, Vec<String>) {
    (
        m.name.clone(),
        m.params
            .iter()
            .map(|p| bound_type(&Type::parse(&p.ty), owner, index, bindings).signature())
            .collect(),
    )
}

fn collect_ancestors<'a>(
    c: &'a Class,
    index: &ClassIndex<'a>,
) -> Vec<(&'a Class, BTreeMap<String, Type>)> {
    let mut ancestors = Vec::new();
    let mut seen = BTreeSet::new();
    seen.insert(c.qualified());
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((c, BTreeMap::new()));
    while let Some((curr, bindings)) = queue.pop_front() {
        for base in curr.bases.iter().chain(curr.interfaces.iter()) {
            let ty = bound_type(&Type::parse(base), curr, index, &bindings);
            if let Some(parent) = index.resolve_quiet(&ty.name, curr)
                && seen.insert(parent.qualified())
            {
                let next_bindings: BTreeMap<String, Type> =
                    parent.generics.iter().cloned().zip(ty.args).collect();
                ancestors.push((parent, next_bindings.clone()));
                queue.push_back((parent, next_bindings));
            }
        }
    }
    ancestors
}

fn merge_param(param: &mut source::Field, parent_param: &source::Field) {
    for pa in &parent_param.annotations {
        if let Some(existing) = param.annotations.iter_mut().find(|a| a.name == pa.name) {
            if existing.args.is_empty() && !pa.args.is_empty() {
                existing.args = pa.args.clone();
                existing.expressions = pa.expressions.clone();
                existing.item_exprs = pa.item_exprs.clone();
                existing.nested = pa.nested.clone();
            }
        } else {
            param.annotations.push(pa.clone());
        }
    }
    if source::nonempty(&param.doc).is_none() {
        param.doc = parent_param.doc.clone();
    }
}

fn effective_controller<'a>(
    c: &'a Class,
    index: &ClassIndex<'a>,
) -> (Class, Vec<String>, Vec<&'a Class>, Vec<EffectiveMethod<'a>>) {
    let ancestors = collect_ancestors(c, index);
    let mut effective_class = c.clone();
    let has_own_tag = has(&c.annotations, "Tag") || has(&c.annotations, "Api");
    for (parent, _) in &ancestors {
        for ann in &parent.annotations {
            if matches!(
                ann.name.as_str(),
                "Produces" | "Consumes" | "ApiResponses" | "ApiResponse"
            ) || (!has_own_tag && matches!(ann.name.as_str(), "Tag" | "Api"))
            {
                effective_class.annotations.push(ann.clone());
            }
        }
    }
    let (base_paths, base_contexts) =
        if mappings(&c.annotations).is_some() || annotation(&c.annotations, "Path").is_some() {
            (path_of(&c.annotations), vec![c])
        } else if let Some((parent, _)) = ancestors.iter().find(|(p, _)| {
            mappings(&p.annotations).is_some() || annotation(&p.annotations, "Path").is_some()
        }) {
            (path_of(&parent.annotations), vec![c, *parent])
        } else {
            (vec![String::new()], vec![c])
        };

    let mut methods = Vec::new();
    let mut signatures = BTreeSet::new();
    for m in &c.methods {
        if ancestors.is_empty() {
            methods.push(EffectiveMethod {
                method: m.clone(),
                is_inherited: false,
                owner: c,
            });
            continue;
        }
        let mut merged = m.clone();
        let mut owner = c;
        let key = method_key(m, c, index, &BTreeMap::new());
        if let Some((parent, pm)) = ancestors.iter().find_map(|(parent, bindings)| {
            parent
                .methods
                .iter()
                .find(|pm| pm.name == m.name && method_key(pm, parent, index, bindings) == key)
                .map(|pm| (*parent, pm))
        }) {
            owner = parent;
            merged.annotations.extend(pm.annotations.iter().cloned());
            if source::nonempty(&merged.doc).is_none() {
                merged.doc = pm.doc.clone();
            }
            for (p, pp) in merged.params.iter_mut().zip(&pm.params) {
                merge_param(p, pp);
            }
        }
        signatures.insert(key);
        methods.push(EffectiveMethod {
            method: merged,
            is_inherited: false,
            owner,
        });
    }
    for (parent, bindings) in &ancestors {
        for pm in &parent.methods {
            if (mappings(&pm.annotations).is_none()
                && annotation(&pm.annotations, "Path").is_none())
                || !signatures.insert(method_key(pm, parent, index, bindings))
            {
                continue;
            }
            let mut inherited = pm.clone();
            inherited.ty = Type::parse(&pm.ty).substitute(bindings).signature();
            for p in &mut inherited.params {
                p.ty = Type::parse(&p.ty).substitute(bindings).signature();
            }
            methods.push(EffectiveMethod {
                method: inherited,
                is_inherited: true,
                owner: parent,
            });
        }
    }
    (effective_class, base_paths, base_contexts, methods)
}

fn collect_direct_refs<'v>(root: &'v Value, out: &mut Vec<&'v str>) {
    let mut stack = vec![root];
    while let Some(v) = stack.pop() {
        match v {
            Value::Object(map) => {
                if let Some(name) = map
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|r| r.strip_prefix("#/components/schemas/"))
                {
                    out.push(name);
                }
                stack.extend(map.values());
            }
            Value::Array(items) => stack.extend(items),
            _ => (),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_endpoint_route(
    raw_base: &str,
    raw_path: &str,
    verb: &str,
    base_contexts: &[&Class],
    method_contexts: &[&Class],
    index: &ClassIndex,
    project: &crate::project::ProjectContext,
    controller_path: &str,
) -> Result<(String, String, Option<String>), String> {
    let base = resolve_route_candidate(raw_base, base_contexts, index, controller_path)?;
    let path = resolve_route_candidate(raw_path, method_contexts, index, controller_path)?;
    let placeholders = base.contains("${") || path.contains("${");
    let (base, route) = project
        .route(&base)
        .and_then(|base| project.route(&path).map(|path| (base, path)))
        .map_err(|error| format!("未解析路由 {base}{path}（{controller_path}）：{error}"))?;
    let placeholder_notice = placeholders.then(|| {
        format!("路由占位符按项目基础配置解析（{controller_path}）；运行时 profile 和环境变量覆盖值需核对")
    });
    let path = join_path(&base, &route);
    if ![
        "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE",
    ]
    .contains(&verb)
    {
        return Err(format!("未解析 HTTP 方法 {verb}（{controller_path}）"));
    }
    if path.contains('$') || !path.starts_with('/') {
        return Err(format!("未解析路由 {path}（{controller_path}）"));
    }
    let cleaned = normalize_route(&path)
        .map_err(|error| format!("无法规范化路由 {path}（{controller_path}）：{error}"))?;
    Ok((path, cleaned, placeholder_notice))
}

/// 仅提取接口路由与源码位置，供宿主 EndpointsProvider 使用，不构建 OpenAPI 与模型。
pub fn list_endpoints(
    classes: &[Class],
    project: &crate::project::ProjectContext,
) -> Vec<EndpointLocation> {
    let index = ClassIndex::new(classes);
    let mut out = Vec::new();
    let mut operation_ids = HashSet::new();
    let mut seen_routes = HashSet::new();
    for c in classes {
        if !["RestController", "Controller", "FeignClient", "Path"]
            .iter()
            .any(|a| has(&c.annotations, a))
        {
            continue;
        }
        let (_, base_paths, base_contexts, effective_methods) = effective_controller(c, &index);
        for em in &effective_methods {
            let m = &em.method;
            if has(&m.annotations, "Hidden")
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
            let method_contexts: Vec<&Class> = if std::ptr::eq(em.owner, c) {
                vec![c]
            } else {
                vec![c, em.owner]
            };
            let declaration = if em.is_inherited { em.owner } else { c };
            for raw_base in &base_paths {
                for raw_path in &method_paths {
                    for verb in &verbs {
                        let Ok((path, cleaned, _)) = resolve_endpoint_route(
                            raw_base,
                            raw_path,
                            verb,
                            &base_contexts,
                            &method_contexts,
                            &index,
                            project,
                            &c.path,
                        ) else {
                            continue;
                        };
                        let id = crate::schema::component_key(&format!(
                            "{}_{}_{}_{}",
                            c.qualified(),
                            m.name,
                            verb,
                            path
                        ));
                        if !operation_ids.insert(id)
                            || !seen_routes.insert((cleaned.clone(), verb.to_lowercase()))
                        {
                            continue;
                        }
                        out.push(EndpointLocation {
                            method: verb.clone(),
                            path: cleaned,
                            handler: format!("{}.{}", c.name, m.name),
                            file: c.path.clone(),
                            declaration_file: em.is_inherited.then(|| declaration.path.clone()),
                            line: m.line,
                        });
                    }
                }
            }
        }
    }
    out
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
    let mut endpoints: Vec<Endpoint> = Vec::new();
    let mut endpoint_meta: Vec<(BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)> = Vec::new();
    let mut paths = json!({});
    let mut operation_ids = BTreeSet::new();
    for c in classes {
        // 仅声明方法映射不会注册 Spring 控制器；注释掉的类不能制造重复路由。
        if !scope.accepts_class(c)
            || !["RestController", "Controller", "FeignClient", "Path"]
                .iter()
                .any(|a| has(&c.annotations, a))
        {
            continue;
        }
        let (effective_class, base_paths, base_contexts, effective_methods) =
            effective_controller(c, &schemas.index);
        for em in &effective_methods {
            let m = &em.method;
            if (em.is_inherited && scope.method_line.is_some())
                || !scope.accepts(c, m)
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
            let method_contexts: Vec<&Class> = if std::ptr::eq(em.owner, c) {
                vec![c]
            } else {
                vec![c, em.owner]
            };
            for raw_base in &base_paths {
                for raw_path in &method_paths {
                    for verb in &verbs {
                        let (path, cleaned, placeholder_notice) = match resolve_endpoint_route(
                            raw_base,
                            raw_path,
                            verb,
                            &base_contexts,
                            &method_contexts,
                            &schemas.index,
                            project,
                            &c.path,
                        ) {
                            Ok(v) => v,
                            Err(msg) => {
                                schemas.warnings.insert(msg);
                                continue;
                            }
                        };
                        if let Some(msg) = &placeholder_notice {
                            schemas.notices.insert(msg.clone());
                        }
                        let declaration = if em.is_inherited { em.owner } else { c };
                        let mut op =
                            operation(&effective_class, declaration, m, &mut schemas, &cleaned);
                        let id = format!("{}_{}_{}_{}", c.qualified(), m.name, verb, path);
                        let id = crate::schema::component_key(&id);
                        if !operation_ids.insert(id.clone())
                            || paths[&cleaned][verb.to_lowercase()].is_object()
                        {
                            let msg = format!(
                                "重复路由 {verb} {cleaned}（原始路由 {path}），保留首个声明"
                            );
                            schemas.warnings.insert(msg.clone());
                            if let Some((_, meta)) = endpoints
                                .iter()
                                .zip(&mut endpoint_meta)
                                .find(|(e, _)| e.path == cleaned && e.method == *verb)
                            {
                                meta.0.insert(msg);
                            }
                            continue;
                        }
                        op["operationId"] = json!(id);
                        if paths[&cleaned].is_null() {
                            paths[&cleaned] = json!({});
                        }
                        paths[&cleaned][verb.to_lowercase()] = op.clone();
                        let mut ep_notices = schemas.op_notices.clone();
                        if let Some(msg) = placeholder_notice {
                            ep_notices.insert(msg);
                        }
                        endpoints.push(Endpoint {
                            method: verb.clone(),
                            path: cleaned,
                            handler: format!("{}.{}", c.name, m.name),
                            file: c.path.clone(),
                            declaration_file: em.is_inherited.then(|| declaration.path.clone()),
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
                            controller_name: op["x-apifox-folder"]
                                .as_str()
                                .unwrap_or(&c.name)
                                .to_string(),
                            operation: op,
                            warnings: Vec::new(),
                            notices: Vec::new(),
                        });
                        endpoint_meta.push((
                            schemas.op_warnings.clone(),
                            ep_notices,
                            schemas.op_touched.clone(),
                        ));
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
    let mut mapped_warnings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (key, msgs) in schemas.component_warnings {
        let final_name = names.get(&key).cloned().unwrap_or(key);
        mapped_warnings.entry(final_name).or_default().extend(msgs);
    }
    let mut mapped_notices: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (key, msgs) in schemas.component_notices {
        let final_name = names.get(&key).cloned().unwrap_or(key);
        mapped_notices.entry(final_name).or_default().extend(msgs);
    }
    let mut warnings: Vec<_> = schemas.warnings.into_iter().collect();
    let has_component_diagnostics = !mapped_warnings.is_empty() || !mapped_notices.is_empty();
    let schema_refs: HashMap<&str, Vec<&str>> = if has_component_diagnostics {
        components
            .iter()
            .map(|(name, schema)| {
                let mut refs = Vec::new();
                collect_direct_refs(schema, &mut refs);
                (name.as_str(), refs)
            })
            .collect()
    } else {
        HashMap::new()
    };
    for (ep, (mut ep_w, mut ep_n, touched)) in endpoints.iter_mut().zip(endpoint_meta) {
        for p in ep.operation["parameters"].as_array().into_iter().flatten() {
            if p["in"] == "path"
                && !ep
                    .path
                    .contains(&format!("{{{}}}", p["name"].as_str().unwrap_or("")))
            {
                let msg = format!("路径参数 {} 与路由 {} 不匹配", p["name"], ep.path);
                warnings.push(msg.clone());
                ep_w.insert(msg);
            }
        }
        if has_component_diagnostics {
            let seed: Vec<String> = touched
                .into_iter()
                .map(|k| names.get(&k).cloned().unwrap_or(k))
                .collect();
            let mut visited: HashSet<&str> = HashSet::new();
            let mut queue: Vec<&str> = Vec::new();
            for s in &seed {
                if visited.insert(s.as_str()) {
                    queue.push(s.as_str());
                }
            }
            let mut op_refs = Vec::new();
            collect_direct_refs(&ep.operation, &mut op_refs);
            for r in op_refs {
                if visited.insert(r) {
                    queue.push(r);
                }
            }
            while let Some(curr) = queue.pop() {
                if let Some(nexts) = schema_refs.get(curr) {
                    for &next in nexts {
                        if visited.insert(next) {
                            queue.push(next);
                        }
                    }
                }
            }
            for sname in visited {
                if let Some(msgs) = mapped_warnings.get(sname) {
                    ep_w.extend(msgs.iter().cloned());
                }
                if let Some(msgs) = mapped_notices.get(sname) {
                    ep_n.extend(msgs.iter().cloned());
                }
            }
        }
        ep.warnings = ep_w.into_iter().collect();
        ep.notices = ep_n.into_iter().collect();
    }
    Export {
        detected_url: project.debug_url(),
        document: json!({"openapi":"3.0.3","info":{"title":title,"version":"1.0.0"},"paths":paths,"components":{"schemas":components}}),
        endpoints,
        warnings,
        notices: schemas.notices.into_iter().collect(),
    }
}
