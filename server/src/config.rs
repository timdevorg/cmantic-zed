//! Server settings (counterpart of `configuration.ts`).
//!
//! Zed sends `lsp.cmantic.settings` through `workspace/didChangeConfiguration`; the same keys
//! are also accepted in `initializationOptions`.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CurlyBraceFormat {
    /// Opening brace on its own line.
    NewLine,
    /// Opening brace on its own line for constructors and destructors, same line otherwise.
    NewLineCtorDtor,
    /// Opening brace on the same line as the signature.
    SameLine,
}

impl CurlyBraceFormat {
    pub fn brace_on_new_line(self, is_ctor_or_dtor: bool) -> bool {
        match self {
            Self::NewLine => true,
            Self::NewLineCtorDtor => is_ctor_or_dtor,
            Self::SameLine => false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CurlyBraceSettings {
    pub function: CurlyBraceFormat,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LanguageSettings {
    pub curly_brace_format: CurlyBraceSettings,
    /// Use `member{}` instead of `member()` in generated member initializer lists (C++ only).
    pub braced_initialization: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// File extensions (without the dot) treated as headers.
    pub header_extensions: Vec<String>,
    /// File extensions (without the dot) treated as source files.
    pub source_extensions: Vec<String>,
    pub cpp: LanguageSettings,
    pub c: LanguageSettings,
    /// Move the cursor into a definition added to another file.
    pub reveal_new_definition: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            header_extensions: ["h", "hpp", "hh", "hxx"].map(String::from).to_vec(),
            source_extensions: ["c", "cpp", "cc", "cxx"].map(String::from).to_vec(),
            cpp: LanguageSettings {
                curly_brace_format: CurlyBraceSettings {
                    function: CurlyBraceFormat::NewLineCtorDtor,
                },
                braced_initialization: false,
            },
            c: LanguageSettings {
                curly_brace_format: CurlyBraceSettings {
                    function: CurlyBraceFormat::NewLine,
                },
                braced_initialization: false,
            },
            reveal_new_definition: true,
        }
    }
}

impl Default for LanguageSettings {
    fn default() -> Self {
        Config::default().cpp
    }
}

impl Default for CurlyBraceSettings {
    fn default() -> Self {
        Config::default().cpp.curly_brace_format
    }
}

impl Config {
    /// Parses settings sent by the client. Accepts the settings object itself or one nested
    /// under a `"cmantic"` key. Invalid or missing values fall back to the defaults.
    pub fn from_value(value: &Value) -> Self {
        let value = value.get("cmantic").unwrap_or(value);
        if !value.is_object() {
            return Self::default();
        }
        // A missing `c` section must keep the C default (`newLine`), not the C++ one.
        let mut config: Self = match serde_json::from_value(value.clone()) {
            Ok(config) => config,
            Err(_) => return Self::lenient(value),
        };
        if value.pointer("/c/curlyBraceFormat/function").is_none() {
            config.c = Self::default().c;
        }
        config
    }

    /// Field-by-field parsing, used when the settings object as a whole doesn't deserialize.
    fn lenient(value: &Value) -> Self {
        let mut config = Self::default();
        let strings = |key: &str| -> Option<Vec<String>> {
            serde_json::from_value(value.get(key)?.clone()).ok()
        };
        let brace = |pointer: &str| -> Option<CurlyBraceFormat> {
            serde_json::from_value(value.pointer(pointer)?.clone()).ok()
        };
        if let Some(exts) = strings("headerExtensions") {
            config.header_extensions = exts;
        }
        if let Some(exts) = strings("sourceExtensions") {
            config.source_extensions = exts;
        }
        if let Some(format) = brace("/cpp/curlyBraceFormat/function") {
            config.cpp.curly_brace_format.function = format;
        }
        if let Some(format) = brace("/c/curlyBraceFormat/function") {
            config.c.curly_brace_format.function = format;
        }
        if let Some(braced) = value
            .pointer("/cpp/bracedInitialization")
            .and_then(Value::as_bool)
        {
            config.cpp.braced_initialization = braced;
        }
        if let Some(reveal) = value.get("revealNewDefinition").and_then(Value::as_bool) {
            config.reveal_new_definition = reveal;
        }
        config
    }

    pub fn function_brace_format(&self, is_c: bool) -> CurlyBraceFormat {
        if is_c {
            self.c.curly_brace_format.function
        } else {
            self.cpp.curly_brace_format.function
        }
    }

    fn extension_of(path: &str) -> Option<&str> {
        let name = path.rsplit(['/', '\\']).next()?;
        name.rsplit_once('.').map(|(_, ext)| ext)
    }

    pub fn is_header(&self, path: &str) -> bool {
        Self::extension_of(path).is_some_and(|ext| self.header_extensions.iter().any(|e| e == ext))
    }

    pub fn is_source(&self, path: &str) -> bool {
        Self::extension_of(path).is_some_and(|ext| self.source_extensions.iter().any(|e| e == ext))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults() {
        let config = Config::from_value(&Value::Null);
        assert_eq!(
            config.function_brace_format(false),
            CurlyBraceFormat::NewLineCtorDtor
        );
        assert_eq!(
            config.function_brace_format(true),
            CurlyBraceFormat::NewLine
        );
        assert!(config.reveal_new_definition);
        assert!(config.is_header("/a/b.hpp"));
        assert!(config.is_source("/a/b.cc"));
        assert!(!config.is_header("/a/b.cc"));
    }

    #[test]
    fn parses_settings() {
        let config = Config::from_value(&json!({
            "headerExtensions": ["h"],
            "revealNewDefinition": false,
            "cpp": { "curlyBraceFormat": { "function": "sameLine" }, "bracedInitialization": true }
        }));
        assert!(config.cpp.braced_initialization);
        assert_eq!(config.header_extensions, vec!["h"]);
        assert_eq!(config.source_extensions.len(), 4);
        assert_eq!(
            config.function_brace_format(false),
            CurlyBraceFormat::SameLine
        );
        assert_eq!(
            config.function_brace_format(true),
            CurlyBraceFormat::NewLine
        );
        assert!(!config.reveal_new_definition);
    }

    #[test]
    fn invalid_values_fall_back() {
        let config = Config::from_value(&json!({
            "cmantic": {
                "cpp": { "curlyBraceFormat": { "function": "bogus" } },
                "c": { "curlyBraceFormat": { "function": "sameLine" } }
            }
        }));
        assert_eq!(
            config.function_brace_format(false),
            CurlyBraceFormat::NewLineCtorDtor
        );
        assert_eq!(
            config.function_brace_format(true),
            CurlyBraceFormat::SameLine
        );
    }
}
