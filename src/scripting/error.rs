/// Errors from the scripting engine.
#[derive(Debug)]
pub enum ScriptError {
    /// mJS instance creation returned null.
    InitFailed,
    /// Script source exceeds the maximum allowed length.
    ScriptTooLarge(usize),
    /// mJS returned an error during script execution.
    Execution(String),
    /// Cascading script depth limit exceeded.
    DepthLimitExceeded,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InitFailed => write!(f, "mJS engine initialization failed"),
            Self::ScriptTooLarge(len) => {
                write!(f, "script too large: {} bytes (max {})", len, super::engine::MAX_SCRIPT_LEN)
            }
            Self::Execution(msg) => write!(f, "script error: {}", msg),
            Self::DepthLimitExceeded => write!(f, "script depth limit exceeded"),
        }
    }
}

impl std::error::Error for ScriptError {}
