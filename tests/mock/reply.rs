#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub stalls: bool,
}

impl Reply {
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            stalls: false,
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: "{}".to_owned(),
            stalls: false,
        }
    }

    pub fn stall() -> Self {
        Self {
            stalls: true,
            ..Self::status(200)
        }
    }
}
