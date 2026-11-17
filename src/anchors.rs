//! Where a ticket belongs (repository, branch, environment) and how a patch changes that.

/// Where a ticket belongs, plus its fingerprint. Empty strings count as absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Anchors {
    pub machine: Option<String>,
    pub repo: Option<String>,
    pub environment: Option<String>,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub fingerprint: Option<String>,
}

impl Anchors {
    /// Column name and value, in column order.
    pub(crate) fn columns(&self) -> [(&'static str, &Option<String>); 6] {
        [
            ("machine", &self.machine),
            ("repo", &self.repo),
            ("environment", &self.environment),
            ("agent", &self.agent),
            ("session", &self.session),
            ("fingerprint", &self.fingerprint),
        ]
    }
}

/// An edit to the anchors: `Some(None)` clears one, `None` leaves it.
#[derive(Debug, Clone, Default)]
pub struct AnchorPatch {
    pub machine: Option<Option<String>>,
    pub repo: Option<Option<String>>,
    pub environment: Option<Option<String>>,
    pub agent: Option<Option<String>>,
    pub session: Option<Option<String>>,
    pub fingerprint: Option<Option<String>>,
}

impl AnchorPatch {
    pub(crate) fn columns(&self) -> [(&'static str, &Option<Option<String>>); 6] {
        [
            ("machine", &self.machine),
            ("repo", &self.repo),
            ("environment", &self.environment),
            ("agent", &self.agent),
            ("session", &self.session),
            ("fingerprint", &self.fingerprint),
        ]
    }
}

pub(crate) fn clean_anchor(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}
