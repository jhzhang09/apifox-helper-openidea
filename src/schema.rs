//! 以完整类型签名注册模型，泛型实例各自生成组件，循环引用保留为 $ref。
use crate::source::{self, Class, Field, annotation, has};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Type {
    pub name: String,
    pub args: Vec<Type>,
    pub array: u32,
    pub nullable: bool,
}
impl Type {
    pub fn parse(s: &str) -> Self {
        let s = s
            .trim()
            .trim_start_matches("?extends")
            .trim_start_matches("?super")
            .trim_start_matches("out ")
            .trim_start_matches("in ");
        let nullable = s.ends_with('?');
        let s = s.trim_end_matches('?');
        let mut s = s;
        let mut array = 0;
        while let Some(base) = s.strip_suffix("[]").or_else(|| s.strip_suffix("...")) {
            array += 1;
            s = base;
        }
        let (name, args) = if let Some(p) = s.find('<') {
            let inner = s[p + 1..].strip_suffix('>').unwrap_or(&s[p + 1..]);
            let mut depth = 0;
            let mut start = 0;
            let mut args = Vec::new();
            for (i, c) in inner.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => depth -= 1,
                    ',' if depth == 0 => {
                        args.push(Self::parse(&inner[start..i]));
                        start = i + 1;
                    }
                    _ => (),
                }
            }
            args.push(Self::parse(&inner[start..]));
            (s[..p].to_string(), args)
        } else {
            (s.to_string(), vec![])
        };
        Self {
            name,
            args,
            array,
            nullable,
        }
    }
    pub fn simple(&self) -> &str {
        self.name.rsplit('.').next().unwrap_or(&self.name)
    }
    pub fn signature(&self) -> String {
        format!(
            "{}{}{}",
            self.name,
            if self.args.is_empty() {
                String::new()
            } else {
                format!(
                    "<{}>",
                    self.args
                        .iter()
                        .map(Self::signature)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            },
            "[]".repeat(self.array as usize)
        )
    }
    pub fn substitute(&self, bindings: &BTreeMap<String, Type>) -> Self {
        if let Some(t) = bindings.get(&self.name) {
            let mut t = t.clone();
            t.array += self.array;
            t.nullable |= self.nullable;
            return t;
        }
        Self {
            args: self.args.iter().map(|a| a.substitute(bindings)).collect(),
            ..self.clone()
        }
    }
}

pub struct Schemas<'a> {
    pub classes: &'a [Class],
    pub components: BTreeMap<String, Value>,
    pub warnings: BTreeSet<String>,
    pub notices: BTreeSet<String>,
    active: BTreeSet<String>,
    active_stack: Vec<String>,
    pub component_warnings: BTreeMap<String, BTreeSet<String>>,
    pub component_notices: BTreeMap<String, BTreeSet<String>>,
    pub op_warnings: BTreeSet<String>,
    pub op_notices: BTreeSet<String>,
    pub op_touched: BTreeSet<String>,
    types: BTreeMap<String, Type>,
    project: crate::project::ProjectContext,
}
impl<'a> Schemas<'a> {
    pub fn new(classes: &'a [Class]) -> Self {
        Self::with_project(classes, &crate::project::ProjectContext::default())
    }
    pub fn with_project(classes: &'a [Class], project: &crate::project::ProjectContext) -> Self {
        Self {
            classes,
            components: BTreeMap::new(),
            warnings: BTreeSet::new(),
            notices: BTreeSet::new(),
            active: BTreeSet::new(),
            active_stack: Vec::new(),
            component_warnings: BTreeMap::new(),
            component_notices: BTreeMap::new(),
            op_warnings: BTreeSet::new(),
            op_notices: BTreeSet::new(),
            op_touched: BTreeSet::new(),
            types: BTreeMap::new(),
            project: project.clone(),
        }
    }
    pub fn begin_operation(&mut self) {
        self.op_warnings.clear();
        self.op_notices.clear();
        self.op_touched.clear();
    }
    pub fn warn(&mut self, msg: String) {
        self.warnings.insert(msg.clone());
        if let Some(key) = self.active_stack.last() {
            self.component_warnings
                .entry(key.clone())
                .or_default()
                .insert(msg);
        } else {
            self.op_warnings.insert(msg);
        }
    }
    pub fn notice(&mut self, msg: String) {
        self.notices.insert(msg.clone());
        if let Some(key) = self.active_stack.last() {
            self.component_notices
                .entry(key.clone())
                .or_default()
                .insert(msg);
        } else {
            self.op_notices.insert(msg);
        }
    }
    pub fn resolve(&mut self, name: &str, ctx: &Class) -> Option<&'a Class> {
        if name.contains('.')
            && let Some(c) = self.classes.iter().find(|c| c.qualified() == name)
        {
            return Some(c);
        }
        let mut enclosing = ctx.name.as_str();
        loop {
            let candidate = format!("{}.{}.{name}", ctx.package, enclosing)
                .trim_start_matches('.')
                .to_string();
            if let Some(c) = self.classes.iter().find(|c| c.qualified() == candidate) {
                return Some(c);
            }
            match enclosing.rsplit_once('.') {
                Some((parent, _)) => enclosing = parent,
                None => break,
            }
        }
        // 显式导入先绑定名称；即使源码不可用，也不能改绑到通配或同包类型。
        let explicit: BTreeSet<_> = ctx
            .imports
            .iter()
            .filter(|i| !i.ends_with(".*") && i.rsplit('.').next() == Some(name))
            .cloned()
            .collect();
        if !explicit.is_empty() {
            return self.bind_candidates(name, ctx, explicit);
        }
        if let Some(c) = self
            .classes
            .iter()
            .find(|c| c.qualified() == format!("{}.{name}", ctx.package).trim_start_matches('.'))
        {
            return Some(c);
        }
        let external = Self::external_name(name, ctx);
        let wildcard: BTreeSet<_> = ctx
            .imports
            .iter()
            .filter_map(|i| i.strip_suffix(".*"))
            .map(|p| format!("{p}.{name}"))
            .filter(|wanted| {
                self.classes.iter().any(|c| c.qualified() == *wanted)
                    || (external != name && external == *wanted)
            })
            .collect();
        if !wildcard.is_empty() {
            return self.bind_candidates(name, ctx, wildcard);
        }
        // 已知外部通配 import 不能被项目其他包中的同名类抢占。
        if Self::external_name(name, ctx) != name {
            return None;
        }
        let candidates = self
            .classes
            .iter()
            .filter(|c| c.name == name)
            .map(Class::qualified)
            .collect();
        self.bind_candidates(name, ctx, candidates)
    }
    fn bind_candidates(
        &mut self,
        name: &str,
        ctx: &Class,
        candidates: BTreeSet<String>,
    ) -> Option<&'a Class> {
        if candidates.len() > 1 {
            self.warn(format!(
                "类型 {name} 存在歧义（{}）：{}，未选择任一模型",
                ctx.path,
                candidates.into_iter().collect::<Vec<_>>().join("、")
            ));
            return None;
        }
        let wanted = candidates.first()?;
        self.classes.iter().find(|c| c.qualified() == *wanted)
    }
    fn canonical(&mut self, ty: &Type, ctx: &Class) -> Type {
        Type {
            name: self
                .resolve(&ty.name, ctx)
                .map(Class::qualified)
                .unwrap_or_else(|| Self::external_name(&ty.name, ctx)),
            args: ty.args.iter().map(|t| self.canonical(t, ctx)).collect(),
            ..ty.clone()
        }
    }
    pub(crate) fn external_name(name: &str, ctx: &Class) -> String {
        if name.contains('.') {
            return name.into();
        }
        if let Some(import) = ctx
            .imports
            .iter()
            .find(|i| i.rsplit('.').next() == Some(name))
        {
            return import.clone();
        }
        for full in [
            "java.util.Properties",
            "cn.hutool.core.lang.tree.Tree",
            "org.springframework.web.servlet.mvc.method.annotation.SseEmitter",
        ] {
            if full.rsplit('.').next() == Some(name)
                && ctx
                    .imports
                    .iter()
                    .any(|i| i == &format!("{}.*", full.rsplit_once('.').unwrap().0))
            {
                return full.into();
            }
        }
        name.into()
    }
    pub fn schema(&mut self, ty: &Type, ctx: &Class) -> Value {
        let mut value = self.inner(ty, ctx);
        if ty.nullable {
            if value.get("$ref").is_some() {
                value = json!({"allOf":[value],"nullable":true});
            } else {
                value["nullable"] = json!(true);
            }
        }
        value
    }
    fn inner(&mut self, ty: &Type, ctx: &Class) -> Value {
        if self.active.len() > 48 || self.components.len() > 2000 {
            self.warn("模型展开达到上限，停止递归".into());
            return json!({"type":"object","x-apifox-unresolved":ty.signature()});
        }
        if ty.array > 0 {
            let mut item = ty.clone();
            item.array -= 1;
            return json!({"type":"array","items":self.schema(&item,ctx)});
        }
        let simple = ty.simple();
        let first = ty
            .args
            .first()
            .cloned()
            .unwrap_or_else(|| Type::parse("Object"));
        let external = self
            .resolve(&ty.name, ctx)
            .map(Class::qualified)
            .unwrap_or_else(|| Self::external_name(&ty.name, ctx));
        if external == "java.util.Properties" {
            return json!({"type":"object","additionalProperties":{},"description":"Properties 动态键值对象"});
        }
        if external == "cn.hutool.core.lang.tree.Tree" {
            return self.hutool_tree(ty, ctx);
        }
        if external == "org.springframework.web.servlet.mvc.method.annotation.SseEmitter" {
            return json!({"type":"string","description":"SSE 事件流"});
        }
        match simple {
            "ResponseEntity" | "HttpEntity" | "Optional" | "Mono" | "CompletionStage"
            | "CompletableFuture" | "DeferredResult" | "Callable" => {
                return self.schema(&first, ctx);
            }
            "List" | "Set" | "Collection" | "Iterable" | "ArrayList" | "LinkedList" | "Array"
            | "Flux" | "Sequence" | "MutableList" | "MutableSet" => {
                return json!({"type":"array","items":self.schema(&first,ctx)});
            }
            "Map" | "HashMap" | "LinkedHashMap" | "SortedMap" | "MutableMap" => {
                return json!({"type":"object","additionalProperties":self.schema(ty.args.get(1).unwrap_or(&Type::parse("Object")),ctx)});
            }
            "String" | "char" | "Character" | "Char" | "CharSequence" => {
                return json!({"type":"string"});
            }
            "int" | "Integer" | "Int" | "short" | "Short" | "byte" | "Byte" => {
                return json!({"type":"integer","format":"int32"});
            }
            "long" | "Long" | "BigInteger" => return json!({"type":"integer","format":"int64"}),
            "float" | "Float" => return json!({"type":"number","format":"float"}),
            "double" | "Double" | "BigDecimal" | "Number" => {
                return json!({"type":"number","format":"double"});
            }
            "boolean" | "Boolean" => return json!({"type":"boolean"}),
            "LocalDate" => return json!({"type":"string","format":"date"}),
            "Date" | "Instant" | "LocalDateTime" | "OffsetDateTime" | "ZonedDateTime"
            | "Timestamp" => return json!({"type":"string","format":"date-time"}),
            "UUID" => return json!({"type":"string","format":"uuid"}),
            "LocalTime" | "OffsetTime" => return json!({"type":"string"}),
            "MultipartFile" | "File" | "InputStream" => {
                return json!({"type":"string","format":"binary"});
            }
            "Object" | "Any" | "?" => return json!({}),
            "void" | "Void" | "Unit" | "" => {
                return json!({"type":"object"});
            }
            _ => (),
        }
        let Some(class) = self.resolve(&ty.name, ctx) else {
            self.warn(format!(
                "无法解析类型 {}（{}），未推测字段",
                ty.signature(),
                ctx.path
            ));
            return json!({"type":"object","description":format!("未解析类型：{}",ty.signature()),"x-apifox-unresolved":ty.signature()});
        };
        let resolved = self.canonical(ty, ctx);
        let signature = resolved.signature();
        // 完整签名只作内部标识；导出时统一映射为可读模型名。
        let key = component_key(&signature);
        self.types.insert(key.clone(), resolved.clone());
        self.op_touched.insert(key.clone());
        if !self.components.contains_key(&key) && !self.active.contains(&key) {
            self.active.insert(key.clone());
            self.active_stack.push(key.clone());
            let bindings: BTreeMap<_, _> = class
                .generics
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let actual = resolved.args.get(i).cloned().unwrap_or_else(|| {
                        self.notice(format!(
                            "{} 未指定泛型参数 {name}，对应字段保留为开放结构（{}）",
                            class.name, class.path
                        ));
                        Type::parse("Object")
                    });
                    (name.clone(), actual)
                })
                .collect();
            let shape = if !class.variants.is_empty() {
                self.enum_schema(class)
            } else {
                let mut properties = serde_json::Map::new();
                let mut required = Vec::new();
                let mut additional = None;
                self.fields(
                    class,
                    &bindings,
                    &mut properties,
                    &mut required,
                    &mut additional,
                    &mut BTreeSet::new(),
                );
                let mut v = json!({"type":"object","properties":properties});
                v["x-apifox-orders"] = json!(
                    v["properties"]
                        .as_object()
                        .map(|p| p.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                );
                if let Some(schema) = additional {
                    v["additionalProperties"] = schema;
                }
                if !required.is_empty() {
                    v["required"] = json!(required);
                }
                v
            };
            let mut shape = shape;
            shape["description"] = json!(crate::documentation::model_description(class));
            if let Some(title) = crate::documentation::model_title(class) {
                shape["title"] = json!(title);
            }
            self.components.insert(key.clone(), shape);
            self.active_stack.pop();
            self.active.remove(&key);
        }
        json!({"$ref":format!("#/components/schemas/{key}")})
    }

    fn enum_schema(&self, class: &Class) -> Value {
        if class.enum_items.is_empty() {
            return json!({"type":"string","enum":class.variants});
        }
        let json_val_target = class
            .fields
            .iter()
            .enumerate()
            .find(|(_, f)| {
                annotation(&f.annotations, "JsonValue")
                    .is_some_and(|a| a.value("value").as_deref() != Some("false"))
            })
            .map(|(idx, f)| (idx, Type::parse(&f.ty)))
            .or_else(|| {
                class
                    .methods
                    .iter()
                    .find(|m| {
                        annotation(&m.annotations, "JsonValue")
                            .is_some_and(|a| a.value("value").as_deref() != Some("false"))
                    })
                    .map(|m| {
                        let stem = m
                            .name
                            .strip_prefix("get")
                            .or_else(|| m.name.strip_prefix("is"))
                            .unwrap_or(&m.name);
                        let lower = stem
                            .chars()
                            .next()
                            .map(|c| format!("{}{}", c.to_lowercase(), &stem[c.len_utf8()..]))
                            .unwrap_or_default();
                        let idx = class
                            .fields
                            .iter()
                            .position(|f| f.name == lower || f.name == stem)
                            .unwrap_or(0);
                        (idx, Type::parse(&m.ty))
                    })
            });

        let mut used_val_idx = None;
        let mut out_type = json!("string");
        let mut out_format: Option<&'static str> = None;
        let mut values = Vec::new();

        if let Some((idx, ty)) = json_val_target {
            let primitive = match ty.simple() {
                "int" | "Integer" | "Int" | "short" | "Short" | "byte" | "Byte" => {
                    Some(("integer", Some("int32")))
                }
                "long" | "Long" | "BigInteger" => Some(("integer", Some("int64"))),
                "float" | "Float" => Some(("number", Some("float"))),
                "double" | "Double" | "BigDecimal" | "Number" => Some(("number", Some("double"))),
                "boolean" | "Boolean" => Some(("boolean", None)),
                "String" | "char" | "Character" | "Char" | "CharSequence" => Some(("string", None)),
                _ => None,
            };
            if let Some((tname, fmt)) = primitive {
                let mut extracted = Vec::with_capacity(class.enum_items.len());
                let mut ok = true;
                for item in &class.enum_items {
                    if item.arg_literals.get(idx).copied() != Some(true) {
                        ok = false;
                        break;
                    }
                    let is_str = item.arg_strings.get(idx).copied().unwrap_or(false);
                    let raw = &item.args[idx];
                    let parsed = match tname {
                        "integer" if !is_str => raw.parse::<i64>().ok().map(|v| json!(v)),
                        "number" if !is_str => raw.parse::<f64>().ok().map(|v| json!(v)),
                        "boolean" if !is_str => raw.parse::<bool>().ok().map(|v| json!(v)),
                        "string" if is_str => Some(json!(raw)),
                        _ => None,
                    };
                    if let Some(v) = parsed {
                        extracted.push(v);
                    } else {
                        ok = false;
                        break;
                    }
                }
                if ok && !extracted.is_empty() {
                    used_val_idx = Some(idx);
                    out_type = json!(tname);
                    out_format = fmt;
                    values = extracted;
                }
            }
        }

        if used_val_idx.is_none() {
            values = class
                .enum_items
                .iter()
                .map(|item| {
                    let custom = [("JsonProperty", "value"), ("SerializedName", "value")]
                        .into_iter()
                        .find_map(|(a, k)| {
                            annotation(&item.annotations, a).and_then(|a| a.text(k))
                        });
                    json!(custom.unwrap_or_else(|| item.name.clone()))
                })
                .collect();
        }

        let descriptions: Vec<Option<String>> = class
            .enum_items
            .iter()
            .map(|item| {
                crate::documentation::text(
                    &item.annotations,
                    &[("Schema", "description"), ("ApiModelProperty", "value")],
                )
                .or_else(|| source::nonempty(&source::summary(&item.doc)))
                .or_else(|| {
                    item.args
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| Some(*i) != used_val_idx)
                        .filter(|(i, s)| {
                            item.arg_strings.get(*i).copied().unwrap_or(false)
                                && !s.trim().is_empty()
                        })
                        .map(|(_, s)| s.trim().to_string())
                        .next_back()
                })
            })
            .collect();

        let mut obj = json!({"type": out_type, "enum": values});
        if let Some(fmt) = out_format {
            obj["format"] = json!(fmt);
        }
        let has_meta = used_val_idx.is_some()
            || descriptions.iter().any(Option::is_some)
            || values
                .iter()
                .zip(&class.enum_items)
                .any(|(v, item)| v.as_str() != Some(item.name.as_str()));
        if has_meta {
            let entries: Vec<Value> = class
                .enum_items
                .iter()
                .zip(values.iter())
                .zip(descriptions.iter())
                .map(|((item, val), desc)| {
                    json!({
                        "value": val,
                        "name": item.name,
                        "description": desc.as_deref().unwrap_or("")
                    })
                })
                .collect();
            obj["x-apifox-enum"] = json!(entries);
        }
        obj
    }

    fn hutool_tree(&mut self, ty: &Type, ctx: &Class) -> Value {
        let mut resolved = self.canonical(ty, ctx);
        resolved.name = "cn.hutool.core.lang.tree.Tree".into();
        let key = component_key(&resolved.signature());
        self.types.insert(key.clone(), resolved.clone());
        self.op_touched.insert(key.clone());
        if !self.components.contains_key(&key) && !self.active.contains(&key) {
            self.active.insert(key.clone());
            self.active_stack.push(key.clone());
            let id = resolved
                .args
                .first()
                .cloned()
                .unwrap_or_else(|| Type::parse("Object"));
            let mut fields = serde_json::Map::new();
            for field in ["id", "parentId", "name", "weight", "children"] {
                if let Some(name) = self.project.tree_keys.get(field).cloned() {
                    let shape = match field {
                        "id" | "parentId" => self.schema(&id, ctx),
                        "name" => json!({"type":"string"}),
                        "children" => {
                            json!({"type":"array","items":{"$ref":format!("#/components/schemas/{key}")}})
                        }
                        _ => json!({}),
                    };
                    fields.insert(name, shape);
                }
            }
            let order: Vec<_> = fields.keys().cloned().collect();
            self.components.insert(key.clone(),json!({"type":"object","properties":fields,"additionalProperties":{},"x-apifox-orders":order,"description":"Hutool Tree 节点，字段名按库默认值和项目全局默认配置解析，动态扩展字段保持开放"}));
            self.notice("Hutool Tree 按 5.8.x 默认结构及项目全局字面量配置生成；局部定制字段和动态扩展字段需核对".into());
            self.active_stack.pop();
            self.active.remove(&key);
        }
        json!({"$ref":format!("#/components/schemas/{key}")})
    }
    fn fields(
        &mut self,
        class: &Class,
        bindings: &BTreeMap<String, Type>,
        props: &mut serde_json::Map<String, Value>,
        required: &mut Vec<String>,
        additional: &mut Option<Value>,
        seen: &mut BTreeSet<String>,
    ) {
        if !seen.insert(class.qualified()) {
            return;
        }
        for base in &class.bases {
            let ty = Type::parse(base).substitute(bindings);
            if matches!(
                ty.simple(),
                "Serializable"
                    | "Comparable"
                    | "Cloneable"
                    | "AutoCloseable"
                    | "Object"
                    | "Any"
                    | "Enum"
            ) {
                continue;
            }
            if matches!(
                ty.simple(),
                "Map"
                    | "HashMap"
                    | "LinkedHashMap"
                    | "TreeMap"
                    | "SortedMap"
                    | "AbstractMap"
                    | "ConcurrentHashMap"
            ) {
                let value = ty
                    .args
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| Type::parse("Object"));
                *additional = Some(self.schema(&value, class));
                self.notice(format!("{} 继承 Map，动态字段按 additionalProperties 保留，声明中未确定的键不会推测（{}）", class.name, class.path));
            } else if let Some(parent) = self.resolve(&ty.name, class) {
                let canonical = self.canonical(&ty, class);
                let b = parent
                    .generics
                    .iter()
                    .cloned()
                    .zip(canonical.args)
                    .collect();
                self.fields(parent, &b, props, required, additional, seen);
            } else {
                self.warn(format!("无法解析父类型 {base}（{}）", class.path));
            }
        }
        let ignored_props: BTreeSet<String> =
            annotation(&class.annotations, "JsonIgnoreProperties")
                .map(|a| a.values("value").into_iter().collect())
                .unwrap_or_default();
        let naming_strategy = annotation(&class.annotations, "JsonNaming")
            .and_then(|a| a.value("value"))
            .unwrap_or_default();
        for f in &class.fields {
            if annotation(&f.annotations, "JsonIgnore")
                .is_some_and(|a| a.value("value").as_deref() != Some("false"))
                || annotation(&f.annotations, "JSONField")
                    .and_then(|a| a.value("serialize"))
                    .as_deref()
                    == Some("false")
                || annotation(&f.annotations, "Schema")
                    .and_then(|a| a.value("hidden"))
                    .as_deref()
                    == Some("true")
            {
                continue;
            }
            let explicit_name = [
                ("JsonProperty", "value"),
                ("JSONField", "name"),
                ("SerializedName", "value"),
                ("Schema", "name"),
            ]
            .into_iter()
            .find_map(|(a, k)| annotation(&f.annotations, a).and_then(|a| a.value(k)))
            .filter(|n| !n.is_empty());
            let name = explicit_name.unwrap_or_else(|| apply_naming(&f.name, &naming_strategy));
            if ignored_props.contains(&f.name) || ignored_props.contains(&name) {
                continue;
            }
            let ty = Type::parse(&f.ty).substitute(bindings);
            let base = self.schema(&ty, class);
            let mut schema = if base.get("$ref").is_some() {
                json!({"allOf":[base]})
            } else {
                base
            };
            decorate(&mut schema, f);
            if ["NotNull", "NotBlank", "NotEmpty"]
                .iter()
                .any(|a| has(&f.annotations, a))
                || annotation(&f.annotations, "Schema").is_some_and(|a| {
                    a.value("required").as_deref() == Some("true")
                        || a.value("requiredMode").as_deref()
                            == Some("Schema.RequiredMode.REQUIRED")
                })
            {
                required.push(name.clone());
            }
            props.insert(name, schema);
        }
        seen.remove(&class.qualified());
    }

    /// 导出名使用简单类名和泛型参数；冲突类型仍保持独立，不能覆盖同名 DTO。
    pub fn export_names(&self) -> BTreeMap<String, String> {
        let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for class in self.classes {
            declared
                .entry(model_name(&Type::parse(&class.qualified())))
                .or_default()
                .insert(class.qualified());
        }
        let mut counts = BTreeMap::<String, usize>::new();
        for ty in self.types.values() {
            *counts.entry(model_name(ty)).or_default() += 1;
        }
        let mut used: BTreeSet<String> = counts.keys().chain(declared.keys()).cloned().collect();
        fn ambiguous(ty: &Type, declared: &BTreeMap<String, BTreeSet<String>>) -> bool {
            declared
                .get(&model_name(ty))
                .is_some_and(|types| types.iter().any(|name| name != &ty.signature()))
                || ty.args.iter().any(|arg| ambiguous(arg, declared))
        }
        self.types
            .iter()
            .map(|(key, ty)| {
                let base = model_name(ty);
                let own_class = declared
                    .get(&base)
                    .is_some_and(|types| types.len() == 1 && types.contains(&ty.signature()));
                let collision = (counts[&base] > 1 && !own_class) || ambiguous(ty, &declared);
                let name = if collision {
                    let mut candidate = format!("{base}_{:016x}", signature_hash(&ty.signature()));
                    while used.contains(&candidate) {
                        candidate.push('_');
                    }
                    used.insert(candidate.clone());
                    candidate
                } else {
                    base
                };
                (key.clone(), name)
            })
            .collect()
    }
}

/// 泛型名称与官方插件示例保持相同的可读形式，例如 R<List<User>> → RListUser。
pub fn model_name(ty: &Type) -> String {
    let base: String = ty
        .simple()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    format!(
        "{}{}{}",
        if base.is_empty() { "Model" } else { &base },
        ty.args.iter().map(model_name).collect::<String>(),
        "Array".repeat(ty.array as usize)
    )
}
fn signature_hash(signature: &str) -> u64 {
    // 固定算法使冲突名不依赖扫描顺序或 Rust 随机哈希种子。
    signature.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

/// 替换结构中的组件引用；普通描述、示例或业务数据里的同名文本保持原样。
pub fn rename_refs(value: &mut Value, names: &BTreeMap<String, String>) {
    match value {
        Value::Object(map) => {
            if let Some(key) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix("#/components/schemas/"))
                && let Some(name) = names.get(key)
            {
                map.insert("$ref".into(), json!(format!("#/components/schemas/{name}")));
            }
            for child in map.values_mut() {
                rename_refs(child, names);
            }
        }
        Value::Array(items) => {
            for child in items {
                rename_refs(child, names);
            }
        }
        _ => (),
    }
}
pub fn component_key(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_string()
            } else {
                format!("_{:x}_", c as u32)
            }
        })
        .collect()
}
pub fn decorate(schema: &mut Value, f: &Field) {
    if let Some(desc) = crate::documentation::field_description(f) {
        schema["description"] = json!(desc);
    }
    if let Some(title) = source::annotation_text(&f.annotations, &[("Schema", "title")]) {
        schema["title"] = json!(title);
    }
    for (a, key, dest) in [
        ("Schema", "example", "example"),
        ("Schema", "defaultValue", "default"),
        ("Schema", "format", "format"),
        ("Pattern", "regexp", "pattern"),
        ("Min", "value", "minimum"),
        ("Max", "value", "maximum"),
        ("DecimalMin", "value", "minimum"),
        ("DecimalMax", "value", "maximum"),
    ] {
        if let Some(v) = annotation(&f.annotations, a).and_then(|a| a.value(key)) {
            schema[dest] = if matches!(dest, "minimum" | "maximum") {
                v.parse::<f64>().map(|n| json!(n)).unwrap_or(json!(v))
            } else if matches!(dest, "default" | "example") {
                typed_value(&v, schema)
            } else {
                json!(v)
            };
        }
    }
    if let Some(a) = annotation(&f.annotations, "Size") {
        let array = schema["type"] == "array";
        for (key, target) in [
            ("min", if array { "minItems" } else { "minLength" }),
            ("max", if array { "maxItems" } else { "maxLength" }),
        ] {
            if let Some(v) = a.value(key).and_then(|v| v.parse::<u64>().ok()) {
                schema[target] = json!(v);
            }
        }
    }
    if let Some(a) = annotation(&f.annotations, "Schema") {
        let vals = a.values("allowableValues");
        if !vals.is_empty() {
            schema["enum"] = json!(vals);
        }
        if a.value("accessMode")
            .is_some_and(|v| v.ends_with("READ_ONLY"))
        {
            schema["readOnly"] = json!(true);
        }
        if a.value("accessMode")
            .is_some_and(|v| v.ends_with("WRITE_ONLY"))
        {
            schema["writeOnly"] = json!(true);
        }
    }
}
pub fn typed_value(s: &str, schema: &Value) -> Value {
    match schema["type"].as_str() {
        Some("integer") => s.parse::<i64>().map(|v| json!(v)).unwrap_or(json!(s)),
        Some("number") => s.parse::<f64>().map(|v| json!(v)).unwrap_or(json!(s)),
        Some("boolean") => s.parse::<bool>().map(|v| json!(v)).unwrap_or(json!(s)),
        _ => json!(s),
    }
}

fn apply_naming(name: &str, strategy: &str) -> String {
    if strategy.contains("SnakeCaseStrategy") {
        to_delimited_case(name, '_')
    } else if strategy.contains("KebabCaseStrategy") {
        to_delimited_case(name, '-')
    } else if strategy.contains("LowerCaseStrategy") {
        name.to_lowercase()
    } else if strategy.contains("UpperCamelCaseStrategy") {
        let mut chars = name.chars();
        match chars.next() {
            Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
            None => String::new(),
        }
    } else {
        name.to_string()
    }
}

fn to_delimited_case(name: &str, sep: char) -> String {
    let mut out = String::new();
    let mut prev_sep = true;
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            if !prev_sep && !out.is_empty() {
                out.push(sep);
                prev_sep = true;
            }
            continue;
        }
        if c.is_uppercase() {
            let prev_lower =
                i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if !out.is_empty() && !prev_sep && (prev_lower || next_lower) {
                out.push(sep);
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_sep = false;
        } else {
            out.push(c);
            prev_sep = false;
        }
    }
    out
}
