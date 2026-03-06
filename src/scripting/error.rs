/// Errors from the scripting engine.
#[derive(Debug)]
pub enum ScriptError {
    /// mJS instance creation returned null.
    InitFailed,
    /// Script source exceeds the maximum allowed length.
    ScriptTooLarge(usize),
    /// mJS returned an error during script execution.
    Execution(String),
    /// Script exceeded the operation limit.
    OpLimitExceeded,
    /// Script exceeded the wall-clock time limit.
    TimeLimitExceeded(core::time::Duration),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InitFailed => write!(f, "mJS engine initialization failed"),
            Self::ScriptTooLarge(len) => {
                write!(f, "script too large: {} bytes (max {})", len, super::engine::MAX_SCRIPT_LEN)
            }
            Self::Execution(msg) => write!(f, "script error: {}", msg),
            Self::OpLimitExceeded => write!(f, "script exceeded operation limit"),
            Self::TimeLimitExceeded(elapsed) => {
                write!(f, "script exceeded time limit after {}ms", elapsed.as_millis())
            }
        }
    }
}

impl std::error::Error for ScriptError {}
