use crate::model::Provider;

/// A provider was called in a build that left out the feature adding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{provider} support is not in this build; enable the `{feature}` feature")]
#[non_exhaustive]
pub struct Unsupported {
    pub provider: Provider,
    /// The Cargo feature that adds `provider`.
    pub feature: &'static str,
}
