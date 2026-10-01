use serde::Deserialize;

const UUID_PREFIX: usize = 8;

/// The account half of a [`Profile`](super::Profile).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct ProfileAccount {
    /// The account's unique id.
    #[serde(default)]
    pub uuid: Option<String>,
    /// The account's email address.
    #[serde(default)]
    pub email: Option<String>,
    /// The name the user chose to be shown by.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The user's full name.
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
