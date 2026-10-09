//! Worker Drive operations use only the injected Workspace API. Observations
//! below are bounded preconditions, never a hierarchy replica or a grant cache.
pub mod backend;
pub(crate) mod delegation;
#[cfg(test)]
mod tests;
pub mod wip;

use crate::{
    feature::{
        FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule,
        ToolContribution, ToolDeclaration,
    },
    worker::WorkspaceClient,
};
use agen::tool::{
    Attachment, ImageAttachment, Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta,
    ToolOutput,
};
use async_trait::async_trait;
use backend::{DriveBackend, DriveError};
use schemars::JsonSchema;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use server_api::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use workdir::{WorkdirPath, WorkdirSessionCapability, WorkdirSessionRouter};

pub const FEATURE_ID: &str = "drive";
const MAX_OBSERVATIONS: usize = 256;

#[derive(Default)]
struct Observations {
    entries: HashMap<String, DriveEntry>,
    order: VecDeque<String>,
}
impl Observations {
    fn insert(&mut self, entry: DriveEntry) {
        let key = entry.entry.node_id.clone();
        self.order.retain(|old| old != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key, entry);
        while self.order.len() > MAX_OBSERVATIONS {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }
    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
        self.order.retain(|old| old != key);
    }
}

#[derive(Clone)]
pub struct DriveFeature {
    pub(super) backend: DriveBackend,
    pub(super) router: Arc<WorkdirSessionRouter>,
    observations: Arc<Mutex<Observations>>,
    nonce: Arc<str>,
    pub(super) permissions: Option<manifest::ToolPermissionConfig>,
}
impl DriveFeature {
    pub fn configured(
        client: Arc<dyn WorkspaceClient>,
        enabled: bool,
        router: Arc<WorkdirSessionRouter>,
    ) -> Option<Self> {
        (enabled
            && client.is_available()
            && client
                .workspace_id()
                .is_some_and(|id| !id.is_empty() && !id.chars().any(char::is_control)))
        .then(|| Self::new(client, router))
    }
    pub fn new(client: Arc<dyn WorkspaceClient>, router: Arc<WorkdirSessionRouter>) -> Self {
        Self {
            backend: DriveBackend::new(client),
            router,
            observations: Arc::new(Mutex::new(Observations::default())),
            nonce: uuid::Uuid::now_v7().to_string().into(),
            permissions: None,
        }
    }
    pub fn with_permissions(mut self, permissions: Option<manifest::ToolPermissionConfig>) -> Self {
        self.permissions = permissions;
        self
    }
    pub fn tools(&self) -> Vec<(&'static str, ToolDefinition)> {
        SPECS
            .iter()
            .map(|spec| {
                let feature = self.clone();
                let spec = *spec;
                let definition: ToolDefinition = Arc::new(move || {
                    (
                        ToolMeta::new(spec.name)
                            .description(spec.description)
                            .input_schema((spec.schema)()),
                        Arc::new(DriveTool {
                            feature: feature.clone(),
                            name: spec.name,
                        }) as Arc<dyn Tool>,
                    )
                });
                (spec.name, definition)
            })
            .collect()
    }
    fn observe(&self, entry: DriveEntry) -> Result<(), DriveError> {
        self.backend.validate_entry(&entry)?;
        self.observations
            .lock()
            .map_err(|_| DriveError::Unavailable)?
            .insert(entry);
        Ok(())
    }
    fn observed(
        &self,
        id: &DriveEntryRef,
        pinned: Option<&DriveEntry>,
    ) -> Result<DriveEntry, DriveError> {
        self.backend.validate_ref(id)?;
        if let Some(pinned) = pinned {
            if pinned.entry != *id {
                return Err(DriveError::Denied);
            }
            return Ok(pinned.clone());
        }
        self.observations.lock().map_err(|_| DriveError::Unavailable)?.entries.get(&id.node_id).cloned().ok_or_else(|| DriveError::Invalid("read or inspect metadata for this entry before mutation; observations are bounded and not persisted authority".into()))
    }
    fn request_id(&self, context: &ToolExecutionContext) -> String {
        digest(format!("{}:{}", self.nonce, context.execution_id()).as_bytes())
    }
    pub(super) async fn execute(
        &self,
        name: &str,
        input: &str,
        context: &ToolExecutionContext,
        pinned: Option<&DriveEntry>,
    ) -> Result<DriveOutput, DriveError> {
        self.backend.workspace()?;
        let request_id = self.request_id(context);
        let mut attachments = Vec::new();
        let mut mutated_id = None;
        let value = match name {
            "DriveRoot" => {
                let _: EmptyArgs = decode(input)?;
                let entry = self.backend.root().await?;
                self.observe(entry.clone())?;
                entry_value(entry)
            }
            "DriveMetadata" => {
                let args: EntryArgs = decode(input)?;
                let entry = self.backend.metadata(&args.entry).await?;
                self.observe(entry.clone())?;
                entry_value(entry)
            }
            "DriveList" => {
                let args: ListArgs = decode(input)?;
                if args
                    .limit
                    .is_some_and(|n| n == 0 || n > DRIVE_PAGE_MAX_LIMIT)
                {
                    return Err(DriveError::Limit);
                }
                let parent = match args.parent {
                    Some(parent) => parent,
                    None => self.backend.root().await?.entry,
                };
                let response = self.backend.list(&parent, args.limit, args.after).await?;
                page_value(response)
            }
            "DriveSearch" => {
                let args: DriveSearchQuery = decode(input)?;
                if args.query.len() > 512 || args.after.as_ref().is_some_and(|s| s.len() > 2048) {
                    return Err(DriveError::Limit);
                }
                page_value(self.backend.search(args).await?)
            }
            "DriveRead" => {
                let args: ReadArgs = decode(input)?;
                if args.max_bytes == 0 || args.max_bytes > DRIVE_CHUNK_MAX_BYTES {
                    return Err(DriveError::Limit);
                }
                let read = self.backend.read(&args.entry, args.max_bytes).await?;
                if pinned.is_some_and(|entry| entry.revision != read.entry.revision) {
                    return Err(DriveError::Conflict);
                }
                self.observe(read.entry.clone())?;
                json!({"entry": entry_value(read.entry), "text": read.text, "truncated":read.truncated})
            }
            "DriveViewImage" => {
                let args: EntryArgs = decode(input)?;
                let entry = if let Some(entry) = pinned {
                    self.observed(&args.entry, Some(entry))?
                } else {
                    self.backend.metadata(&args.entry).await?
                };
                let bytes = self
                    .backend
                    .bytes(&entry, tools::view_image::MAX_IMAGE_BYTES)
                    .await?;
                let media = tools::view_image::detect_image_mime(&bytes).ok_or_else(|| {
                    DriveError::Invalid("supported image bytes required: PNG/JPEG/GIF/WebP".into())
                })?;
                let size = bytes.len();
                attachments.push(Attachment::Image(ImageAttachment::new(
                    media,
                    Arc::<[u8]>::from(bytes),
                )));
                self.observe(entry.clone())?;
                json!({"entry":entry_value(entry), "attached":true, "mime_type":media, "bytes":size})
            }
            "DriveCreateFolder" => {
                let args: FolderArgs = decode(input)?;
                self.backend.validate_ref(&args.parent)?;
                mutation_value(
                    self.backend
                        .mutate(DriveMutationRequest {
                            request_id,
                            mutation: DriveMutation::CreateFolder {
                                parent: args.parent,
                                name: args.name,
                            },
                        })
                        .await?,
                )
            }
            "DriveCreateText" => {
                let args: CreateArgs = decode(input)?;
                self.backend.validate_ref(&args.parent)?;
                let text = checked_text(args.content)?;
                mutation_value(
                    self.backend
                        .mutate(DriveMutationRequest {
                            request_id,
                            mutation: DriveMutation::CreateText {
                                parent: args.parent,
                                name: args.name,
                                text,
                                content_type: args.content_type,
                            },
                        })
                        .await?,
                )
            }
            "DriveWrite" => {
                let args: WriteArgs = decode(input)?;
                let entry = self.observed(&args.entry, pinned)?;
                let text = checked_text(args.content)?;
                mutated_id = Some(entry.entry.node_id.clone());
                mutation_value(
                    self.backend
                        .mutate(DriveMutationRequest {
                            request_id,
                            mutation: DriveMutation::UpdateText {
                                id: entry.entry,
                                expected_revision: entry.revision,
                                text,
                                content_type: entry.content_type.unwrap_or_else(default_media),
                            },
                        })
                        .await?,
                )
            }
            "DriveEdit" => {
                let args: EditArgs = decode(input)?;
                let entry = self.observed(&args.entry, pinned)?;
                let edit = fs_operation::text::EditArgs {
                    old_string: args.old_string,
                    new_string: args.new_string,
                    replace_all: args.replace_all,
                };
                edit.validate(text_limits()).map_err(text_error)?;
                let read = self
                    .backend
                    .read(&entry.entry, DRIVE_TEXT_MAX_BYTES as u32)
                    .await?;
                if read.entry.revision != entry.revision {
                    return Err(DriveError::Conflict);
                }
                if read.truncated {
                    return Err(DriveError::Limit);
                }
                let text = fs_operation::text::edit(&read.text, &edit, text_limits())
                    .map_err(text_error)?
                    .content;
                mutated_id = Some(entry.entry.node_id.clone());
                mutation_value(
                    self.backend
                        .mutate(DriveMutationRequest {
                            request_id,
                            mutation: DriveMutation::UpdateText {
                                id: entry.entry,
                                expected_revision: entry.revision,
                                text,
                                content_type: entry.content_type.unwrap_or_else(default_media),
                            },
                        })
                        .await?,
                )
            }
            "DriveRelocate" => {
                let args: RelocateArgs = decode(input)?;
                self.backend.validate_ref(&args.parent)?;
                let entry = self.observed(&args.entry, pinned)?;
                mutated_id = Some(entry.entry.node_id.clone());
                mutation_value(
                    self.backend
                        .mutate(DriveMutationRequest {
                            request_id,
                            mutation: DriveMutation::Relocate {
                                id: entry.entry,
                                expected_revision: entry.revision,
                                parent: args.parent,
                                name: args.name,
                            },
                        })
                        .await?,
                )
            }
            "DriveDelete" => {
                let args: EntryArgs = decode(input)?;
                let entry = self.observed(&args.entry, pinned)?;
                mutated_id = Some(entry.entry.node_id.clone());
                mutation_value(
                    self.backend
                        .mutate(DriveMutationRequest {
                            request_id,
                            mutation: DriveMutation::Delete {
                                id: entry.entry,
                                expected_revision: entry.revision,
                            },
                        })
                        .await?,
                )
            }
            "DriveSaveWorkdir" => {
                let args: SaveArgs = decode(input)?;
                self.backend.validate_ref(&args.parent)?;
                // Drive API rechecks write at publication; source uses the selected
                // Workdir read broker, with no OS-path or command fallback.
                let selected = self
                    .router
                    .resolve(Some(&args.target_workdir))
                    .map_err(|_| DriveError::Denied)?;
                if !selected
                    .session
                    .capabilities()
                    .supports(WorkdirSessionCapability::Read)
                {
                    return Err(DriveError::Denied);
                }
                if args.path.starts_with('/')
                    || args.path.contains('\\')
                    || args.path.split('/').any(|part| part == "..")
                {
                    return Err(DriveError::Invalid(
                        "logical Workdir-relative path required".into(),
                    ));
                }
                let path = WorkdirPath::new(&args.path).map_err(|_| {
                    DriveError::Invalid("logical Workdir-relative path required".into())
                })?;
                let bytes = tools::view_image::read_workdir_bytes(
                    selected.session.as_ref(),
                    path,
                    DRIVE_FILE_MAX_BYTES as usize,
                )
                .await
                .map_err(|_| {
                    DriveError::Invalid(
                        "Workdir source denied, changed, oversized or unavailable".into(),
                    )
                })?;
                let query = DriveUploadQuery {
                    operation: DriveUploadOperation::Create,
                    request_id,
                    entry_workspace_id: args.parent.workspace_id,
                    parent_id: Some(args.parent.node_id),
                    name: Some(args.name),
                    id: None,
                    expected_revision: None,
                    content_type: args.content_type,
                    size: bytes.len() as u32,
                    sha256: digest(&bytes),
                };
                mutation_value(self.backend.upload(query, bytes).await?)
            }
            "DriveRequestStatus" => {
                let args: StatusArgs = decode(input)?;
                serde_json::to_value(self.backend.status(&args.request_id).await?)
                    .map_err(|_| DriveError::Unavailable)?
            }
            _ => return Err(DriveError::Invalid("unknown operation".into())),
        };
        // A successful mutation does not silently substitute its new revision
        // into a prior observation. The caller observes explicitly next time.
        if let Some(id) = mutated_id {
            self.observations
                .lock()
                .map_err(|_| DriveError::Unavailable)?
                .remove(&id);
        }
        Ok(DriveOutput { value, attachments })
    }
}

pub(super) struct DriveOutput {
    pub value: Value,
    pub attachments: Vec<Attachment>,
}
impl DriveOutput {
    fn tool(self, name: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            summary: format!("{name} completed through Workspace Drive API"),
            content: Some(
                serde_json::to_string(&self.value)
                    .map_err(|_| ToolError::Internal("Drive result encoding failed".into()))?,
            ),
            attachments: self.attachments,
        })
    }
}
struct DriveTool {
    feature: DriveFeature,
    name: &'static str,
}
#[async_trait]
impl Tool for DriveTool {
    async fn execute(
        &self,
        input: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.feature
            .execute(self.name, input, &context, None)
            .await
            .map_err(tool_error)?
            .tool(self.name)
    }
}
fn tool_error(error: DriveError) -> ToolError {
    match error {
        DriveError::Conflict => ToolError::StructuredConflict {
            code: "drive_conflict".into(),
            message: error.to_string(),
        },
        DriveError::Invalid(_) | DriveError::Limit => ToolError::InvalidArgument(error.to_string()),
        _ => ToolError::ExecutionFailed(error.to_string()),
    }
}
impl FeatureModule for DriveFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID,"Workspace Drive").with_description("Explicit Backend-authorized Drive documents, images and artifact publication; no Workdir required").with_instruction(drive_instruction());
        for spec in SPECS {
            descriptor = descriptor.with_tool(ToolDeclaration::new(spec.name, spec.description));
        }
        descriptor
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .instructions()
            .register(crate::feature::FeatureInstructionContribution::new(
                drive_instruction(),
            ))?;
        for (name, definition) in self.tools() {
            context
                .tools()
                .register(ToolContribution::new(name, definition))?;
        }
        Ok(())
    }
}

fn drive_instruction() -> crate::feature::FeatureInstructionDeclaration {
    crate::feature::FeatureInstructionDeclaration::new(
        crate::feature::FeatureInstructionId::builtin("drive-workflow"),
        "common.drive",
        "Authorized Drive operation guidance",
    )
    .expect("static Drive instruction")
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EntryArgs {
    entry: DriveEntryRef,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    parent: Option<DriveEntryRef>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 200))]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    entry: DriveEntryRef,
    #[serde(default = "default_read")]
    #[schemars(range(min = 1, max = 65536))]
    max_bytes: u32,
}
fn default_read() -> u32 {
    DRIVE_CHUNK_MAX_BYTES
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FolderArgs {
    parent: DriveEntryRef,
    name: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    parent: DriveEntryRef,
    name: String,
    content: String,
    #[serde(default = "default_media")]
    content_type: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    entry: DriveEntryRef,
    content: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    entry: DriveEntryRef,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RelocateArgs {
    entry: DriveEntryRef,
    parent: DriveEntryRef,
    name: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    target_workdir: String,
    path: String,
    parent: DriveEntryRef,
    name: String,
    #[serde(default = "default_binary_media")]
    content_type: String,
}
fn default_binary_media() -> String {
    "application/octet-stream".into()
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    request_id: String,
}
fn default_media() -> String {
    "text/markdown".into()
}
fn decode<T: DeserializeOwned>(input: &str) -> Result<T, DriveError> {
    if input.len() > 512 * 1024 {
        return Err(DriveError::Limit);
    }
    serde_json::from_str(input)
        .map_err(|_| DriveError::Invalid("input shape or bound rejected".into()))
}
fn text_limits() -> fs_operation::text::TextLimits {
    fs_operation::text::TextLimits {
        max_input_bytes: Some(DRIVE_TEXT_MAX_BYTES),
        max_output_bytes: Some(DRIVE_TEXT_MAX_BYTES),
        max_replacements: None,
    }
}
fn checked_text(text: String) -> Result<String, DriveError> {
    fs_operation::text::write(text, text_limits())
        .map(|out| out.content)
        .map_err(text_error)
}
fn text_error(error: fs_operation::text::TextError) -> DriveError {
    DriveError::Invalid(error.to_string())
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(super) fn entry_path(entry: &DriveEntryRef) -> String {
    format!(
        "/drive/{}/{}",
        crate::feature::builtin::manage_workdir::wip::encode_identity(&entry.workspace_id),
        entry.node_id
    )
}
fn entry_value(entry: DriveEntry) -> Value {
    json!({"path":entry_path(&entry.entry),"metadata":entry})
}
fn page_value(page: DriveListResponse) -> Value {
    json!({"entries":page.entries.into_iter().map(entry_value).collect::<Vec<_>>(),"next_after":page.next_after})
}
fn mutation_value(response: DriveMutationResponse) -> Value {
    json!({"request_id":response.request_id,"entry":response.entry.map(entry_value)})
}

#[derive(Clone, Copy)]
pub(super) struct Spec {
    pub name: &'static str,
    pub operation: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
}
fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("static Drive argument schema")
}
pub(super) const SPECS: &[Spec] = &[
    Spec {
        name: "DriveRoot",
        operation: "root",
        description: "Discover the authorized Workspace Drive root. Profile activation is not a grant; no Workdir is required.",
        schema: schema::<EmptyArgs>,
    },
    Spec {
        name: "DriveMetadata",
        operation: "metadata",
        description: "Inspect one Workspace-bound stable entry ID and observe its revision for mutation.",
        schema: schema::<EntryArgs>,
    },
    Spec {
        name: "DriveList",
        operation: "list",
        description: "List a bounded folder page (default root), up to 200 entries. Results do not expand the WIP tree or observe revisions for writes.",
        schema: schema::<ListArgs>,
    },
    Spec {
        name: "DriveSearch",
        operation: "search",
        description: "Search names and optionally bounded text through Backend paged search; no embedding/index. Use the returned cursor to continue.",
        schema: schema::<DriveSearchQuery>,
    },
    Spec {
        name: "DriveRead",
        operation: "read",
        description: "Read up to 64 KiB UTF-8 text and observe entry revision. Past Session text remains an immutable observation.",
        schema: schema::<ReadArgs>,
    },
    Spec {
        name: "DriveCreateFolder",
        operation: "create_folder",
        description: "Create a folder through the explicit Drive write grant. Same-name conflicts are not overwritten.",
        schema: schema::<FolderArgs>,
    },
    Spec {
        name: "DriveCreateText",
        operation: "create_text",
        description: "Create bounded UTF-8 text (default Markdown), <=64 KiB. Success means DB publication committed.",
        schema: schema::<CreateArgs>,
    },
    Spec {
        name: "DriveWrite",
        operation: "write",
        description: "Replace <=64 KiB text using a previously read/metadata-observed revision. Conflicts are not retried with latest revision.",
        schema: schema::<WriteArgs>,
    },
    Spec {
        name: "DriveEdit",
        operation: "edit",
        description: "Partial text edit using previously observed revision and shared old_string rules. Require nonempty unique match unless replace_all. Truncated preimages cannot be edited.",
        schema: schema::<EditArgs>,
    },
    Spec {
        name: "DriveRelocate",
        operation: "relocate",
        description: "Rename and/or move an observed entry with revision precondition; stable ID and URL remain unchanged.",
        schema: schema::<RelocateArgs>,
    },
    Spec {
        name: "DriveDelete",
        operation: "delete",
        description: "Delete one observed entry with revision precondition. A recreated name has a different ID; stale observations cannot target it.",
        schema: schema::<EntryArgs>,
    },
    Spec {
        name: "DriveViewImage",
        operation: "view_image",
        description: "Attach revision-fixed PNG/JPEG/GIF/WebP bytes (<=10 MiB) through durable ToolOutput history/capture; a URL alone is not image viewing.",
        schema: schema::<EntryArgs>,
    },
    Spec {
        name: "DriveSaveWorkdir",
        operation: "save_workdir",
        description: "Publish a <=16 MiB artifact using selected Workdir alias and logical path plus Drive parent/name. Both source read and destination write are required. No Bash/raw host path/base64 input.",
        schema: schema::<SaveArgs>,
    },
    Spec {
        name: "DriveRequestStatus",
        operation: "request_status",
        description: "Query the same mutation request ID after OutcomeUnknown. Uncommitted snapshot is not proof of failure; never blindly recreate the request.",
        schema: schema::<StatusArgs>,
    },
];
