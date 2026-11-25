//! Provider connection checks share the engine's real tool protocol.
use crate::ai_config::AiProvider;
pub fn test_connection(provider: AiProvider, model: &str) -> Result<(), String> {
    crate::engine::model::connection_test(provider, model)
}
