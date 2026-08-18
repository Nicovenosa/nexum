use std::sync::Arc;

use nexum_agent::{agent::state::State, middleware::r#trait::Middleware};

use crate::plugin::loader::LoadedPlugin;

pub struct PluginMiddleware {
    plugins: Arc<Vec<LoadedPlugin>>,
}

impl PluginMiddleware {
    pub fn new(plugins: Vec<LoadedPlugin>) -> Self {
        Self {
            plugins: Arc::new(plugins),
        }
    }

    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }
}

#[async_trait::async_trait]
impl<S: State> Middleware<S> for PluginMiddleware {
    fn name(&self) -> &str {
        "PluginMiddleware"
    }

    async fn before_agent(&self, _state: &mut S) -> nexum_agent::error::AgentResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use nexum_agent::{agent::state::AgentState, middleware::r#trait::Middleware};

    use super::*;
    use crate::plugin::loader::tests::make_manifest_with_commands;
    include!("middleware_test.rs");
}
