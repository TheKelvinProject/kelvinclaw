use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KelvinError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("backend failure: {0}")]
    Backend(String),
    #[error("io failure: {0}")]
    Io(String),
}

impl From<std::io::Error> for KelvinError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub type KelvinResult<T> = Result<T, KelvinError>;
