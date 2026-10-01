use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    headers: BTreeMap<String, String>,
}

impl Request {
    pub fn parse(head: &str) -> Option<Self> {
        let mut lines = head.split("\r\n");
        let mut start = lines.next()?.split(' ');
        let method = start.next()?.to_owned();
        let path = start.next()?.to_owned();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        Some(Self {
            method,
            path,
            headers,
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}
