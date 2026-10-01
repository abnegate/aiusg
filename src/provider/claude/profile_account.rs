use serde::Deserialize;

const UUID_PREFIX: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
/// The account half of a [`Profile`](super::Profile).
#[non_exhaustive]
pub struct ProfileAccount {
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
}

impl ProfileAccount {
    /// The first non-empty of the email, display name and full name, else the
    /// first block of the uuid.
    pub fn label(&self) -> Option<&str> {
        [&self.email, &self.display_name, &self.full_name]
            .into_iter()
            .filter_map(Option::as_deref)
            .find(|value| !value.is_empty())
            .or_else(|| {
                self.uuid
                    .as_deref()
                    .filter(|uuid| !uuid.is_empty())
                    .map(|uuid| uuid.get(..UUID_PREFIX).unwrap_or(uuid))
            })
    }
}
