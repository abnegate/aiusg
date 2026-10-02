use chrono::{DateTime, Utc};

/// When an account can take work again, ordered from soonest to least known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Availability {
    /// No window is spent.
    Now,
    /// Every spent window has reset by this time.
    At(DateTime<Utc>),
    /// A spent window reports no reset time.
    Unknown,
}
