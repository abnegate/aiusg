use crate::model::Provider;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{provider} support is not in this build; enable the `{feature}` feature")]
pub struct Unsupported {
    pub provider: Provider,
    pub feature: &'static str,
}
