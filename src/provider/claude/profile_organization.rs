use serde::Deserialize;

/// The organization half of a [`Profile`](super::Profile).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct ProfileOrganization {
    /// The rate limit tier, such as `default_claude_max_20x`.
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
    /// The kind of organization, such as `claude_max`.
    #[serde(default)]
    pub organization_type: Option<String>,
    /// How the organization is billed, such as `stripe_subscription`.
    #[serde(default)]
    pub billing_type: Option<String>,
}

impl ProfileOrganization {
    /// The first non-empty of the rate limit tier, organization type and
    /// billing type.
    pub fn plan(&self) -> Option<&str> {
        [
            &self.rate_limit_tier,
            &self.organization_type,
            &self.billing_type,
        ]
        .into_iter()
        .filter_map(Option::as_deref)
        .find(|value| !value.is_empty())
    }
}
