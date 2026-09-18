//! Capability registry: registers typed capabilities and their metadata.

use std::collections::BTreeMap;

use crate::error::DomainError;
use crate::{CapabilityDescriptor, CapabilityId};

/// A registry of capabilities, keyed by [`CapabilityId`].
#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    descriptors: BTreeMap<CapabilityId, CapabilityDescriptor>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a capability descriptor. Fails on duplicate registration.
    pub fn register(&mut self, descriptor: CapabilityDescriptor) -> Result<(), DomainError> {
        let id = descriptor.id().clone();
        if self.descriptors.contains_key(&id) {
            return Err(DomainError::DuplicateCapability(id.as_str().to_string()));
        }
        self.descriptors.insert(id, descriptor);
        Ok(())
    }

    pub fn get(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.descriptors.get(id)
    }

    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.descriptors.contains_key(id)
    }

    pub fn list(&self) -> impl Iterator<Item = &CapabilityDescriptor> {
        self.descriptors.values()
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::*;
    use crate::Reversibility;
    use crate::RiskClass;

    fn descriptor(capability: &str) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new(capability).unwrap(),
            "argusd",
            "health.get",
            RiskClass::Read,
            Version::new(0, 1, 0),
            serde_json::json!({}),
            serde_json::json!({}),
            Reversibility::None,
        )
    }

    #[test]
    fn register_and_get() {
        let mut registry = CapabilityRegistry::new();
        let desc = descriptor("argus.health.read");
        registry.register(desc.clone()).unwrap();
        assert!(registry.contains(&desc.id().clone()));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut registry = CapabilityRegistry::new();
        let desc = descriptor("argus.health.read");
        registry.register(desc.clone()).unwrap();
        assert!(matches!(
            registry.register(desc),
            Err(DomainError::DuplicateCapability(_))
        ));
    }
}
