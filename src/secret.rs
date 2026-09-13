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

/// A field of an answer that holds a key or a topic decodes straight into a
/// `Secret`, so the value never sits in a struct that could be printed. Only
/// a value of the wrong type fails, and serde's message then shows that
/// value -- a string, the one type that can be a secret, never fails.
impl<'de> serde::Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Secret)
    }
}
