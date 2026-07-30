use crate::{error::MigrationError, value::ConfigValue};

/// A single schema migration step.
pub trait Migration {
    fn from_version(&self) -> u32;
    fn to_version(&self) -> u32;

    /// Transform `value` from `from_version` to `to_version`.
    ///
    /// # Errors
    /// Returns [`MigrationError::Step`] if the transformation fails.
    fn migrate(&self, value: ConfigValue) -> Result<ConfigValue, MigrationError>;
}

/// A sequence of [`Migration`] steps applied in version order.
#[derive(Default)]
pub struct MigrationChain {
    steps: Vec<Box<dyn Migration>>,
}

impl MigrationChain {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_step(mut self, step: impl Migration + 'static) -> Self {
        self.steps.push(Box::new(step));
        self
    }

    /// Run migrations from `current_version` up to `target_version`.
    ///
    /// # Errors
    /// Returns [`MigrationError::NoPath`] if no step covers the current version,
    /// or [`MigrationError::Step`] if a step's transformation fails.
    pub fn run(
        &self,
        mut value: ConfigValue,
        mut current_version: u32,
        target_version: u32,
    ) -> Result<ConfigValue, MigrationError> {
        while current_version < target_version {
            let step = self
                .steps
                .iter()
                .find(|s| s.from_version() == current_version)
                .ok_or(MigrationError::NoPath {
                    from: current_version,
                    to: target_version,
                })?;
            value = step.migrate(value)?;
            current_version = step.to_version();
        }
        Ok(value)
    }
}

/// Rename a top-level key in a `ConfigValue::Table`. No-op if `from` is absent.
pub fn rename_key(value: &mut ConfigValue, from: &str, to: &str) {
    if let ConfigValue::Table(table) = value {
        if let Some(v) = table.remove(from) {
            table.insert(to.to_owned(), v);
        }
    }
}
