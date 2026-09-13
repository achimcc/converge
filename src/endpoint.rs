/// What a request or response body is, in OpenAPI component names.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    One(&'static str),
    List(&'static str),
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
