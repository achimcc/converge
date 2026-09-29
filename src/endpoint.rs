/// What a request or response body is, in OpenAPI component names.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// One object, compared field by field with a wire type of that name.
    One(&'static str),
    /// A list of such objects.
    List(&'static str),
    /// One object whose fields a spec names by path (Jellyfin's
    /// configuration documents): the endpoint and component are checked, the
    /// fields with `schema::check_paths` against the spec.
    Document(&'static str),
    /// A list of such documents (Servarr's root folders).
    Documents(&'static str),
    /// A component the description declares without properties (Jellyfin's
    /// `BasePluginConfiguration`): only the reference itself is checked, and
    /// the task's documentation says its fields are checked at runtime only.
    Opaque(&'static str),
}

/// One HTTP endpoint a task uses. Task code reads `path` from here, so the
/// schema check and the requests cannot name different endpoints.
#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: &'static str,
    pub request: Option<Shape>,
    pub response: Option<Shape>,
}

/// Whether an id a service answered can stand as one segment of a request
/// path: letters, digits, `-` and `_`, at most 128 of them. A ULID, a UUID
/// with or without dashes and an integer all are; `..`, `/`, `?`, `#`, `%`
/// and whitespace are not, so an answer cannot point a `DELETE` or a write
/// at another path of the same origin (audit 3, B2-CV-5).
pub fn is_path_segment(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::is_path_segment;

    #[test]
    fn a_path_segment_is_a_plain_id_and_nothing_that_moves_the_path() {
        for id in [
            "01K52Z6P7B8C9D0E1F2G3H4J5K",
            "5b3c35a3-d25d-45ac-bbdb-4cc57e7ffd21",
            "098873858ff54b6c9a23aefae609dd20",
            "root",
            "42",
        ] {
            assert!(is_path_segment(id), "{id}");
        }
        for id in [
            "", "..", "../x", "a/b", "1?x", "1#x", "%2e%2e", "a b", "a\n", "ä",
        ] {
            assert!(!is_path_segment(id), "{id:?}");
        }
        assert!(!is_path_segment(&"a".repeat(129)));
    }
}
