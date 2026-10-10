use serde::Serializer;

/// 面向用户的错误：Display 输出中文句子，序列化后前端可直接展示。
#[derive(Debug)]
pub enum AppError {
    Io(String),
    Http(String),
    Parse(String),
    Args(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Io(m) => write!(f, "读取本地文件失败：{m}"),
            AppError::Http(m) => write!(f, "网络请求失败：{m}"),
            AppError::Parse(m) => write!(f, "数据解析失败：{m}"),
            AppError::Args(m) => write!(f, "参数不正确：{m}"),
        }
    }
}

impl serde::Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Parse(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
