//! 按本地项目保存最后一次完整扫描，仅作预览快照；上传始终重新解析源码。
use crate::openapi::Export;
use openidea_plugin::{Error, host};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const FORMAT: u32 = 1;
// 宿主侧栏消息上限 8 MiB，为配置和消息封装预留空间。
const MAX_BYTES: usize = 6 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    version: u32,
    pub project_root: String,
    pub scanned_at_ms: u64,
    pub export: Export,
}

fn key(root: &str) -> String {
    format!("scan-snapshot:{root}")
}

impl Snapshot {
    pub fn new(project_root: String, export: Export) -> Result<Self, Error> {
        let scanned_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::Failed(format!("无法读取扫描时间：{e}")))?
            .as_millis() as u64;
        Ok(Self {
            version: FORMAT,
            project_root,
            scanned_at_ms,
            export,
        })
    }

    pub fn save(&self) -> Result<(), Error> {
        let text = serde_json::to_string(self).map_err(|e| Error::Invalid(e.to_string()))?;
        if text.len() > MAX_BYTES {
            return Err("扫描快照超过 6 MiB，未覆盖上次保存的结果".into());
        }
        host::storage::set(&key(&self.project_root), &text)
    }
}

pub fn exists(root: &str) -> Result<bool, Error> {
    let Some(text) = host::storage::get(&key(root))? else {
        return Ok(false);
    };
    Ok(!text.is_empty() && text.len() <= MAX_BYTES)
}

pub fn load(root: &str) -> Result<Option<Snapshot>, Error> {
    let Some(text) = host::storage::get(&key(root))? else {
        return Ok(None);
    };
    if text.len() > MAX_BYTES {
        return Err("已保存的扫描快照超过读取上限，请重新扫描".into());
    }
    let snapshot: Snapshot = serde_json::from_str(&text)
        .map_err(|e| Error::Invalid(format!("扫描快照损坏，请重新扫描：{e}")))?;
    if snapshot.version != FORMAT
        || snapshot.project_root != root
        || snapshot.scanned_at_ms == 0
        || snapshot.scanned_at_ms > 8_640_000_000_000_000
        || snapshot.export.endpoints.is_empty()
        || !snapshot.export.document["paths"].is_object()
        || !snapshot.export.document["components"]["schemas"].is_object()
    {
        return Err("扫描快照版本、项目或内容不匹配，请重新扫描".into());
    }
    Ok(Some(snapshot))
}
