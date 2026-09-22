//! Capability registry: registers typed capabilities and their metadata.

use std::collections::BTreeMap;

use crate::error::DomainError;
use crate::{CapabilityDescriptor, CapabilityId, PrivilegeDeclaration};

/// A registry of capabilities, keyed by [`CapabilityId`].
///
/// When a sandbox grant is configured, registration fails closed: a capability
/// whose declared privileges the sandbox does not grant is rejected rather than
/// published, so the installation never advertises a capability it cannot
/// actually perform (ADR-0021 §2).
#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    descriptors: BTreeMap<CapabilityId, CapabilityDescriptor>,
    sandbox: Option<PrivilegeDeclaration>,
}

impl CapabilityRegistry {
    /// A registry with no modelled sandbox.
    ///
    /// Privileged declarations are still validated for completeness, but nothing
    /// is compared against a grant. Use [`Self::with_sandbox`] in the daemon.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry that fails closed against the privileges the sandbox grants.
    pub fn with_sandbox(sandbox: PrivilegeDeclaration) -> Self {
        Self {
            descriptors: BTreeMap::new(),
            sandbox: Some(sandbox),
        }
    }

    /// Registers a capability descriptor, rejecting duplicates and privilege
    /// declarations the sandbox cannot honour.
    pub fn register(&mut self, descriptor: CapabilityDescriptor) -> Result<(), DomainError> {
        let id = descriptor.id().clone();

        if self.descriptors.contains_key(&id) {
            return Err(DomainError::DuplicateCapability(id.as_str().to_string()));
        }

        if descriptor.is_privileged() && descriptor.privileges().is_empty() {
            return Err(DomainError::UndeclaredPrivileges(
                id.as_str().to_string(),
            ));
        }

        if let Some(sandbox) = &self.sandbox {
            let missing: Vec<String> = descriptor
                .privileges()
                .missing_from(sandbox)
                .map(|privilege| format!("{privilege:?}"))
                .collect();
            if !missing.is_empty() {
                return Err(DomainError::UngrantedPrivileges(
                    id.as_str().to_string(),
                    missing.join(", "),
                ));
            }
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

    /// The privileges declared by at least one registered capability.
    ///
    /// This is the least-privilege set the packaged sandbox is allowed to grant
    /// (SC-019): a sandbox may not hold a blanket grant.
    pub fn declared_privileges(&self) -> PrivilegeDeclaration {
        let mut all: Vec<_> = self
            .descriptors
            .values()
            .flat_map(|descriptor| descriptor.privileges().privileges().iter().cloned())
            .collect();
        all.sort();
        all.dedup();
        PrivilegeDeclaration::new(all)
    }
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::*;
    use crate::authorization::BlastRadius;
    use crate::{OsPrivilege, Reversibility, RiskClass};

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

    fn cap_kill() -> Vec<OsPrivilege> {
        vec![OsPrivilege::LinuxCapability("CAP_KILL".into())]
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

    #[test]
    fn a_privileged_capability_declaring_nothing_is_rejected() {
        let mut registry = CapabilityRegistry::new();
        let desc = descriptor("host.process.signal").privileged_with(PrivilegeDeclaration::none());
        assert!(matches!(
            registry.register(desc),
            Err(DomainError::UndeclaredPrivileges(_))
        ));
        assert!(registry.is_empty(), "nothing may be registered");
    }

    #[test]
    fn registration_fails_closed_when_the_sandbox_grants_no_capability() {
        let mut registry = CapabilityRegistry::with_sandbox(PrivilegeDeclaration::none());
        let desc = descriptor("host.process.signal")
            .privileged_with(PrivilegeDeclaration::new(cap_kill()));

        let error = registry.register(desc).unwrap_err();
        assert!(
            matches!(&error, DomainError::UngrantedPrivileges(id, missing)
                if id == "host.process.signal" && missing.contains("CAP_KILL")),
            "the refusal must name the capability and the missing privilege: {error:?}"
        );
        assert!(registry.is_empty(), "nothing may be registered");
    }

    #[test]
    fn a_granted_privilege_permits_registration() {
        let mut registry = CapabilityRegistry::with_sandbox(PrivilegeDeclaration::new(cap_kill()));
        let desc = descriptor("host.process.signal")
            .privileged_with(PrivilegeDeclaration::new(cap_kill()));
        registry.register(desc).expect("the sandbox grants it");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn an_environment_changing_capability_needs_no_os_privilege_to_register() {
        // A host service restart is privileged to perform but needs no Linux
        // capability of its own: it is authorized through the service manager.
        let mut registry = CapabilityRegistry::with_sandbox(PrivilegeDeclaration::none());
        let desc = descriptor("host.service.restart")
            .with_blast_radius(BlastRadius::Host)
            .requiring_approval();
        registry.register(desc).expect("no OS privilege declared");
        assert!(registry.get(&CapabilityId::new("host.service.restart").unwrap()).is_some());
    }

    #[test]
    fn declared_privileges_are_the_union_of_what_capabilities_need() {
        let mut registry = CapabilityRegistry::with_sandbox(PrivilegeDeclaration::new(vec![
            OsPrivilege::LinuxCapability("CAP_KILL".into()),
            OsPrivilege::Landlock,
        ]));
        registry
            .register(
                descriptor("host.process.signal")
                    .privileged_with(PrivilegeDeclaration::new(cap_kill())),
            )
            .unwrap();
        registry
            .register(
                descriptor("host.filesystem.inspect")
                    .privileged_with(PrivilegeDeclaration::new(vec![OsPrivilege::Landlock])),
            )
            .unwrap();

        let declared = registry.declared_privileges();
        assert_eq!(declared.privileges().len(), 2);
        assert!(declared.is_satisfied_by(&PrivilegeDeclaration::new(vec![
            OsPrivilege::LinuxCapability("CAP_KILL".into()),
            OsPrivilege::Landlock,
        ])));
    }
}
