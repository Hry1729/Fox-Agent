use fox_engine_protocol::ExecutionAuthority;
use std::env::VarError;

pub(super) fn select(
    frozen: Option<ExecutionAuthority>,
    configured: Result<String, VarError>,
) -> Result<ExecutionAuthority, String> {
    if let Some(authority) = frozen {
        return Ok(authority);
    }
    match configured {
        // All launch paths (including plain `pnpm tauri.cmd dev`) use Kernel.
        // Legacy is an explicit process-level opt-in, never a build fallback.
        Err(VarError::NotPresent) => Ok(ExecutionAuthority::Authoritative),
        Ok(value) if value == "legacy" => Ok(ExecutionAuthority::Legacy),
        Ok(value) if value == "authoritative" => Ok(ExecutionAuthority::Authoritative),
        _ => Err("FOX_KERNEL_MODE must be legacy or authoritative".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_environment_uses_the_built_in_default() {
        assert_eq!(select(None, Err(VarError::NotPresent)).unwrap(), ExecutionAuthority::Authoritative);
    }

    #[test]
    fn explicit_override_is_validated() {
        assert_eq!(select(None, Ok("legacy".into())).unwrap(), ExecutionAuthority::Legacy);
        assert_eq!(select(None, Ok("authoritative".into())).unwrap(), ExecutionAuthority::Authoritative);
        assert!(select(None, Ok("typo".into())).is_err());
    }

    #[test]
    fn existing_binding_is_not_reinterpreted_by_a_new_default() {
        for authority in [ExecutionAuthority::Legacy, ExecutionAuthority::Authoritative] {
            assert_eq!(select(Some(authority), Err(VarError::NotPresent)).unwrap(), authority);
            assert_eq!(select(Some(authority), Ok("typo".into())).unwrap(), authority);
        }
    }
}
