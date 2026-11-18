//! Documents open in the editor (full text sync).

pub struct Document {
    pub text: String,
    pub version: i32,
    pub language_id: String,
}

impl Document {
    pub fn new(text: String, version: i32, language_id: String) -> Self {
        Self {
            text,
            version,
            language_id,
        }
    }

    /// Replaces the full text (the server uses full document sync).
    pub fn update(&mut self, text: String, version: i32) {
        self.text = text;
        self.version = version;
    }

    pub fn is_c(&self) -> bool {
        self.language_id == "c"
    }
}
