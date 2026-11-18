//! LSP request handlers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower_lsp_server::jsonrpc::{Error, Result};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

use crate::actions::accessors::{self, AccessorKind};
use crate::actions::add_definition::{self, BulkTarget, FileInput, Initializers, Target};
use crate::actions::member::GeneratedEdit;
use crate::actions::operators;
use crate::config::Config;
use crate::document::Document;
use crate::header_source::HeaderSourceCache;

/// Command run by the "Add Definition in matching source file" code action.
pub const COMMAND_ADD_DEFINITION_IN_SOURCE: &str = "cmantic.addDefinitionInSourceFile";

/// Generic command for code actions that edit another file or move the cursor: the server
/// recomputes the action named by [`ActionArgs::action`], sends `workspace/applyEdit` and then
/// `window/showDocument`.
pub const COMMAND_APPLY_ACTION: &str = "cmantic.applyAction";

/// Action ids used with [`COMMAND_APPLY_ACTION`].
pub mod action_id {
    pub const ADD_DEFINITION_MATCHING_SOURCE: &str = "addDefinition.matchingSource";
    pub const CONSTRUCTOR_MATCHING_SOURCE: &str = "generateConstructor.matchingSource";
    pub const ADD_DEFINITIONS_MATCHING_SOURCE: &str = "addDefinitions.matchingSource";
    pub const ADD_DEFINITIONS_CURRENT_FILE: &str = "addDefinitions.currentFile";
    pub const GENERATE_GETTER_SETTER: &str = "generateGetterSetter";
    pub const GENERATE_GETTER: &str = "generateGetter";
    pub const GENERATE_SETTER: &str = "generateSetter";
}

fn accessor_action_id(kind: AccessorKind) -> &'static str {
    match kind {
        AccessorKind::GetterAndSetter => action_id::GENERATE_GETTER_SETTER,
        AccessorKind::Getter => action_id::GENERATE_GETTER,
        AccessorKind::Setter => action_id::GENERATE_SETTER,
    }
}

fn accessor_kind(action: &str) -> Option<AccessorKind> {
    match action {
        action_id::GENERATE_GETTER_SETTER => Some(AccessorKind::GetterAndSetter),
        action_id::GENERATE_GETTER => Some(AccessorKind::Getter),
        action_id::GENERATE_SETTER => Some(AccessorKind::Setter),
        _ => None,
    }
}

/// Arguments of [`COMMAND_ADD_DEFINITION_IN_SOURCE`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddDefinitionArgs {
    pub uri: Uri,
    pub position: Position,
}

/// Arguments of [`COMMAND_APPLY_ACTION`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionArgs {
    pub uri: Uri,
    pub position: Position,
    pub action: String,
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

    /// Runs a command-based action (see [`action_id`]).
    async fn run_action(&self, args: ActionArgs) -> Result<Option<Value>> {
        let config = self.config.read().await.clone();
        let invalid = |message: &str| Error::invalid_params(message.to_string());
        let path = args
            .uri
            .to_file_path()
            .ok_or_else(|| invalid("not a file URI"))?
            .into_owned();
        let file = self
            .file_text(&args.uri)
            .await
            .ok_or_else(|| invalid("cannot read the file"))?;
        let is_header = config.is_header(&path.to_string_lossy());
        let decl = FileInput {
            text: &file.text,
            is_header,
            is_c: file.is_c,
        };

        let source_uri = self.matching_source(&path, &config).await;
        let source_text = match &source_uri {
            Some(uri) => Some(
                self.file_text(uri)
                    .await
                    .ok_or_else(|| invalid("cannot read the matching source file"))?,
            ),
            None => None,
        };
        let source = source_text.as_ref().map(|s| FileInput {
            text: &s.text,
            is_header: false,
            is_c: s.is_c,
        });
        let needs_source = matches!(
            args.action.as_str(),
            action_id::ADD_DEFINITION_MATCHING_SOURCE
                | action_id::CONSTRUCTOR_MATCHING_SOURCE
                | action_id::ADD_DEFINITIONS_MATCHING_SOURCE
        );
        let source_uri = match source_uri {
            Some(uri) => Some(uri),
            None if needs_source => {
                self.show_warning("No matching source file was found.")
                    .await;
                return Ok(None);
            }
            None => None,
        };

        // Edits per file, and where to put the cursor afterwards.
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        let reveal: Option<(Uri, Position)> = match args.action.as_str() {
            action_id::ADD_DEFINITION_MATCHING_SOURCE | action_id::CONSTRUCTOR_MATCHING_SOURCE => {
                let initializers = if args.action == action_id::CONSTRUCTOR_MATCHING_SOURCE {
                    Initializers::All
                } else {
                    Initializers::None
                };
                let Some(insertion) = add_definition::add_definition_with(
                    &decl,
                    args.position,
                    Target::MatchingSource,
                    source.as_ref(),
                    initializers,
                    &config,
                ) else {
                    self.show_warning("No undefined function declaration at the cursor.")
                        .await;
                    return Ok(None);
                };
                let uri = source_uri.clone().unwrap_or_else(|| args.uri.clone());
                changes.insert(
                    uri.clone(),
                    vec![TextEdit::new(insertion.range, insertion.new_text)],
                );
                Some((uri, insertion.cursor))
            }
            action_id::ADD_DEFINITIONS_MATCHING_SOURCE
            | action_id::ADD_DEFINITIONS_CURRENT_FILE => {
                let (target, target_uri) =
                    if args.action == action_id::ADD_DEFINITIONS_MATCHING_SOURCE {
                        (
                            BulkTarget::MatchingSource,
                            source_uri.clone().unwrap_or_else(|| args.uri.clone()),
                        )
                    } else {
                        (BulkTarget::CurrentFile, args.uri.clone())
                    };
                let Some(bulk) =
                    add_definition::add_definitions(&decl, target, source.as_ref(), &config)
                else {
                    self.show_info("No undefined functions found in this file.")
                        .await;
                    return Ok(None);
                };
                changes.insert(target_uri.clone(), bulk.edits);
                Some((target_uri, bulk.cursor))
            }
            other => {
                let Some(kind) = accessor_kind(other) else {
                    return Err(invalid(&format!("unknown action: {other}")));
                };
                let Some(generated) = accessors::generate_accessors(
                    &decl,
                    args.position,
                    kind,
                    source.as_ref(),
                    &config,
                ) else {
                    self.show_info("Nothing to generate for this member variable.")
                        .await;
                    return Ok(None);
                };
                add_generated(&mut changes, generated, &args.uri, source_uri.as_ref())
            }
        };

        let edit = WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        };
        let applied = self
            .client
            .apply_edit(edit)
            .await
            .is_ok_and(|response| response.applied);
        if applied
            && config.reveal_new_definition
            && let Some((uri, cursor)) = reveal
        {
            let _ = self
                .client
                .show_document(ShowDocumentParams {
                    uri,
                    external: Some(false),
                    take_focus: Some(true),
                    selection: Some(Range::new(cursor, cursor)),
                })
                .await;
        }
        Ok(None)
    }

    async fn show_info(&self, message: &str) {
        self.client.show_message(MessageType::INFO, message).await;
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

/// Adds a [`GeneratedEdit`] to `changes`; returns the cursor in the source file, if any.
fn add_generated(
    changes: &mut HashMap<Uri, Vec<TextEdit>>,
    generated: GeneratedEdit,
    uri: &Uri,
    source_uri: Option<&Uri>,
) -> Option<(Uri, Position)> {
    if !generated.edits.is_empty() {
        changes.insert(uri.clone(), generated.edits);
    }
    let source_uri = source_uri?;
    if !generated.source_edits.is_empty() {
        changes.insert(source_uri.clone(), generated.source_edits);
    }
    generated
        .source_cursor
        .map(|cursor| (source_uri.clone(), cursor))
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
                    commands: vec![
                        COMMAND_ADD_DEFINITION_IN_SOURCE.into(),
                        COMMAND_APPLY_ACTION.into(),
                    ],
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
        if !crate::actions::may_offer_actions_at(&decl, position) {
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

        let command_action = |title: &str, action: &str| {
            let args = ActionArgs {
                uri: uri.clone(),
                position,
                action: action.into(),
            };
            CodeActionOrCommand::CodeAction(CodeAction {
                title: title.into(),
                kind: Some(CodeActionKind::REFACTOR),
                command: Some(Command {
                    title: title.into(),
                    command: COMMAND_APPLY_ACTION.into(),
                    arguments: Some(vec![serde_json::to_value(args).unwrap_or_default()]),
                }),
                ..Default::default()
            })
        };
        let edit_action = |title: &str, edits: Vec<TextEdit>| {
            CodeActionOrCommand::CodeAction(CodeAction {
                title: title.into(),
                kind: Some(CodeActionKind::REFACTOR),
                edit: Some(WorkspaceEdit {
                    changes: Some(HashMap::from([(uri.clone(), edits)])),
                    ..Default::default()
                }),
                ..Default::default()
            })
        };

        let targets = add_definition::available_targets(&decl, position, source.as_ref());
        let constructor =
            !targets.is_empty() && add_definition::offers_constructor_initializers(&decl, position);
        let mut actions = Vec::new();
        for initializers in [Initializers::None, Initializers::All] {
            if initializers == Initializers::All && !constructor {
                continue;
            }
            for &target in &targets {
                let title = match initializers {
                    Initializers::All => match target.constructor_title() {
                        Some(title) => title,
                        None => continue,
                    },
                    _ => target.title(),
                };
                match target {
                    Target::MatchingSource => {
                        let id = if initializers == Initializers::All {
                            action_id::CONSTRUCTOR_MATCHING_SOURCE
                        } else {
                            action_id::ADD_DEFINITION_MATCHING_SOURCE
                        };
                        actions.push(command_action(title, id));
                    }
                    Target::CurrentFile | Target::Inline => {
                        if let Some(insertion) = add_definition::add_definition_with(
                            &decl,
                            position,
                            target,
                            source.as_ref(),
                            initializers,
                            &config,
                        ) {
                            actions.push(edit_action(
                                title,
                                vec![TextEdit::new(insertion.range, insertion.new_text)],
                            ));
                        }
                    }
                }
            }
        }
        for target in add_definition::available_bulk_targets(&decl, position, source.as_ref()) {
            let id = match target {
                BulkTarget::MatchingSource => action_id::ADD_DEFINITIONS_MATCHING_SOURCE,
                BulkTarget::CurrentFile => action_id::ADD_DEFINITIONS_CURRENT_FILE,
            };
            actions.push(command_action(target.title(), id));
        }
        for action in accessors::available_accessors(&decl, position, source.is_some(), &config) {
            if action.uses_matching_source {
                actions.push(command_action(
                    &action.title,
                    accessor_action_id(action.kind),
                ));
            } else if let Some(generated) =
                accessors::generate_accessors(&decl, position, action.kind, None, &config)
            {
                actions.push(edit_action(&action.title, generated.edits));
            }
        }
        for action in operators::available_operators(&decl, position) {
            if let Some(generated) =
                operators::generate_operators(&decl, position, action.kind, &config)
            {
                actions.push(edit_action(&action.title, generated.edits));
            }
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
                self.run_action(ActionArgs {
                    uri: args.uri,
                    position: args.position,
                    action: action_id::ADD_DEFINITION_MATCHING_SOURCE.into(),
                })
                .await
            }
            COMMAND_APPLY_ACTION => {
                let args = params
                    .arguments
                    .into_iter()
                    .next()
                    .and_then(|value| serde_json::from_value::<ActionArgs>(value).ok())
                    .ok_or_else(|| Error::invalid_params("expected {uri, position, action}"))?;
                self.run_action(args).await
            }
            _ => Err(Error::invalid_params(format!(
                "unknown command: {}",
                params.command
            ))),
        }
    }
}
