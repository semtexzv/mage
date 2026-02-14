#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Network Error: {0}")]
    Network(String), // We'll use String for now until we add request/ureq

    #[error("Extraction Error: {0}")]
    Extraction(String),

    #[error("Parse Error: {0}")]
    Parse(String),

    #[error("Compilation Error: {0}")]
    Compilation(String),

    #[error("Toolchain Resolution Error: {0}")]
    Toolchain(String),

    #[error("Bundle generation failed: {0}")]
    Bundle(String),

    #[error("Invalid template: {0}")]
    Template(String),

    #[error("JSON serialization/deserialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("UTF-8 parsing error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub type Result<T> = std::result::Result<T, Error>;
