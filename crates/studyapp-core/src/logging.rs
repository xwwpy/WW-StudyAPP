//! Persistent logging choices, independent of notification delivery and filesystem I/O.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    pub const ALL: [Self; 5] = [
        Self::Error,
        Self::Warn,
        Self::Info,
        Self::Debug,
        Self::Trace,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warn => "Warn",
            Self::Info => "Info",
            Self::Debug => "Debug",
            Self::Trace => "Trace",
        }
    }
    pub fn allows(self, level: Self) -> bool {
        level <= self
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingSettings {
    pub enabled: bool,
    pub minimum_level: LogLevel,
    pub directory: Option<String>,
}
impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            minimum_level: LogLevel::Info,
            directory: None,
        }
    }
}
impl LoggingSettings {
    pub fn records(&self, level: LogLevel) -> bool {
        self.enabled && self.minimum_level.allows(level)
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(path) = &self.directory {
            let bytes = path.as_bytes();
            let windows = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], 47 | 92);
            // A portable configuration may retain another OS's absolute path.
            // The writer must separately validate the current host and availability.
            let absolute = path.starts_with('/') || windows || bytes.starts_with(&[92, 92]);
            ensure!(
                !path.is_empty()
                    && path.len() <= 4096
                    && path.trim() == path
                    && absolute
                    && !path.chars().any(char::is_control)
                    && !path.split(['/', char::from(92)]).any(|part| part == "..")
                    && !bytes.starts_with(&[92, 92, 46, 92]),
                "日志位置须为不含上级引用、控制字符或设备命名空间的绝对目录路径（最多4096字节）"
            );
        }
        Ok(())
    }
}
