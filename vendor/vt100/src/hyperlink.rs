use std::sync::Arc;

/// OSC 8 destination shared by the printed cells that carry it.
#[derive(Debug, Eq, PartialEq)]
pub struct Hyperlink {
    id: Option<Box<str>>,
    url: Option<url::Url>,
}

impl Hyperlink {
    pub(crate) fn parse(params: &[u8], uri: &[u8]) -> Option<Arc<Self>> {
        if uri.is_empty() {
            return None;
        }
        let url = std::str::from_utf8(uri)
            .ok()
            .and_then(|uri| url::Url::parse(uri).ok())
            .filter(|url| matches!(url.scheme(), "http" | "https"));
        let id = std::str::from_utf8(params)
            .ok()?
            .trim()
            .split(':')
            .find_map(|param| param.strip_prefix("id="))
            .filter(|id| !id.is_empty())
            .map(Box::from);
        Some(Arc::new(Self { id, url }))
    }

    /// The canonical HTTP(S) URL shown for confirmation and passed to the browser.
    pub fn url(&self) -> Option<&str> {
        self.url.as_ref().map(url::Url::as_str)
    }

    /// Anonymous spans are identified by the opening sequence. An explicit ID
    /// can join neighboring spans printed with the same ID and destination.
    pub fn same_span(&self, other: &Self) -> bool {
        std::ptr::eq(self, other) || (self.id.is_some() && self == other)
    }
}
