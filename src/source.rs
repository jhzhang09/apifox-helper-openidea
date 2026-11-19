//! 本地源码解析：按词法边界处理注解、泛型和声明，不执行项目代码。
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Token {
    pub text: String,
    pub line: u32,
}
#[derive(Clone, Debug, Default)]
pub struct Annotation {
    pub name: String,
    pub full_name: String,
    pub args: BTreeMap<String, Vec<String>>,
    pub nested: BTreeMap<String, Vec<Annotation>>,
    pub expressions: BTreeSet<String>,
    pub item_exprs: BTreeMap<String, Vec<bool>>,
}
impl Annotation {
    pub fn values(&self, key: &str) -> Vec<String> {
        self.args.get(key).cloned().unwrap_or_default()
    }
    pub fn value(&self, key: &str) -> Option<String> {
        self.values(key).into_iter().next()
    }
    pub fn is_expr_item(&self, key: &str, index: usize) -> bool {
        self.item_exprs
            .get(key)
            .and_then(|flags| flags.get(index))
            .copied()
            .unwrap_or_else(|| self.expressions.contains(key))
    }
    /// 说明文字只接受非空字面值，尚未求值的常量不能替换有效文档。
    pub fn text(&self, key: &str) -> Option<String> {
        if self.expressions.contains(key) {
            return None;
        }
        self.value(key).and_then(|v| nonempty(&v))
    }
    pub fn children(&self, key: &str) -> &[Annotation] {
        self.nested.get(key).map(Vec::as_slice).unwrap_or_default()
    }
}
pub fn nonempty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}
/// 按调用方给定的优先级取说明，空白值继续回退。
pub fn annotation_text(annotations: &[Annotation], choices: &[(&str, &str)]) -> Option<String> {
    choices.iter().find_map(|(name, key)| {
        annotations
            .iter()
            .filter(|a| a.name == *name)
            .find_map(|a| a.text(key))
    })
}
#[derive(Clone, Debug, Default)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub annotations: Vec<Annotation>,
    pub doc: String,
}
#[derive(Clone, Debug, Default)]
pub struct Method {
    pub returned_field: Option<String>,
    pub name: String,
    pub ty: String,
    pub annotations: Vec<Annotation>,
    pub params: Vec<Field>,
    pub doc: String,
    pub line: u32,
    pub end_line: u32,
}
#[derive(Clone, Debug, Default)]
pub struct EnumVariant {
    pub name: String,
    pub doc: String,
    pub annotations: Vec<Annotation>,
    pub args: Vec<String>,
    pub arg_literals: Vec<bool>,
    pub arg_strings: Vec<bool>,
}
#[derive(Clone, Debug, Default)]
pub struct Class {
    pub name: String,
    pub package: String,
    pub imports: Vec<String>,
    pub static_imports: Vec<String>,
    pub path: String,
    pub annotations: Vec<Annotation>,
    pub doc: String,
    pub generics: Vec<String>,
    pub bases: Vec<String>,
    pub interfaces: Vec<String>,
    pub constants: BTreeMap<String, String>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub variants: Vec<String>,
    pub enum_items: Vec<EnumVariant>,
    pub enum_constructors: Vec<BTreeMap<String, usize>>,
}
impl Class {
    pub fn qualified(&self) -> String {
        if self.package.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.package, self.name)
        }
    }
}

pub fn annotation<'a>(a: &'a [Annotation], name: &str) -> Option<&'a Annotation> {
    a.iter().find(|a| a.name == name)
}
pub fn has(a: &[Annotation], name: &str) -> bool {
    annotation(a, name).is_some()
}

/// 扫描只收录业务源码；不按包名中的 test 字样排除正常生产接口。
pub fn ignored_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    let parts: Vec<_> = path.split('/').collect();
    parts
        .iter()
        .any(|p| matches!(*p, "target" | "build" | ".git" | "node_modules" | ".gradle"))
        || parts.windows(2).any(|p| {
            p[0] == "src"
                && matches!(
                    p[1],
                    "test"
                        | "testFixtures"
                        | "integrationTest"
                        | "integration-test"
                        | "androidTest"
                        | "testIntegration"
                )
        })
        || matches!(parts.first(), Some(&"test" | &"tests"))
}
pub fn production_source(path: &str) -> bool {
    (path.ends_with(".java") || path.ends_with(".kt")) && !ignored_path(path)
}

pub fn lex(source: &str) -> Vec<Token> {
    let c: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 0);
    while i < c.len() {
        let start = i;
        let at = line;
        if c[i].is_whitespace() {
            if c[i] == '\n' {
                line += 1;
            }
            i += 1;
            continue;
        }
        if c[i] == '/' && c.get(i + 1) == Some(&'/') {
            let doc = c.get(i + 2) == Some(&'/');
            i += 2;
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            if doc {
                out.push(Token {
                    text: format!("/** {} */", c[start + 3..i].iter().collect::<String>()),
                    line: at,
                });
            }
            continue;
        }
        if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let doc = c.get(i + 2) == Some(&'*');
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(c.len());
            if doc {
                out.push(Token {
                    text: c[start..i].iter().collect(),
                    line: at,
                });
            }
            continue;
        }
        if c[i] == '"' || c[i] == '\'' || c[i] == '`' {
            let q = c[i];
            i += 1;
            while i < c.len() {
                if c[i] == '\n' {
                    line += 1;
                }
                if c[i] == '\\' {
                    i = (i + 2).min(c.len());
                    continue;
                }
                if c[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if c[i].is_alphanumeric() || c[i] == '_' || c[i] == '$' {
            i += 1;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '$') {
                i += 1;
            }
        } else {
            i += 1;
        }
        out.push(Token {
            text: c[start..i].iter().collect(),
            line: at,
        });
    }
    out
}
fn t(ts: &[Token], i: usize) -> &str {
    ts.get(i).map(|t| t.text.as_str()).unwrap_or("")
}
pub fn paired(ts: &[Token], start: usize) -> usize {
    let close = match t(ts, start) {
        "(" => ")",
        "{" => "}",
        "[" => "]",
        "<" => ">",
        _ => return start,
    };
    let mut depth = 0;
    for i in start..ts.len() {
        if t(ts, i) == t(ts, start) {
            depth += 1;
        }
        if t(ts, i) == close {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    ts.len()
}
pub fn split<'a>(ts: &'a [Token], delimiter: &str) -> Vec<&'a [Token]> {
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < ts.len() {
        if matches!(t(ts, i), "(" | "{" | "[" | "<") {
            i = paired(ts, i) + 1;
            continue;
        }
        if t(ts, i) == delimiter {
            out.push(&ts[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    out.push(&ts[start..]);
    out
}
fn joined(ts: &[Token]) -> String {
    ts.iter().map(|x| x.text.as_str()).collect()
}
fn without_type_annotations(ts: &[Token]) -> Vec<Token> {
    let mut clean = Vec::new();
    let mut i = 0;
    while i < ts.len() {
        if t(ts, i) == "@" {
            let (_, end) = parse_annotation(ts, i);
            i = end;
        } else {
            clean.push(ts[i].clone());
            i += 1;
        }
    }
    clean
}
fn unquote(s: &str) -> String {
    serde_json::from_str::<String>(s)
        .unwrap_or_else(|_| s.trim_matches(['"', '\'', '`']).to_string())
}
fn parse_annotation_elem(elem: &[Token]) -> (Option<String>, bool) {
    let mut inner = elem;
    if t(inner, 0) == "(" && paired(inner, 0) + 1 == inner.len() {
        inner = &inner[1..inner.len() - 1];
    }
    if inner.is_empty() {
        return (None, false);
    }
    let parts = split(inner, "+");
    let all_strings = parts.iter().all(|p| {
        p.len() == 1
            && (p[0].text.starts_with('"') || (parts.len() > 1 && p[0].text.starts_with('\'')))
    });
    if all_strings {
        let combined: String = parts.iter().map(|p| unquote(&p[0].text)).collect();
        return (Some(combined), false);
    }
    if parts.len() == 1 {
        let p = parts[0];
        if p.len() == 1 && (p[0].text.starts_with('"') || p[0].text.starts_with('\'')) {
            return (Some(unquote(&p[0].text)), false);
        }
        return (Some(joined(p)), true);
    }
    let mut expr_parts = Vec::with_capacity(parts.len());
    for p in parts {
        if p.len() == 1 && (p[0].text.starts_with('"') || p[0].text.starts_with('\'')) {
            let Ok(lit) = serde_json::to_string(&unquote(&p[0].text)) else {
                return (Some(joined(inner)), true);
            };
            expr_parts.push(lit);
        } else {
            expr_parts.push(joined(p));
        }
    }
    (Some(expr_parts.join("+")), true)
}

fn unwrap_array_values(values: &[Token]) -> &[Token] {
    if values.len() >= 2
        && matches!(t(values, 0), "{" | "[")
        && paired(values, 0) + 1 == values.len()
    {
        return &values[1..values.len() - 1];
    }
    if values.len() >= 3
        && t(values, 0) == "arrayOf"
        && t(values, 1) == "("
        && paired(values, 1) + 1 == values.len()
    {
        return &values[2..values.len() - 1];
    }
    values
}

fn parse_annotation(ts: &[Token], start: usize) -> (Annotation, usize) {
    let mut i = start + 1;
    let mut name = t(ts, i).to_string();
    i += 1;
    // Kotlin 的 @field:Schema、@get:JsonProperty 使用同一个字段契约。
    if t(ts, i) == ":" {
        i += 1;
        name = t(ts, i).to_string();
        i += 1;
    }
    let mut full_name = name.clone();
    while t(ts, i) == "." {
        name = t(ts, i + 1).to_string();
        full_name.push('.');
        full_name.push_str(&name);
        i += 2;
    }
    let mut a = Annotation {
        name,
        full_name,
        args: BTreeMap::new(),
        nested: BTreeMap::new(),
        expressions: BTreeSet::new(),
        item_exprs: BTreeMap::new(),
    };
    if t(ts, i) == "(" {
        let end = paired(ts, i);
        for part in split(&ts[i + 1..end], ",") {
            let mut eq = None;
            let mut p = 0;
            while p < part.len() {
                if t(part, p) == "=" {
                    eq = Some(p);
                    break;
                }
                if matches!(t(part, p), "(" | "{" | "[") {
                    p = paired(part, p) + 1;
                } else {
                    p += 1;
                }
            }
            let (key, values) = eq
                .map(|p| (joined(&part[..p]), &part[p + 1..]))
                .unwrap_or(("value".into(), part));
            let mut vals = Vec::new();
            let mut item_expr = Vec::new();
            let mut nested = Vec::new();
            let mut has_expr = false;
            for elem in split(unwrap_array_values(values), ",") {
                if elem.is_empty() {
                    continue;
                }
                if t(elem, 0) == "@" {
                    let mut n = 0;
                    while n < elem.len() {
                        if t(elem, n) == "@" {
                            let (child, child_end) = parse_annotation(elem, n);
                            nested.push(child);
                            n = child_end;
                        } else {
                            n += 1;
                        }
                    }
                    continue;
                }
                if let (Some(v), is_expr) = parse_annotation_elem(elem) {
                    vals.push(v);
                    item_expr.push(is_expr);
                    has_expr |= is_expr;
                }
            }
            if has_expr {
                a.expressions.insert(key.clone());
            }
            if !item_expr.is_empty() {
                a.item_exprs.insert(key.clone(), item_expr);
            }
            if !nested.is_empty() {
                a.nested.insert(key.clone(), nested);
            }
            a.args.insert(key, vals);
        }
        i = end + 1;
    }
    (a, i)
}
fn doc(s: &str) -> String {
    s.trim_start_matches("/**")
        .trim_end_matches("*/")
        .lines()
        .map(|s| s.trim().trim_start_matches('*').trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
fn prefix(ts: &[Token], mut i: usize) -> (Vec<Annotation>, String, usize) {
    let mut anns = Vec::new();
    let mut docs = Vec::new();
    while i < ts.len() {
        if t(ts, i).starts_with("/**") {
            docs.push(doc(t(ts, i)));
            i += 1;
        } else if t(ts, i) == "@" {
            let (a, n) = parse_annotation(ts, i);
            anns.push(a);
            i = n;
        } else if matches!(
            t(ts, i),
            "public"
                | "private"
                | "protected"
                | "internal"
                | "static"
                | "final"
                | "abstract"
                | "override"
                | "open"
                | "suspend"
                | "inline"
                | "lateinit"
                | "transient"
                | "volatile"
                | "synchronized"
                | "native"
                | "default"
                | "external"
                | "actual"
                | "expect"
                | "const"
        ) {
            i += 1;
        } else {
            break;
        }
    }
    (anns, docs.join("\n"), i)
}
fn field(ts: &[Token], kotlin: bool) -> Option<Field> {
    let (a, d, mut i) = prefix(ts, 0);
    if matches!(t(ts, i), "val" | "var" | "vararg") {
        i += 1;
    }
    if i >= ts.len() {
        return None;
    }
    // 类型位置的注解不能参与变量名、默认值或泛型类型的识别。
    let clean = without_type_annotations(&ts[i..]);
    let rest = split(&clean, "=").into_iter().next()?;
    let (name, ty) = if kotlin {
        let colon = rest.iter().position(|x| x.text == ":")?;
        (
            t(rest, 0).trim_matches('`').to_string(),
            joined(&rest[colon + 1..]),
        )
    } else {
        let pos = rest.iter().rposition(|x| {
            x.text
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        })?;
        (
            t(rest, pos).to_string(),
            format!("{}{}", joined(&rest[..pos]), joined(&rest[pos + 1..])),
        )
    };
    Some(Field {
        name,
        ty,
        annotations: a,
        doc: d,
    })
}

pub fn parse(path: &str, source: &str) -> Vec<Class> {
    let ts = lex(source);
    if !syntax_issues(&ts).is_empty() {
        return Vec::new();
    }
    let mut classes = parse_tokens(path, &ts, "", &[], &[], "");
    for class in &mut classes {
        qualify_annotations(&mut class.annotations, &class.imports);
        for item in &mut class.enum_items {
            qualify_annotations(&mut item.annotations, &class.imports);
        }
        for field in &mut class.fields {
            qualify_annotations(&mut field.annotations, &class.imports);
        }
        for method in &mut class.methods {
            qualify_annotations(&mut method.annotations, &class.imports);
            for param in &mut method.params {
                qualify_annotations(&mut param.annotations, &class.imports);
            }
        }
    }
    classes
}

fn qualify_annotations(annotations: &mut [Annotation], imports: &[String]) {
    for a in annotations {
        if !a.full_name.contains('.') {
            if let Some(import) = imports
                .iter()
                .find(|i| i.rsplit('.').next() == Some(a.name.as_str()))
            {
                a.full_name = import.clone();
            } else if a.name == "RequestBody" {
                let candidates: Vec<_> = [
                    "org.springframework.web.bind.annotation",
                    "io.swagger.v3.oas.annotations.parameters",
                ]
                .iter()
                .filter(|p| imports.iter().any(|i| i == &format!("{p}.*")))
                .collect();
                if candidates.len() == 1 {
                    a.full_name = format!("{}.RequestBody", candidates[0]);
                }
            }
        }
        for children in a.nested.values_mut() {
            qualify_annotations(children, imports);
        }
    }
}

fn eval_enum_arg(slice: &[Token]) -> (String, bool, bool) {
    let mut inner = slice;
    if t(inner, 0) == "(" && paired(inner, 0) + 1 == inner.len() {
        inner = &inner[1..inner.len() - 1];
    }
    if inner.is_empty() {
        return (String::new(), false, false);
    }
    let parts = split(inner, "+");
    if parts.iter().all(|p| {
        p.len() == 1
            && (p[0].text.starts_with('"') || (parts.len() > 1 && p[0].text.starts_with('\'')))
    }) {
        let combined: String = parts.iter().map(|p| unquote(&p[0].text)).collect();
        return (combined, true, true);
    }
    if inner.len() == 1 && matches!(inner[0].text.as_str(), "true" | "false") {
        return (inner[0].text.clone(), true, false);
    }
    if inner.len() == 1 && inner[0].text.starts_with('\'') {
        return (unquote(&inner[0].text), true, true);
    }
    let raw = joined(inner);
    let trimmed = raw.trim_end_matches(['L', 'l', 'F', 'f', 'D', 'd']);
    let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
    if digits.chars().next().is_some_and(|c| c.is_ascii_digit())
        && (trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok())
    {
        return (trimmed.to_string(), true, false);
    }
    (String::new(), false, false)
}

fn extract_const_expr(ts: &[Token]) -> Option<String> {
    if ts.is_empty()
        || ts
            .iter()
            .any(|tok| matches!(tok.text.as_str(), ";" | "{" | "}" | "(" | ")"))
    {
        return None;
    }
    let parts = split(ts, "+");
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty() {
            return None;
        }
        if part.len() == 1 && (part[0].text.starts_with('"') || part[0].text.starts_with('\'')) {
            out.push(serde_json::to_string(&unquote(&part[0].text)).ok()?);
        } else if part.first().is_some_and(|tok| {
            tok.text != "."
                && tok
                    .text
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        }) && part.last().is_some_and(|tok| tok.text != ".")
            && part.iter().all(|tok| {
                tok.text == "."
                    || tok
                        .text
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
            })
        {
            out.push(joined(part));
        } else {
            return None;
        }
    }
    Some(out.join("+"))
}

fn parse_kotlin_val_stmt(ts: &[Token], n: usize, limit: usize) -> usize {
    let mut e = n + 1;
    let mut paren = 0usize;
    while e < limit && t(ts, e) != ";" {
        if e > n + 1
            && paren == 0
            && ts[e].line > ts[e - 1].line
            && !matches!(t(ts, e - 1), "+" | "=")
            && t(ts, e) != "+"
        {
            break;
        }
        match t(ts, e) {
            "(" | "<" | "[" => paren += 1,
            ")" | ">" | "]" if paren > 0 => paren -= 1,
            _ => (),
        }
        e += 1;
    }
    e
}

fn extract_kotlin_constant(decl: &[Token], allow_plain: bool) -> Option<(String, String)> {
    if !allow_plain || decl.len() < 4 || t(decl, 0) != "val" {
        return None;
    }
    let name = t(decl, 1).trim_matches('`').to_string();
    if t(decl, 2) == "=" {
        return extract_const_expr(&decl[3..]).map(|expr| (name, expr));
    }
    if t(decl, 2) == ":"
        && let Some(eq) = decl[3..].iter().position(|tok| tok.text == "=")
    {
        let ty = joined(&without_type_annotations(&decl[3..3 + eq]));
        if ty.rsplit('.').next() == Some("String") || ty == "CharSequence" {
            return extract_const_expr(&decl[3 + eq + 1..]).map(|expr| (name, expr));
        }
    }
    None
}

fn parse_base_clauses(
    slice: &[Token],
    is_interface: bool,
    kotlin: bool,
    bases: &mut Vec<String>,
    interfaces: &mut Vec<String>,
) {
    if kotlin {
        let Some(colon) = slice.iter().position(|x| x.text == ":") else {
            return;
        };
        let mut end = slice.len();
        let mut p = colon + 1;
        while p < slice.len() {
            if matches!(t(slice, p), "(" | "<") {
                p = paired(slice, p) + 1;
                continue;
            }
            if t(slice, p) == "where" {
                end = p;
                break;
            }
            p += 1;
        }
        for v in split(&slice[colon + 1..end], ",") {
            let cut = v.iter().position(|x| x.text == "(").unwrap_or(v.len());
            if cut > 0 {
                let name = joined(&without_type_annotations(&v[..cut]));
                if !name.is_empty() {
                    if is_interface {
                        interfaces.push(name);
                    } else {
                        bases.push(name);
                    }
                }
            }
        }
        return;
    }
    let (mut ext, mut imp, mut permits) = (None, None, None);
    let mut p = 0;
    while p < slice.len() {
        if matches!(t(slice, p), "(" | "<") {
            p = paired(slice, p) + 1;
            continue;
        }
        match t(slice, p) {
            "extends" if ext.is_none() => ext = Some(p),
            "implements" if imp.is_none() => imp = Some(p),
            "permits" if permits.is_none() => permits = Some(p),
            _ => (),
        }
        p += 1;
    }
    if let Some(start) = ext {
        let end = imp.or(permits).unwrap_or(slice.len());
        if start + 1 < end {
            for v in split(&slice[start + 1..end], ",") {
                let name = joined(&without_type_annotations(v));
                if !name.is_empty() {
                    if is_interface {
                        interfaces.push(name);
                    } else {
                        bases.push(name);
                    }
                }
            }
        }
    }
    if let Some(start) = imp {
        let end = permits.unwrap_or(slice.len());
        if start + 1 < end {
            for v in split(&slice[start + 1..end], ",") {
                let name = joined(&without_type_annotations(v));
                if !name.is_empty() {
                    interfaces.push(name);
                }
            }
        }
    }
}

fn parse_tokens(
    path: &str,
    ts: &[Token],
    inherited_package: &str,
    inherited_imports: &[String],
    inherited_static_imports: &[String],
    outer: &str,
) -> Vec<Class> {
    let kotlin = path.ends_with(".kt");
    let mut package = inherited_package.to_string();
    let mut imports = inherited_imports.to_vec();
    let mut static_imports = inherited_static_imports.to_vec();
    let mut i = 0;
    while i < ts.len() {
        if matches!(t(ts, i), "package" | "import") {
            let imp = t(ts, i) == "import";
            i += 1;
            let is_static = imp && t(ts, i) == "static";
            if is_static {
                i += 1;
            }
            let start = i;
            while i < ts.len() && t(ts, i) != ";" && (i == start || ts[i].line == ts[start].line) {
                i += 1;
            }
            let s = joined(&ts[start..i]);
            if is_static {
                static_imports.push(s);
            } else if imp {
                imports.push(s);
            } else {
                package = s;
            }
        } else {
            i += 1;
        }
    }
    let mut classes = Vec::new();
    let mut file_constants = BTreeMap::new();
    i = 0;
    while i < ts.len() {
        let (a, d, n) = prefix(ts, i);
        if kotlin && outer.is_empty() && matches!(t(ts, n), "val" | "var") {
            let e = parse_kotlin_val_stmt(ts, n, ts.len());
            if let Some((k, v)) = extract_kotlin_constant(&ts[n..e], true) {
                file_constants.entry(k).or_insert(v);
            }
            i = if t(ts, e) == ";" { e + 1 } else { e.max(n + 1) };
            continue;
        }
        let mut cstart = n;
        if matches!(t(ts, cstart), "data" | "enum" | "annotation" | "sealed")
            && t(ts, cstart + 1) == "class"
        {
            cstart += 1;
        }
        let is_named_object =
            kotlin && t(ts, cstart) == "object" && !matches!(t(ts, cstart + 1), "" | ":" | "{");
        if !matches!(t(ts, cstart), "class" | "interface" | "record" | "enum") && !is_named_object {
            i = n.max(i + 1);
            continue;
        }
        if cstart + 1 >= ts.len() {
            return classes;
        }
        let mut c = Class {
            name: format!("{outer}{}", t(ts, cstart + 1)),
            package: package.clone(),
            imports: imports.clone(),
            static_imports: static_imports.clone(),
            path: path.into(),
            annotations: a,
            doc: d,
            ..Default::default()
        };
        let is_enum = t(ts, cstart) == "enum" || t(ts, cstart.saturating_sub(1)) == "enum";
        let is_interface = t(ts, cstart) == "interface";
        let mut p = (cstart + 2).min(ts.len());
        if t(ts, p) == "<" {
            let e = paired(ts, p);
            if e >= ts.len() {
                return classes;
            }
            c.generics = split(&ts[p + 1..e], ",")
                .iter()
                .map(|v| t(v, 0).to_string())
                .collect();
            p = e + 1;
        }
        if t(ts, p) == "(" {
            let e = paired(ts, p);
            c.fields = split(&ts[p + 1..e], ",")
                .iter()
                .filter_map(|v| field(v, kotlin))
                .collect();
            p = e + 1;
        }
        let base_start = p;
        while p < ts.len() && !matches!(t(ts, p), "{" | ";") {
            if kotlin
                && ts[p].line > ts[cstart].line
                && matches!(t(ts, p), "@" | "class" | "data" | "interface" | "object")
            {
                break;
            }
            p += 1;
        }
        parse_base_clauses(
            &ts[base_start..p],
            is_interface,
            kotlin,
            &mut c.bases,
            &mut c.interfaces,
        );
        if t(ts, p) != "{" {
            classes.push(c);
            i = p.max(cstart + 2);
            continue;
        }
        let body_end = paired(ts, p);
        let mut m = p + 1;
        if is_enum {
            let start = m;
            while m < body_end && !matches!(t(ts, m), ";" | "}") {
                if matches!(t(ts, m), "(" | "{") {
                    m = paired(ts, m);
                }
                m += 1;
            }
            for v in split(&ts[start..m], ",") {
                let (anns, doc, idx) = prefix(v, 0);
                if idx >= v.len() || matches!(t(v, idx), ";" | "}") {
                    continue;
                }
                let name = t(v, idx).to_string();
                if !name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_alphabetic() || ch == '_' || ch == '$')
                {
                    continue;
                }
                let mut args = Vec::new();
                let mut arg_literals = Vec::new();
                let mut arg_strings = Vec::new();
                if t(v, idx + 1) == "(" {
                    let arg_end = paired(v, idx + 1);
                    if arg_end < v.len() {
                        for arg_slice in split(&v[idx + 2..arg_end], ",") {
                            let (val, lit, is_str) = eval_enum_arg(arg_slice);
                            args.push(val);
                            arg_literals.push(lit);
                            arg_strings.push(is_str);
                        }
                    }
                }
                c.variants.push(name.clone());
                c.enum_items.push(EnumVariant {
                    name,
                    doc,
                    annotations: anns,
                    args,
                    arg_literals,
                    arg_strings,
                });
            }
            if t(ts, m) == ";" {
                m += 1;
            }
        }
        while m < body_end {
            if t(ts, m) == ";" {
                m += 1;
                continue;
            }
            let (anns, docs, n) = prefix(ts, m);
            if n >= body_end {
                break;
            }
            if t(ts, n) == "{" {
                m = paired(ts, n) + 1;
                continue;
            }
            if matches!(t(ts, n), "class" | "interface" | "enum" | "record")
                || (kotlin
                    && (matches!(t(ts, n), "object" | "companion")
                        || (t(ts, n) == "data" && t(ts, n + 1) == "class")))
            {
                let mut e = n;
                while e < body_end && t(ts, e) != "{" {
                    e += 1;
                }
                let end = if e < body_end {
                    paired(ts, e) + 1
                } else {
                    body_end
                };
                if t(ts, n) == "companion" && e < body_end {
                    let mut companion_constants = BTreeMap::new();
                    let mut cm = e + 1;
                    while cm < end - 1 {
                        let (_, _, cn) = prefix(ts, cm);
                        if cn >= end - 1 {
                            break;
                        }
                        if matches!(t(ts, cn), "val" | "var") {
                            let ce = parse_kotlin_val_stmt(ts, cn, end - 1);
                            if let Some((k, v)) = extract_kotlin_constant(&ts[cn..ce], true) {
                                companion_constants.insert(k, v);
                            }
                            cm = if t(ts, ce) == ";" {
                                ce + 1
                            } else {
                                ce.max(cn + 1)
                            };
                        } else if t(ts, cn) == "{" {
                            cm = paired(ts, cn) + 1;
                        } else {
                            cm = cn + 1;
                        }
                    }
                    for (k, v) in &companion_constants {
                        c.constants.insert(k.clone(), v.clone());
                    }
                    if t(ts, n + 1) == "object"
                        && !matches!(t(ts, n + 2), "" | ":" | "{")
                        && !companion_constants.is_empty()
                    {
                        classes.push(Class {
                            name: format!("{}.{}", c.name, t(ts, n + 2)),
                            package: package.clone(),
                            imports: imports.clone(),
                            static_imports: static_imports.clone(),
                            path: path.into(),
                            constants: companion_constants,
                            ..Default::default()
                        });
                    }
                } else if t(ts, n) == "object" {
                    if !matches!(t(ts, n + 1), "" | ":" | "{") {
                        classes.extend(parse_tokens(
                            path,
                            &ts[m..end],
                            &package,
                            &imports,
                            &static_imports,
                            &format!("{}.", c.name),
                        ));
                    }
                } else if t(ts, n) != "companion" {
                    classes.extend(parse_tokens(
                        path,
                        &ts[m..end],
                        &package,
                        &imports,
                        &static_imports,
                        &format!("{}.", c.name),
                    ));
                }
                m = end;
                continue;
            }
            if kotlin && matches!(t(ts, n), "val" | "var") {
                let is_const = ts[m..n].iter().any(|x| x.text == "const");
                let e = parse_kotlin_val_stmt(ts, n, body_end);
                if (is_const || is_named_object)
                    && let Some((k, v)) = extract_kotlin_constant(&ts[n..e], true)
                {
                    c.constants.insert(k, v);
                }
                if !is_const
                    && !is_named_object
                    && let Some(mut f) = field(&ts[n..e], true)
                {
                    f.annotations = anns;
                    f.doc = docs;
                    c.fields.push(f);
                }
                m = if t(ts, e) == ";" { e + 1 } else { e.max(n + 1) };
                continue;
            }
            let mut par = n;
            while par < body_end && !matches!(t(ts, par), "(" | ";" | "{" | "=") {
                if t(ts, par) == "<" {
                    let end = paired(ts, par);
                    if end >= body_end {
                        par = body_end;
                        break;
                    }
                    par = end;
                }
                par += 1;
            }
            if t(ts, par) == "(" {
                let end = paired(ts, par);
                let name = t(ts, par - 1).to_string();
                let mut after = end + 1;
                let ty = if kotlin {
                    if t(ts, after) == ":" {
                        after += 1;
                        let s = after;
                        while after < body_end
                            && !matches!(t(ts, after), "{" | "=" | ";")
                            && !(ts[after].line > ts[end].line
                                && matches!(t(ts, after), "@" | "fun" | "override"))
                        {
                            after += 1;
                        }
                        joined(&ts[s..after])
                    } else {
                        "Unit".into()
                    }
                } else {
                    joined(&ts[n..par - 1])
                };
                while after < body_end && !matches!(t(ts, after), "{" | ";" | "=") {
                    if kotlin && ts[after].line > ts[end].line {
                        break;
                    }
                    after += 1;
                }
                let finish = if t(ts, after) == "{" {
                    paired(ts, after) + 1
                } else if t(ts, after) == "=" {
                    let mut e = after + 1;
                    while e < body_end && ts[e].line == ts[after].line {
                        if t(ts, e) == "{" {
                            e = paired(ts, e);
                        }
                        e += 1;
                    }
                    e
                } else if t(ts, after) == ";" {
                    after + 1
                } else {
                    after
                };
                let body: Vec<_> = if is_enum
                    && t(ts, after) == "{"
                    && finish <= ts.len()
                    && finish >= after + 2
                {
                    ts[after + 1..finish - 1]
                        .iter()
                        .map(|token| token.text.as_str())
                        .collect()
                } else {
                    Vec::new()
                };
                if is_enum && name == c.name.rsplit('.').next().unwrap_or(&c.name) {
                    let params = split(&ts[par + 1..end], ",");
                    let names: Vec<_> = params
                        .iter()
                        .map(|param| field(param, kotlin).map(|param| param.name))
                        .collect();
                    let mut bindings = BTreeMap::new();
                    for statement in body.split_inclusive(|token| *token == ";") {
                        let (target, source) = match statement {
                            ["this", ".", target, "=", source, ";"] => (target, source),
                            [target, "=", source, ";"]
                                if !names.iter().any(|name| name.as_deref() == Some(*target)) =>
                            {
                                (target, source)
                            }
                            _ => {
                                bindings.clear();
                                break;
                            }
                        };
                        if let Some(index) = names
                            .iter()
                            .position(|name| name.as_deref() == Some(*source))
                        {
                            if bindings.insert((*target).to_owned(), index).is_some() {
                                bindings.clear();
                                break;
                            }
                        } else {
                            bindings.clear();
                            break;
                        }
                    }
                    c.enum_constructors.push(bindings);
                }
                if name != c.name.rsplit('.').next().unwrap_or(&c.name) {
                    c.methods.push(Method {
                        returned_field: match body.as_slice() {
                            ["return", field, ";"] | ["return", "this", ".", field, ";"] => {
                                Some((*field).to_owned())
                            }
                            _ => None,
                        },
                        name,
                        ty,
                        annotations: anns,
                        params: split(&ts[par + 1..end], ",")
                            .iter()
                            .filter_map(|p| field(p, kotlin))
                            .collect(),
                        doc: docs,
                        line: ts[m].line,
                        end_line: ts[finish.saturating_sub(1).min(ts.len() - 1)].line,
                    });
                }
                m = finish.max(end + 1);
            } else {
                let mut end = par;
                while end < body_end && t(ts, end) != ";" {
                    if t(ts, end) == "{" {
                        end = paired(ts, end);
                    }
                    end += 1;
                }
                let is_static = ts[m..end].iter().any(|x| x.text == "static");
                let is_transient = ts[m..end].iter().any(|x| x.text == "transient");
                if (is_static || is_interface)
                    && !kotlin
                    && t(ts, par) == "="
                    && let Some(f) = field(&ts[n..end], false)
                    && (f.ty.rsplit('.').next() == Some("String") || f.ty == "CharSequence")
                    && let Some(expr) = extract_const_expr(&ts[par + 1..end])
                {
                    c.constants.insert(f.name, expr);
                }
                if !is_interface
                    && !is_static
                    && !is_transient
                    && let Some(mut f) = field(&ts[n..end], false)
                {
                    f.annotations = anns;
                    f.doc = docs;
                    c.fields.push(f);
                }
                m = end + 1;
            }
        }
        classes.push(c);
        i = body_end + 1;
    }
    if !file_constants.is_empty() {
        for class in &mut classes {
            for (k, v) in &file_constants {
                class
                    .constants
                    .entry(k.clone())
                    .or_insert_with(|| v.clone());
            }
        }
    }
    classes
}

pub fn summary(doc: &str) -> String {
    doc.lines()
        .take_while(|l| !l.starts_with('@'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
pub fn tag(doc: &str, name: &str) -> Option<String> {
    let expected: Vec<_> = name.split_whitespace().collect();
    let lines: Vec<_> = doc.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let Some(mut rest) = line.trim().strip_prefix('@') else {
            continue;
        };
        let mut matches = true;
        for word in &expected {
            rest = rest.trim_start();
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            if &rest[..end] != *word {
                matches = false;
                break;
            }
            rest = &rest[end..];
        }
        if matches {
            let mut parts = vec![rest.trim()];
            for continuation in &lines[index + 1..] {
                if continuation.trim_start().starts_with('@') {
                    break;
                }
                parts.push(continuation.trim());
            }
            if let Some(value) = nonempty(&parts.join("\n")) {
                return Some(value);
            }
        }
    }
    None
}

/// 尚未闭合的编辑缓冲区不进入模型解析，调用方可以展示具体行号。
pub fn issues(source: &str) -> Vec<String> {
    syntax_issues(&lex(source))
}
fn syntax_issues(ts: &[Token]) -> Vec<String> {
    let mut stack = Vec::new();
    for t in ts {
        match t.text.as_str() {
            "(" | "[" | "{" => stack.push(t),
            ")" | "]" | "}" => {
                let Some(open) = stack.pop() else {
                    return vec![format!("第 {} 行存在未匹配的 {}", t.line + 1, t.text)];
                };
                if !matches!(
                    (open.text.as_str(), t.text.as_str()),
                    ("(", ")") | ("[", "]") | ("{", "}")
                ) {
                    return vec![format!("第 {} 行括号不匹配", t.line + 1)];
                }
            }
            _ => (),
        }
    }
    stack
        .into_iter()
        .map(|t| format!("第 {} 行 {} 未闭合", t.line + 1, t.text))
        .collect()
}
