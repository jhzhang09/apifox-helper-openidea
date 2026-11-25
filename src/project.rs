//! 从项目基础配置解析路由，不读取环境变量，也不推测运行时激活的 profile。
use crate::source;
use std::collections::{BTreeMap, BTreeSet};
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Clone, Debug)]
pub struct ProjectContext {
    pub properties: BTreeMap<String, String>,
    pub conflicts: BTreeSet<String>,
    pub tree_keys: BTreeMap<String, String>,
}
impl Default for ProjectContext {
    fn default() -> Self {
        Self {
            properties: BTreeMap::new(),
            conflicts: BTreeSet::new(),
            tree_keys: ["id", "parentId", "name", "weight", "children"]
                .into_iter()
                .map(|s| (s.into(), s.into()))
                .collect(),
        }
    }
}
#[derive(Default)]
pub struct ProjectSettings {
    properties: BTreeMap<String, BTreeSet<String>>,
    tree_keys: BTreeMap<String, BTreeSet<String>>,
}
impl ProjectSettings {
    pub fn config(&mut self, path: &str, text: &str) -> Result<(), String> {
        if source::ignored_path(path) {
            return Ok(());
        }
        if path.ends_with(".properties") {
            for raw in text.lines() {
                let raw = raw.trim();
                if raw.is_empty() || raw.starts_with(['#', '!']) {
                    continue;
                }
                if let Some((key, value)) = raw.split_once('=').or_else(|| raw.split_once(':')) {
                    self.property(key.trim(), value.trim());
                }
            }
            return Ok(());
        }
        // Maven 的独立占位标量不是合法 YAML，先作为未解析字符串保留。
        let text = text
            .lines()
            .map(|line| {
                if !line.trim_start().starts_with('#')
                    && let Some((key, value)) = line.split_once(':')
                {
                    let trimmed = value.trim();
                    let scalar = trimmed
                        .split_once(" #")
                        .map(|(head, _)| head.trim_end())
                        .unwrap_or(trimmed);
                    if scalar.starts_with('@') && scalar.len() > 2 && scalar[1..].contains('@') {
                        return format!("{key}: {}", serde_json::json!(scalar));
                    }
                }
                line.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let docs = YamlLoader::load_from_str(&text)
            .map_err(|_| format!("无法读取基础配置 {path} 的 YAML 结构"))?;
        for doc in docs {
            if !doc["spring"]["config"]["activate"]["on-profile"].is_badvalue()
                || doc["spring"]["profiles"].as_str().is_some()
            {
                continue;
            }
            self.flatten("", &doc);
        }
        Ok(())
    }
    fn property(&mut self, key: &str, value: &str) {
        self.properties
            .entry(key.into())
            .or_default()
            .insert(value.into());
    }
    fn flatten(&mut self, prefix: &str, value: &Yaml) {
        match value {
            Yaml::Hash(map) => {
                for (key, value) in map {
                    if let Some(key) = key.as_str() {
                        let key = if prefix.is_empty() {
                            key.into()
                        } else {
                            format!("{prefix}.{key}")
                        };
                        self.flatten(&key, value);
                    }
                }
            }
            Yaml::String(s) | Yaml::Real(s) => self.property(prefix, s),
            Yaml::Integer(n) => self.property(prefix, &n.to_string()),
            Yaml::Boolean(b) => self.property(prefix, &b.to_string()),
            _ => (),
        }
    }
    /// 仅识别 Hutool 全局默认配置的字面量 setter，局部定制不冒充全局配置。
    pub fn java(&mut self, text: &str) {
        if !text.contains("cn.hutool.core.lang.tree") || !text.contains("DEFAULT_CONFIG") {
            return;
        }
        let tokens = source::lex(text);
        let t = |i: usize| tokens.get(i).map(|t| t.text.as_str()).unwrap_or("");
        for i in 0..tokens.len() {
            if t(i) != "TreeNodeConfig" || t(i + 1) != "." || t(i + 2) != "DEFAULT_CONFIG" {
                continue;
            }
            let mut n = i + 3;
            while t(n) == "." && t(n + 2) == "(" {
                let field = match t(n + 1) {
                    "setIdKey" => "id",
                    "setParentIdKey" => "parentId",
                    "setNameKey" => "name",
                    "setWeightKey" => "weight",
                    "setChildrenKey" => "children",
                    _ => break,
                };
                if t(n + 4) != ")" {
                    break;
                }
                if let Ok(value) = serde_json::from_str::<String>(t(n + 3))
                    && !value.trim().is_empty()
                {
                    self.tree_keys
                        .entry(field.into())
                        .or_default()
                        .insert(value);
                }
                n += 5;
            }
        }
    }
    pub fn finish(self) -> ProjectContext {
        let mut context = ProjectContext::default();
        for (key, values) in self.properties {
            if values.len() == 1 {
                context
                    .properties
                    .insert(key, values.into_iter().next().unwrap());
            } else {
                context.conflicts.insert(key);
            }
        }
        for (key, values) in self.tree_keys {
            if values.len() == 1 {
                context
                    .tree_keys
                    .insert(key, values.into_iter().next().unwrap());
            } else {
                context.tree_keys.remove(&key);
            }
        }
        context
    }
}

impl ProjectContext {
    pub fn debug_url(&self) -> String {
        let address = self
            .route("${server.address:localhost}")
            .unwrap_or_default();
        let host = if let Ok(ip) = address.parse::<std::net::Ipv6Addr>() {
            if ip.is_unspecified() {
                "localhost".into()
            } else {
                format!("[{ip}]")
            }
        } else if address == "0.0.0.0"
            || address.is_empty()
            || !address
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b".-".contains(&ch))
        {
            "localhost".into()
        } else {
            address
        };
        let port = self
            .route("${server.port:8080}")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|p| *p != 0)
            .unwrap_or(8080);
        format!("http://{host}:{port}")
    }
    pub fn route(&self, path: &str) -> Result<String, String> {
        self.resolve(path, &mut Vec::new())
    }
    fn resolve(&self, text: &str, stack: &mut Vec<String>) -> Result<String, String> {
        if stack.len() > 24 {
            return Err("路由占位符递归超过上限".into());
        }
        let mut result = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            result.push_str(&rest[..start]);
            let body = &rest[start + 2..];
            let mut depth = 1;
            let mut end = None;
            let mut colon = None;
            let chars: Vec<_> = body.char_indices().collect();
            for (n, (index, c)) in chars.iter().enumerate() {
                if *c == '$' && chars.get(n + 1).is_some_and(|(_, c)| *c == '{') {
                    depth += 1;
                }
                if *c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(*index);
                        break;
                    }
                }
                if *c == ':' && depth == 1 && colon.is_none() {
                    colon = Some(*index);
                }
            }
            let end = end.ok_or("路由占位符未闭合")?;
            let expression = &body[..end];
            let (key, default) = colon
                .map(|p| (&expression[..p], Some(&expression[p + 1..])))
                .unwrap_or((expression, None));
            if self.conflicts.contains(key) {
                return Err(format!("路由配置 {key} 在项目基础配置中存在冲突"));
            }
            if stack.iter().any(|s| s == key) {
                return Err(format!("路由配置 {key} 循环引用"));
            }
            let value = self
                .properties
                .get(key)
                .map(String::as_str)
                .or(default)
                .ok_or_else(|| format!("路由配置 {key} 未在项目基础配置中定义"))?;
            stack.push(key.into());
            let resolved = self.resolve(value, stack)?;
            stack.pop();
            result.push_str(&resolved);
            rest = &body[end + 1..];
        }
        result.push_str(rest);
        if result.contains(['$', '@']) || result.contains("#{") {
            return Err("路由包含未解析的动态配置".into());
        }
        Ok(result)
    }
}
