//! LSP request handlers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower_lsp_server::jsonrpc::{Error, Result};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

use crate::actions::add_definition::{self, FileInput, Target};
use crate::config::Config;
use crate::document::Document;
use crate::header_source::HeaderSourceCache;

/// Command run by the "Add Definition in matching source file" code action.
pub const COMMAND_ADD_DEFINITION_IN_SOURCE: &str = "cmantic.addDefinitionInSourceFile";

/// Arguments of [`COMMAND_ADD_DEFINITION_IN_SOURCE`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddDefinitionArgs {
    pub uri: Uri,
    pub position: Position,
}

pub struct Backend {
    client: Client,
    documents: RwLock<HashMap<Uri, Document>>,
    config: RwLock<Config>,
    roots: RwLock<Vec<PathBuf>>,
    header_source: Mutex<HeaderSourceCache>,
}

/// Text of a file plus whether it is C, read from the editor or from disk.
struct FileText {
    text: String,
    is_c: bool,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            config: RwLock::new(Config::default()),
            roots: RwLock::new(Vec::new()),
            header_source: Mutex::new(HeaderSourceCache::default()),
        }
    }

    /// Prefers the open document's text over the file on disk.
    async fn file_text(&self, uri: &Uri) -> Option<FileText> {
        if let Some(doc) = self.documents.read().await.get(uri) {
            return Some(FileText {
                text: doc.text.clone(),
                is_c: doc.is_c(),
            });
        }
        let path = uri.to_file_path()?;
        let text = tokio::fs::read_to_string(&path).await.ok()?;
        let is_c = path.extension().is_some_and(|e| e == "c");
        Some(FileText { text, is_c })
    }

    async fn matching_source(&self, path: &Path, config: &Config) -> Option<Uri> {
        if !config.is_header(&path.to_string_lossy()) {
            return None;
        }
        let roots = self.roots.read().await.clone();
        let path = path.to_path_buf();
        let config = config.clone();
        let mut cache = self.header_source.lock().await;
        let found = cache.get(&path, &config, &roots)?;
        Uri::from_file_path(found)
    }

    async fn add_definition_in_source(&self, args: AddDefinitionArgs) -> Result<Option<Value>> {
        let config = self.config.read().await.clone();
        let invalid = |message: &str| Error::invalid_params(message.to_string());
        let path = args
            .uri
            .to_file_path()
            .ok_or_else(|| invalid("not a file URI"))?;
        let header = self
            .file_text(&args.uri)
            .await
            .ok_or_else(|| invalid("cannot read the file"))?;
        let Some(source_uri) = self.matching_source(&path, &config).await else {
            self.show_warning("No matching source file was found.")
                .await;
            return Ok(None);
        };
        let source = self
            .file_text(&source_uri)
            .await
            .ok_or_else(|| invalid("cannot read the matching source file"))?;

        let decl = FileInput {
            text: &header.text,
            is_header: true,
            is_c: header.is_c,
        };
        let target = FileInput {
            text: &source.text,
            is_header: false,
            is_c: source.is_c,
        };
        let Some(insertion) = add_definition::add_definition(
            &decl,
            args.position,
            Target::MatchingSource,
            Some(&target),
            &config,
        ) else {
            self.show_warning("No undefined function declaration at the cursor.")
                .await;
            return Ok(None);
        };

        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(
                source_uri.clone(),
                vec![TextEdit::new(insertion.range, insertion.new_text)],
            )])),
            ..Default::default()
        };
        let applied = self
            .client
            .apply_edit(edit)
            .await
            .is_ok_and(|response| response.applied);
        if applied && config.reveal_new_definition {
            let cursor = Range::new(insertion.cursor, insertion.cursor);
            let _ = self
                .client
                .show_document(ShowDocumentParams {
                    uri: source_uri,
                    external: Some(false),
                    take_focus: Some(true),
                    selection: Some(cursor),
                })
                .await;
        }
        Ok(None)
    }

    async fn show_warning(&self, message: &str) {
        self.client
            .show_message(MessageType::WARNING, message)
            .await;
    }

    async fn update_config(&self, value: &Value) {
        *self.config.write().await = Config::from_value(value);
        self.header_source.lock().await.clear();
    }
}

fn wants_refactor(only: Option<&Vec<CodeActionKind>>) -> bool {
    only.is_none_or(|kinds| {
        kinds.iter().any(|kind| {
            let kind = kind.as_str();
            kind.is_empty() || kind == "refactor" || "refactor".starts_with(&format!("{kind}."))
        })
    })
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .iter()
            .flatten()
            .filter_map(|folder| folder.uri.to_file_path().map(|p| p.into_owned()))
            .collect();
        #[allow(deprecated)]
        if roots.is_empty()
            && let Some(root) = params.root_uri.as_ref().and_then(|uri| uri.to_file_path())
        {
            roots.push(root.into_owned());
        }
        *self.roots.write().await = roots;
        if let Some(options) = &params.initialization_options {
            self.update_config(options).await;
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "cmantic-ls".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![CodeActionKind::REFACTOR]),
                        resolve_provider: Some(true),
                        ..Default::default()
                    },
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![COMMAND_ADD_DEFINITION_IN_SOURCE.into()],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "cmantic-ls initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        self.update_config(&params.settings).await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        self.documents.write().await.insert(
            doc.uri,
            Document::new(doc.text, doc.version, doc.language_id),
        );
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Full sync: the last change holds the entire document.
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let mut documents = self.documents.write().await;
        match documents.get_mut(&uri) {
            Some(doc) => doc.update(change.text, version),
            None => {
                documents.insert(uri, Document::new(change.text, version, "cpp".into()));
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents
            .write()
            .await
            .remove(&params.text_document.uri);
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        if !wants_refactor(params.context.only.as_ref()) {
            return Ok(None);
        }
        let uri = params.text_document.uri;
        let position = params.range.start;
        let config = self.config.read().await.clone();
        let Some(file) = self.file_text(&uri).await else {
            return Ok(None);
        };
        let path = uri.to_file_path().map(|p| p.into_owned());
        let is_header = path
            .as_ref()
            .is_some_and(|p| config.is_header(&p.to_string_lossy()));

        let decl = FileInput {
            text: &file.text,
            is_header,
            is_c: file.is_c,
        };
        if !add_definition::has_declaration_at(&decl, position) {
            return Ok(None);
        }
        let source_uri = match &path {
            Some(path) if is_header => self.matching_source(path, &config).await,
            _ => None,
        };
        let source_text = match &source_uri {
            Some(uri) => self.file_text(uri).await,
            None => None,
        };
        let source = source_text.as_ref().map(|s| FileInput {
            text: &s.text,
            is_header: false,
            is_c: s.is_c,
        });

        let mut actions = Vec::new();
        for target in add_definition::available_targets(&decl, position, source.as_ref()) {
            let action = match target {
                Target::MatchingSource => {
                    let args = AddDefinitionArgs {
                        uri: uri.clone(),
                        position,
                    };
                    CodeAction {
                        title: target.title().into(),
                        kind: Some(CodeActionKind::REFACTOR),
                        command: Some(Command {
                            title: target.title().into(),
                            command: COMMAND_ADD_DEFINITION_IN_SOURCE.into(),
                            arguments: Some(vec![serde_json::to_value(args).unwrap_or_default()]),
                        }),
                        ..Default::default()
                    }
                }
                Target::CurrentFile | Target::Inline => {
                    let Some(insertion) = add_definition::add_definition(
                        &decl,
                        position,
                        target,
                        source.as_ref(),
                        &config,
                    ) else {
                        continue;
                    };
                    CodeAction {
                        title: target.title().into(),
                        kind: Some(CodeActionKind::REFACTOR),
                        edit: Some(WorkspaceEdit {
                            changes: Some(HashMap::from([(
                                uri.clone(),
                                vec![TextEdit::new(insertion.range, insertion.new_text)],
                            )])),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                }
            };
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
        Ok((!actions.is_empty()).then_some(actions))
    }

    async fn code_action_resolve(&self, action: CodeAction) -> Result<CodeAction> {
        // Actions are returned fully formed.
        Ok(action)
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        match params.command.as_str() {
            COMMAND_ADD_DEFINITION_IN_SOURCE => {
                let args = params
                    .arguments
                    .into_iter()
                    .next()
                    .and_then(|value| serde_json::from_value::<AddDefinitionArgs>(value).ok())
                    .ok_or_else(|| Error::invalid_params("expected {uri, position}"))?;
                self.add_definition_in_source(args).await
            }
            _ => Err(Error::invalid_params(format!(
                "unknown command: {}",
                params.command
            ))),
        }
    }
}
