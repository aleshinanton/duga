//! Provider registry for named LLM clients.

use crate::LlmClient;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ProviderRegistryError {
    #[error("duplicate llm provider: {0}")]
    DuplicateProvider(String),
    #[error("llm provider not found: {0}")]
    NotFound(String),
    #[error("no default llm provider configured")]
    NoDefaultProvider,
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn LlmClient>>>,
    default_provider: RwLock<Option<String>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        <Self as Default>::default()
    }

    pub fn register(
        &self,
        name: impl Into<String>,
        provider: Arc<dyn LlmClient>,
    ) -> Result<(), ProviderRegistryError> {
        let name = name.into();
        let mut providers = self.providers.write().unwrap();
        if providers.contains_key(&name) {
            return Err(ProviderRegistryError::DuplicateProvider(name));
        }

        if self.default_provider.read().unwrap().is_none() {
            *self.default_provider.write().unwrap() = Some(name.clone());
        }
        providers.insert(name, provider);
        Ok(())
    }

    pub fn set_default(&self, name: impl Into<String>) -> Result<(), ProviderRegistryError> {
        let name = name.into();
        if !self.providers.read().unwrap().contains_key(&name) {
            return Err(ProviderRegistryError::NotFound(name));
        }
        *self.default_provider.write().unwrap() = Some(name);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn LlmClient>, ProviderRegistryError> {
        self.providers
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| ProviderRegistryError::NotFound(name.into()))
    }

    pub fn default_provider(&self) -> Result<Arc<dyn LlmClient>, ProviderRegistryError> {
        let name = self
            .default_provider
            .read()
            .unwrap()
            .clone()
            .ok_or(ProviderRegistryError::NoDefaultProvider)?;
        self.get(&name)
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dummy::DummyClient;

    #[test]
    fn registry_registers_and_sets_default() {
        let registry = ProviderRegistry::new();
        registry
            .register("dummy", Arc::new(DummyClient::default()))
            .unwrap();

        assert_eq!(registry.names(), vec!["dummy"]);
        assert_eq!(registry.default_provider().unwrap().model(), "dummy");
    }

    #[test]
    fn registry_rejects_duplicates() {
        let registry = ProviderRegistry::new();
        registry
            .register("dummy", Arc::new(DummyClient::default()))
            .unwrap();
        let err = registry
            .register("dummy", Arc::new(DummyClient::default()))
            .unwrap_err();

        assert_eq!(
            err,
            ProviderRegistryError::DuplicateProvider("dummy".into())
        );
    }
}
