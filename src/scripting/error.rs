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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::engine::MAX_SCRIPT_LEN;
    use core::time::Duration;

    #[test]
    fn display_init_failed() {
        let e = ScriptError::InitFailed;
        assert_eq!(e.to_string(), "mJS engine initialization failed");
    }

    #[test]
    fn display_script_too_large() {
        let e = ScriptError::ScriptTooLarge(8192);
        assert_eq!(
            e.to_string(),
            format!("script too large: 8192 bytes (max {})", MAX_SCRIPT_LEN)
        );
    }

    #[test]
    fn display_execution_error() {
        let e = ScriptError::Execution("ReferenceError: x is not defined".into());
        assert_eq!(e.to_string(), "script error: ReferenceError: x is not defined");
    }

    #[test]
    fn display_execution_empty_message() {
        let e = ScriptError::Execution(String::new());
        assert_eq!(e.to_string(), "script error: ");
    }

    #[test]
    fn display_op_limit_exceeded() {
        let e = ScriptError::OpLimitExceeded;
        assert_eq!(e.to_string(), "script exceeded operation limit");
    }

    #[test]
    fn display_time_limit_exceeded() {
        let e = ScriptError::TimeLimitExceeded(Duration::from_millis(5000));
        assert_eq!(e.to_string(), "script exceeded time limit after 5000ms");
    }

    #[test]
    fn display_time_limit_sub_millisecond() {
        let e = ScriptError::TimeLimitExceeded(Duration::from_micros(500));
        assert_eq!(e.to_string(), "script exceeded time limit after 0ms");
    }

    #[test]
    fn implements_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(ScriptError::InitFailed);
        // source() returns None (no chained cause)
        assert!(e.source().is_none());
    }

    #[test]
    fn debug_impl_exists() {
        // Verify Debug derive works for all variants
        let variants: Vec<ScriptError> = vec![
            ScriptError::InitFailed,
            ScriptError::ScriptTooLarge(100),
            ScriptError::Execution("err".into()),
            ScriptError::OpLimitExceeded,
            ScriptError::TimeLimitExceeded(Duration::from_secs(1)),
        ];
        for v in &variants {
            let dbg = format!("{:?}", v);
            assert!(!dbg.is_empty());
        }
    }

    #[test]
    fn error_propagates_via_question_mark() {
        fn fallible() -> Result<(), Box<dyn std::error::Error>> {
            Err(ScriptError::Execution("fail".into()))?
        }
        let result = fallible();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("fail"));
    }
}
