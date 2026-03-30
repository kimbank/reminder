pub(super) struct SearchFilter {
    needle: Option<String>,
}

impl SearchFilter {
    pub(super) fn new(raw: &str) -> Self {
        let trimmed = raw.trim();
        let needle = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_lowercase())
        };
        Self { needle }
    }

    pub(super) fn matches_any(&self, fields: &[&str]) -> bool {
        match &self.needle {
            None => true,
            Some(needle) => fields
                .iter()
                .any(|field| field.to_lowercase().contains(needle)),
        }
    }
}
