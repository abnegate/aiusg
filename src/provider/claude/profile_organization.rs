use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct ProfileOrganization {
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
    #[serde(default)]
    pub organization_type: Option<String>,
    #[serde(default)]
    pub billing_type: Option<String>,
}

impl ProfileOrganization {
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
