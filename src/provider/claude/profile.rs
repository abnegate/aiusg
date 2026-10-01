use serde::Deserialize;

use super::{ProfileAccount, ProfileOrganization};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
/// The Claude account profile.
#[non_exhaustive]
pub struct Profile {
    #[serde(default)]
    pub account: Option<ProfileAccount>,
    #[serde(default)]
    pub organization: Option<ProfileOrganization>,
}

impl Profile {
    /// The account's label, as [`ProfileAccount::label`] gives it.
    pub fn label(&self) -> Option<&str> {
        self.account.as_ref()?.label()
    }

    /// The organization's plan, as [`ProfileOrganization::plan`] gives it.
    pub fn plan(&self) -> Option<&str> {
        self.organization.as_ref()?.plan()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = r#"{"account":{"uuid":"abc12345-ffff","email":"jake@example.com","display_name":"Jake","full_name":"Jake Barnby","has_claude_max":true},"organization":{"uuid":"org-1","name":"Jake's Organization","organization_type":"claude_max","billing_type":"stripe_subscription","rate_limit_tier":"default_claude_max_20x"}}"#;

    fn parsed(json: &str) -> Profile {
        serde_json::from_str(json).expect("profile should parse")
    }

    #[test]
    fn the_profile_is_read_from_its_nested_shape() {
        let profile = parsed(LIVE);

        assert_eq!(
            profile
                .account
                .as_ref()
                .and_then(|account| account.email.as_deref()),
            Some("jake@example.com"),
            "the account label comes from account.email, not a flat field"
        );
        assert_eq!(
            profile
                .organization
                .as_ref()
                .and_then(|organization| organization.rate_limit_tier.as_deref()),
            Some("default_claude_max_20x")
        );
    }

    #[test]
    fn the_label_is_the_email_first() {
        assert_eq!(parsed(LIVE).label(), Some("jake@example.com"));
    }

    #[test]
    fn the_label_falls_back_through_display_name_full_name_and_uuid() {
        assert_eq!(
            parsed(r#"{"account":{"email":"","display_name":"Jake","full_name":"Jake Barnby"}}"#)
                .label(),
            Some("Jake"),
            "an empty email is skipped"
        );
        assert_eq!(
            parsed(r#"{"account":{"full_name":"Jake Barnby","uuid":"abc12345-ffff"}}"#).label(),
            Some("Jake Barnby")
        );
        assert_eq!(
            parsed(r#"{"account":{"uuid":"abc12345-ffff"}}"#).label(),
            Some("abc12345"),
            "a bare uuid is shortened to its first block"
        );
        assert_eq!(parsed(r#"{"account":{}}"#).label(), None);
        assert_eq!(parsed("{}").label(), None);
    }

    #[test]
    fn the_plan_is_the_rate_limit_tier_first() {
        assert_eq!(parsed(LIVE).plan(), Some("default_claude_max_20x"));
    }

    #[test]
    fn the_plan_falls_back_through_organization_type_and_billing_type() {
        assert_eq!(
            parsed(
                r#"{"organization":{"rate_limit_tier":null,"organization_type":"claude_pro","billing_type":"stripe_subscription"}}"#
            )
            .plan(),
            Some("claude_pro")
        );
        assert_eq!(
            parsed(
                r#"{"organization":{"organization_type":"","billing_type":"stripe_subscription"}}"#
            )
            .plan(),
            Some("stripe_subscription"),
            "an empty organization type is skipped"
        );
        assert_eq!(parsed(r#"{"organization":{}}"#).plan(), None);
        assert_eq!(
            parsed(r#"{"account":{"email":"jake@example.com"}}"#).plan(),
            None,
            "a profile without the organization has no plan"
        );
    }
}
