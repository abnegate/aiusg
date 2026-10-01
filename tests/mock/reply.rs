#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

impl Reply {
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: "{}".to_owned(),
        }
    }
}
