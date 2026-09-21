#[derive(Debug, thiserror::Error)]
pub enum AnonError {
    #[error("config: {0}")]
    Config(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dicom: {0}")]
    Dicom(String),
    #[error("unsupported transfer syntax: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, AnonError>;
