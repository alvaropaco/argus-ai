//! Versioned capability contracts.

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CapabilityId;
use crate::authorization::BlastRadius;

/// Risk classification of a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    Read,
    LowRisk,
    Controlled,
    HighRisk,
    Destructive,
}

/// Whether an action can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    None,
    Reversible,
    PartiallyReversible,
}

/// An OS privilege a capability declares it requires.
///
/// The vocabulary mirrors ADR-0021 §1: Linux capabilities, namespaces, a seccomp
/// profile, and Landlock rules. Declaring a privilege is how a capability states
/// its least-privilege requirement, so that registration can fail closed when the
/// running sandbox does not grant it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsPrivilege {
    /// A Linux capability such as `CAP_KILL`.
    LinuxCapability(String),
    /// A namespace the capability must enter, such as `net` or `mnt`.
    Namespace(String),
    /// A seccomp profile the capability requires.
    Seccomp(String),
    /// A Landlock ruleset the capability requires.
    Landlock,
}

/// The set of OS privileges a capability declares, or the set a sandbox grants.
///
/// The same type serves both roles so a capability's requirement and the sandbox's
/// grant are compared with one rule, in one place.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PrivilegeDeclaration {
    privileges: Vec<OsPrivilege>,
}

impl PrivilegeDeclaration {
    /// A declaration requiring nothing.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn new(privileges: Vec<OsPrivilege>) -> Self {
        Self { privileges }
    }

    pub fn is_empty(&self) -> bool {
        self.privileges.is_empty()
    }

    pub fn privileges(&self) -> &[OsPrivilege] {
        &self.privileges
    }

    /// Whether every declared privilege is present in `granted`.
    ///
    /// An unlisted privilege is not granted: the test is closed, not open.
    pub fn is_satisfied_by(&self, granted: &PrivilegeDeclaration) -> bool {
        self.missing_from(granted).next().is_none()
    }

    /// The declared privileges that `granted` does not include.
    pub fn missing_from<'a>(
        &'a self,
        granted: &'a PrivilegeDeclaration,
    ) -> impl Iterator<Item = &'a OsPrivilege> {
        self.privileges
            .iter()
            .filter(move |required| !granted.privileges.contains(required))
    }
}

/// The declarative, versioned description of a capability.
///
/// The descriptor is metadata, not an authorization grant: the policy engine
/// remains authoritative (see ADR-016). It does, however, carry the observations
/// ADR-0020 §2 requires authorization to derive from — risk class and blast
/// radius — rather than accepting them from the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // schemas are serde_json::Value
pub struct CapabilityDescriptor {
    id: CapabilityId,
    provider: String,
    operation: String,
    risk_class: RiskClass,
    version: Version,
    input_schema: Value,
    output_schema: Value,
    reversibility: Reversibility,
    /// `None` means the capability did not declare a radius. It is deliberately
    /// not the same as a declared `BlastRadius::None`; see
    /// [`CapabilityDescriptor::effective_blast_radius`].
    #[serde(default)]
    blast_radius: Option<BlastRadius>,
    #[serde(default)]
    requires_approval: bool,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    privileged: bool,
    #[serde(default)]
    privileges: PrivilegeDeclaration,
}

impl CapabilityDescriptor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: CapabilityId,
        provider: impl Into<String>,
        operation: impl Into<String>,
        risk_class: RiskClass,
        version: Version,
        input_schema: Value,
        output_schema: Value,
        reversibility: Reversibility,
    ) -> Self {
        Self {
            id,
            provider: provider.into(),
            operation: operation.into(),
            risk_class,
            version,
            input_schema,
            output_schema,
            reversibility,
            blast_radius: None,
            requires_approval: false,
            timeout_seconds: None,
            privileged: false,
            privileges: PrivilegeDeclaration::none(),
        }
    }

    /// Declares the scope of impact this capability can produce.
    pub fn with_blast_radius(mut self, blast_radius: BlastRadius) -> Self {
        self.blast_radius = Some(blast_radius);
        self
    }

    /// Requires a local, per-invocation approval before this capability executes.
    pub fn requiring_approval(mut self) -> Self {
        self.requires_approval = true;
        self
    }

    /// Overrides the effective request timeout for this capability.
    pub fn with_timeout_seconds(mut self, seconds: u64) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    /// Marks the capability privileged and records the OS privileges it needs.
    ///
    /// Passing an empty declaration here is a configuration error that
    /// [`CapabilityRegistry::register`](crate::CapabilityRegistry::register)
    /// rejects, because a privileged capability that declares nothing cannot be
    /// least-privileged.
    pub fn privileged_with(mut self, privileges: PrivilegeDeclaration) -> Self {
        self.privileged = true;
        self.privileges = privileges;
        self
    }

    pub fn id(&self) -> &CapabilityId {
        &self.id
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn risk_class(&self) -> RiskClass {
        self.risk_class
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    pub fn output_schema(&self) -> &Value {
        &self.output_schema
    }

    pub fn reversibility(&self) -> Reversibility {
        self.reversibility
    }

    /// The declared blast radius, or `None` when the capability declared none.
    pub fn blast_radius(&self) -> Option<BlastRadius> {
        self.blast_radius
    }

    /// The blast radius authorization must use.
    ///
    /// A capability that cannot declare its blast radius is treated as `Host`,
    /// never as `None` (ADR-0020 §2). Classifying an unknown capability as
    /// harmless would bypass every downstream control.
    pub fn effective_blast_radius(&self) -> BlastRadius {
        self.blast_radius.unwrap_or(BlastRadius::Host)
    }

    pub fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    /// The capability's own timeout override, when it declared one.
    pub fn timeout_seconds(&self) -> Option<u64> {
        self.timeout_seconds
    }

    /// Whether the capability declared that it needs OS privilege.
    pub fn is_privileged(&self) -> bool {
        self.privileged
    }

    pub fn privileges(&self) -> &PrivilegeDeclaration {
        &self.privileges
    }

    /// Whether this capability can change the host or environment.
    ///
    /// This, not [`Self::is_privileged`], is what the local kill switch governs:
    /// an operation that changes the host is privileged to perform even when it
    /// needs no Linux capability of its own.
    pub fn changes_the_host(&self) -> bool {
        self.effective_blast_radius() != BlastRadius::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("argus.health.read").unwrap(),
            "argusd",
            "health.get",
            RiskClass::Read,
            Version::new(0, 1, 0),
            serde_json::json!({ "type": "object", "additionalProperties": false }),
            serde_json::json!({ "type": "object" }),
            Reversibility::None,
        )
    }

    #[test]
    fn risk_class_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&RiskClass::LowRisk).unwrap(),
            "\"low_risk\""
        );
    }

    #[test]
    fn descriptor_serde_round_trip() {
        let id = CapabilityId::new("argus.health.read").unwrap();
        let desc = CapabilityDescriptor::new(
            id.clone(),
            "argusd",
            "health.get",
            RiskClass::Read,
            Version::new(0, 1, 0),
            serde_json::json!({ "type": "object", "additionalProperties": false }),
            serde_json::json!({ "type": "object" }),
            Reversibility::None,
        );
        let json = serde_json::to_string(&desc).unwrap();
        let back: CapabilityDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(desc, back);
        assert_eq!(back.id(), &id);
        assert_eq!(back.version(), &Version::new(0, 1, 0));
    }

    #[test]
    fn an_undeclared_blast_radius_is_treated_as_host_never_none() {
        let desc = descriptor();
        assert_eq!(desc.blast_radius(), None, "nothing was declared");
        assert_eq!(
            desc.effective_blast_radius(),
            BlastRadius::Host,
            "an unknown radius must not be classified as harmless"
        );
    }

    #[test]
    fn a_declared_read_only_radius_stays_none() {
        let desc = descriptor().with_blast_radius(BlastRadius::None);
        assert_eq!(desc.effective_blast_radius(), BlastRadius::None);
        assert!(!desc.changes_the_host());
    }

    #[test]
    fn a_declared_host_radius_changes_the_host() {
        let desc = descriptor().with_blast_radius(BlastRadius::Host);
        assert!(desc.changes_the_host());
    }

    #[test]
    fn descriptors_written_before_this_schema_still_deserialize() {
        // A descriptor persisted without the privilege fields must remain readable,
        // and must land on the fail-safe default rather than a harmless one.
        let json = serde_json::json!({
            "id": "argus.health.read",
            "provider": "argusd",
            "operation": "health.get",
            "risk_class": "read",
            "version": "0.1.0",
            "input_schema": {},
            "output_schema": {},
            "reversibility": "none",
        });
        let back: CapabilityDescriptor = serde_json::from_value(json).unwrap();
        assert_eq!(back.effective_blast_radius(), BlastRadius::Host);
        assert!(!back.is_privileged());
        assert!(back.privileges().is_empty());
    }

    #[test]
    fn a_declaration_is_satisfied_only_when_every_privilege_is_granted() {
        let required =
            PrivilegeDeclaration::new(vec![OsPrivilege::LinuxCapability("CAP_KILL".into())]);
        let partial = PrivilegeDeclaration::none();
        let full = PrivilegeDeclaration::new(vec![
            OsPrivilege::LinuxCapability("CAP_KILL".into()),
            OsPrivilege::Namespace("mnt".into()),
        ]);

        assert!(!required.is_satisfied_by(&partial), "nothing granted");
        assert!(required.is_satisfied_by(&full), "a superset grants it");
        assert!(PrivilegeDeclaration::none().is_satisfied_by(&partial));
    }

    #[test]
    fn missing_privileges_are_reported_individually() {
        let required = PrivilegeDeclaration::new(vec![
            OsPrivilege::LinuxCapability("CAP_KILL".into()),
            OsPrivilege::Landlock,
        ]);
        let granted = PrivilegeDeclaration::none();
        let missing: Vec<_> = required.missing_from(&granted).cloned().collect();
        assert_eq!(
            missing,
            vec![
                OsPrivilege::LinuxCapability("CAP_KILL".into()),
                OsPrivilege::Landlock
            ]
        );
    }

    #[test]
    fn privilege_builder_marks_the_capability_privileged() {
        let desc =
            descriptor().privileged_with(PrivilegeDeclaration::new(vec![OsPrivilege::Landlock]));
        assert!(desc.is_privileged());
        assert!(!desc.privileges().is_empty());
    }
}
