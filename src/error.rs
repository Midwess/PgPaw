use serde_json::json;

#[derive(thiserror::Error, Debug)]
pub enum CacheError {
    #[error(transparent)]
    Pglite(#[from] pglite::Error),
    #[cfg(feature = "server")]
    #[error("upstream: {0}")]
    Upstream(#[from] tokio_postgres::Error),
    #[error("parse: {0}")]
    Parse(String),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("{0}")]
    Config(String),
    #[error("cache: {0}")]
    Cache(String),
    #[error("replica halted: {0}")]
    Halted(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl CacheError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pglite(_) => "PgliteError",
            #[cfg(feature = "server")]
            Self::Upstream(_) => "UpstreamError",
            Self::Parse(_) => "ParseError",
            Self::Rejected(_) => "RejectedError",
            Self::Config(_) => "ConfigError",
            Self::Cache(_) => "CacheError",
            Self::Halted(_) => "HaltedError",
            Self::Unauthorized(_) => "UnauthorizedError",
            Self::Forbidden(_) => "ForbiddenError",
            Self::Io(_) => "IoError",
        }
    }

    pub fn envelope(&self) -> String {
        json!({
            "name": self.name(),
            "message": self.to_string(),
        })
        .to_string()
    }
}
