/// A string that must never reach a log line, an error message or a Debug
/// dump. It has neither `Debug` nor `Display`, so this does not compile:
///
/// ```compile_fail
/// let s = converge::secret::Secret::new("k".to_string());
/// println!("{:?}", s);
/// ```
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}
