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

/// Where a generated member function (getter, setter) is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum DefinitionLocation {
    /// Inside the class body.
    #[serde(rename = "inline", alias = "Generate definition inline")]
    Inline,
    /// In the same file, below the class.
    #[serde(
        rename = "belowClass",
        alias = "currentFile",
        alias = "Generate definition below class body"
    )]
    BelowClass,
    /// In the matching source file; falls back to [`Self::BelowClass`] when there is none
    /// (or the class is a template).
    #[serde(
        rename = "matchingSourceFile",
        alias = "sourceFile",
        alias = "Generate definition in matching source file"
    )]
    MatchingSourceFile,
}

/// Naming style of generated getters and setters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CaseStyle {
    #[serde(rename = "camelCase")]
    CamelCase,
    #[serde(rename = "snake_case")]
    SnakeCase,
    #[serde(rename = "PascalCase")]
    PascalCase,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessorSettings {
    pub getter_definition_location: DefinitionLocation,
    pub setter_definition_location: DefinitionLocation,
    /// Name getters of `bool` members `isFoo` instead of `getFoo`.
    pub bool_getter_is_prefix: bool,
}

impl Default for AccessorSettings {
    fn default() -> Self {
        Self {
            getter_definition_location: DefinitionLocation::Inline,
            setter_definition_location: DefinitionLocation::Inline,
            bool_getter_is_prefix: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LanguageSettings {
    pub curly_brace_format: CurlyBraceSettings,
    /// Use `member{}` instead of `member()` in generated member initializer lists (C++ only).
    pub braced_initialization: bool,
    /// Getter and setter generation (C++ only).
    pub accessor: AccessorSettings,
    /// Prefix members with `this->` in generated member functions (C++ only).
    pub use_explicit_this_pointer: bool,
    /// Generate comparison operators as `friend` functions (C++ only).
    pub friend_comparison_operators: bool,
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
    /// Naming style of generated getters and setters.
    pub case_style: CaseStyle,
    /// Move Definition moves leading comments even when a declaration is left behind.
    pub always_move_comments: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            header_extensions: ["h", "hpp", "hh", "hxx"].map(String::from).to_vec(),
            source_extensions: ["c", "cpp", "cc", "cxx"].map(String::from).to_vec(),
            cpp: LanguageSettings::default_cpp(),
            c: LanguageSettings {
                curly_brace_format: CurlyBraceSettings {
                    function: CurlyBraceFormat::NewLine,
                },
                ..LanguageSettings::default_cpp()
            },
            reveal_new_definition: true,
            case_style: CaseStyle::CamelCase,
            always_move_comments: true,
        }
    }
}

impl LanguageSettings {
    fn default_cpp() -> Self {
        Self {
            curly_brace_format: CurlyBraceSettings {
                function: CurlyBraceFormat::NewLineCtorDtor,
            },
            braced_initialization: false,
            accessor: AccessorSettings::default(),
            use_explicit_this_pointer: false,
            friend_comparison_operators: false,
        }
    }
}

impl Default for LanguageSettings {
    fn default() -> Self {
        Self::default_cpp()
    }
}

impl Default for CurlyBraceSettings {
    fn default() -> Self {
        LanguageSettings::default_cpp().curly_brace_format
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
        fn set<T: serde::de::DeserializeOwned>(field: &mut T, value: &Value, pointer: &str) {
            if let Some(parsed) = value
                .pointer(pointer)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
            {
                *field = parsed;
            }
        }
        let mut config = Self::default();
        set(&mut config.header_extensions, value, "/headerExtensions");
        set(&mut config.source_extensions, value, "/sourceExtensions");
        set(
            &mut config.reveal_new_definition,
            value,
            "/revealNewDefinition",
        );
        set(&mut config.case_style, value, "/caseStyle");
        set(
            &mut config.always_move_comments,
            value,
            "/alwaysMoveComments",
        );
        for (settings, lang) in [(&mut config.cpp, "cpp"), (&mut config.c, "c")] {
            set(
                &mut settings.curly_brace_format.function,
                value,
                &format!("/{lang}/curlyBraceFormat/function"),
            );
            set(
                &mut settings.braced_initialization,
                value,
                &format!("/{lang}/bracedInitialization"),
            );
            let accessor = &mut settings.accessor;
            for (field, key) in [
                (
                    &mut accessor.getter_definition_location,
                    "getterDefinitionLocation",
                ),
                (
                    &mut accessor.setter_definition_location,
                    "setterDefinitionLocation",
                ),
            ] {
                set(field, value, &format!("/{lang}/accessor/{key}"));
            }
            set(
                &mut accessor.bool_getter_is_prefix,
                value,
                &format!("/{lang}/accessor/boolGetterIsPrefix"),
            );
            set(
                &mut settings.use_explicit_this_pointer,
                value,
                &format!("/{lang}/useExplicitThisPointer"),
            );
            set(
                &mut settings.friend_comparison_operators,
                value,
                &format!("/{lang}/friendComparisonOperators"),
            );
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
        assert!(config.always_move_comments);
        assert!(config.is_header("/a/b.hpp"));
        assert!(config.is_source("/a/b.cc"));
        assert!(!config.is_header("/a/b.cc"));
    }

    #[test]
    fn parses_settings() {
        let config = Config::from_value(&json!({
            "headerExtensions": ["h"],
            "revealNewDefinition": false,
            "alwaysMoveComments": false,
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
        assert!(!config.always_move_comments);
        // Also when another value is invalid.
        let config = Config::from_value(&json!({ "alwaysMoveComments": false, "caseStyle": 3 }));
        assert!(!config.always_move_comments);
    }

    #[test]
    fn accessor_settings() {
        let config = Config::from_value(&json!({
            "caseStyle": "snake_case",
            "cpp": {
                "accessor": {
                    "getterDefinitionLocation": "belowClass",
                    "setterDefinitionLocation": "Generate definition in matching source file",
                    "boolGetterIsPrefix": true
                },
                "useExplicitThisPointer": true,
                "friendComparisonOperators": true
            }
        }));
        assert_eq!(config.case_style, CaseStyle::SnakeCase);
        let accessor = &config.cpp.accessor;
        assert_eq!(
            accessor.getter_definition_location,
            DefinitionLocation::BelowClass
        );
        assert_eq!(
            accessor.setter_definition_location,
            DefinitionLocation::MatchingSourceFile
        );
        assert!(accessor.bool_getter_is_prefix);
        assert!(config.cpp.use_explicit_this_pointer);
        assert!(config.cpp.friend_comparison_operators);
        let defaults = Config::default();
        assert_eq!(defaults.case_style, CaseStyle::CamelCase);
        assert_eq!(
            defaults.cpp.accessor.getter_definition_location,
            DefinitionLocation::Inline
        );
        // One invalid value keeps the others.
        let config = Config::from_value(&json!({
            "caseStyle": "PascalCase",
            "cpp": { "accessor": { "getterDefinitionLocation": "nowhere" }, "useExplicitThisPointer": true }
        }));
        assert_eq!(config.case_style, CaseStyle::PascalCase);
        assert!(config.cpp.use_explicit_this_pointer);
        assert_eq!(
            config.cpp.accessor.getter_definition_location,
            DefinitionLocation::Inline
        );
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
