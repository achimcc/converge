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
