use std::{error::Error, fmt};

pub type AppResult<T> = Result<T, AppError>;

/// 状态/输入错误与具体服务错误分开，调用者可根据类型显示操作建议。
/// 保留底层 source 链，测试或诊断工具可以追踪原因，无需解析显示文本。
#[derive(Debug)]
pub enum AppError {
    InvalidInput(String),
    InvalidState(&'static str),
    Busy,
    WorkerStopped,
    Service {
        operation: &'static str,
        source: Box<dyn Error + Send + Sync>,
    },
}

impl AppError {
    pub(crate) fn service(
        operation: &'static str,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::Service {
            operation,
            source: Box::new(source),
        }
    }
}
impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "输入无效：{message}"),
            Self::InvalidState(message) => f.write_str(message),
            Self::Busy => f.write_str("后台任务正在运行，请等待完成"),
            Self::WorkerStopped => f.write_str("后台任务已停止，请重新启动应用"),
            Self::Service { operation, source } => write!(f, "{operation}失败：{source}"),
        }
    }
}
impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Service { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
