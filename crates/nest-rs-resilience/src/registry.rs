//! [`Policies`] — an app's policies, by name.

use std::collections::BTreeMap;

use crate::policy::Policy;

/// An app's policies by name: built once at boot, provided to every service
/// that calls out, so the callers of one dependency share its breaker.
///
/// ```
/// use nest_rs_resilience::{Policies, presets};
///
/// let policies = Policies::new()
///     .with(presets::outbound_http("carrier").build()?)?
///     .with(presets::background("exports").build()?)?;
/// assert_eq!(policies.require("carrier")?.name(), "carrier");
/// assert!(policies.get("payments").is_none());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct Policies {
    by_name: BTreeMap<String, Policy>,
}

/// A second policy under a name already taken.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a resilience policy named `{0}` is already registered")]
pub struct DuplicatePolicy(pub String);

/// A policy asked for by a name none is registered under.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("no resilience policy is named `{0}`")]
pub struct UnknownPolicy(pub String);

impl Policies {
    /// No policy yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `policy` under its name.
    ///
    /// # Errors
    ///
    /// [`DuplicatePolicy`] when the name is taken: two policies for one
    /// dependency would split its breaker in two.
    pub fn insert(&mut self, policy: Policy) -> Result<(), DuplicatePolicy> {
        let name = policy.name().to_owned();
        if self.by_name.contains_key(&name) {
            return Err(DuplicatePolicy(name));
        }
        self.by_name.insert(name, policy);
        Ok(())
    }

    /// `self`, with `policy` registered.
    ///
    /// # Errors
    ///
    /// [`DuplicatePolicy`], as [`insert`](Self::insert).
    pub fn with(mut self, policy: Policy) -> Result<Self, DuplicatePolicy> {
        self.insert(policy)?;
        Ok(self)
    }

    /// The policy named `name`, if any.
    pub fn get(&self, name: &str) -> Option<&Policy> {
        self.by_name.get(name)
    }

    /// The policy named `name`.
    ///
    /// # Errors
    ///
    /// [`UnknownPolicy`] when none is.
    pub fn require(&self, name: &str) -> Result<&Policy, UnknownPolicy> {
        self.get(name).ok_or_else(|| UnknownPolicy(name.to_owned()))
    }

    /// Every policy, by name.
    pub fn iter(&self) -> impl Iterator<Item = &Policy> {
        self.by_name.values()
    }

    /// How many policies are registered.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether none is.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_registers_once() {
        let mut policies = Policies::new();
        policies
            .insert(Policy::builder("a").build().unwrap())
            .unwrap();
        let again = policies.insert(Policy::builder("a").build().unwrap());
        assert_eq!(again, Err(DuplicatePolicy("a".to_owned())));
        assert_eq!(policies.len(), 1);
        assert_eq!(
            policies.require("b").unwrap_err().to_string(),
            "no resilience policy is named `b`"
        );
    }
}
