//! 兼容可安全解释的旧插件规则；脚本规则不在 WASM 中执行。
use crate::source::{Class, has};
pub fn apply(classes: &mut [Class], properties: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    for (line, raw) in properties.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with(['#', '!']) {
            continue;
        }
        let Some((key, value)) = raw.split_once('=') else {
            warnings.push(format!("旧配置第 {} 行无法解析", line + 1));
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if !["param.ignore", "field.ignore", "method.ignore"].contains(&key) {
            warnings.push(format!("旧配置规则 {key} 暂不支持，请核对导出结果"));
            continue;
        }
        let Some(annotation) = value
            .strip_prefix('@')
            .filter(|s| {
                !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$'))
            })
            .and_then(|s| s.rsplit('.').next())
        else {
            warnings.push(format!("旧配置规则 {key} 仅支持 @注解全名，未执行表达式"));
            continue;
        };
        for class in classes.iter_mut() {
            match key {
                "param.ignore" => {
                    for method in &mut class.methods {
                        method.params.retain(|p| !has(&p.annotations, annotation));
                    }
                }
                "field.ignore" => class.fields.retain(|f| !has(&f.annotations, annotation)),
                "method.ignore" => class.methods.retain(|m| !has(&m.annotations, annotation)),
                _ => (),
            }
        }
    }
    warnings
}
