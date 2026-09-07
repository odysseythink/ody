use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::ServerNotification;
use ody_app_server_protocol::VISUAL_WORKSPACE_PROTOCOL_VERSION;
use ody_app_server_protocol::VisualArtifact;
use ody_app_server_protocol::VisualArtifactCreateParams;
use ody_app_server_protocol::VisualArtifactCreateResponse;
use ody_app_server_protocol::VisualArtifactListParams;
use ody_app_server_protocol::VisualArtifactListResponse;
use ody_app_server_protocol::VisualPatchApplyParams;
use ody_app_server_protocol::VisualPatchApplyResponse;
use ody_app_server_protocol::VisualPatchOperation;
use ody_app_server_protocol::VisualPreviewOpenParams;
use ody_app_server_protocol::VisualPreviewOpenResponse;
use ody_app_server_protocol::VisualProject;
use ody_app_server_protocol::VisualProjectListParams;
use ody_app_server_protocol::VisualProjectListResponse;
use ody_app_server_protocol::VisualProjectUpsertParams;
use ody_app_server_protocol::VisualProjectUpsertResponse;
use ody_app_server_protocol::VisualSnapshot;
use ody_app_server_protocol::VisualSnapshotCreateParams;
use ody_app_server_protocol::VisualSnapshotCreateResponse;
use ody_app_server_protocol::VisualWorkspaceChangedNotification;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::outgoing_message::OutgoingMessageSender;

const MAX_HTML_BYTES: usize = 1_048_576;
const MAX_SNAPSHOT_BYTES: usize = 12 * 1024 * 1024;
const STORE_DIR: &str = "visual-workspace";
const STORE_FILE: &str = "v1.json";

#[derive(Clone)]
pub(crate) struct VisualWorkspaceRequestProcessor {
    store: Arc<Mutex<VisualWorkspaceStore>>,
    outgoing: Arc<OutgoingMessageSender>,
}

#[derive(Default, Serialize, Deserialize)]
struct VisualWorkspaceStore {
    schema_version: u32,
    projects: BTreeMap<String, VisualProject>,
    artifacts: BTreeMap<String, VisualArtifact>,
    snapshots: BTreeMap<String, VisualSnapshot>,
    idempotency: BTreeMap<String, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl VisualWorkspaceRequestProcessor {
    pub(crate) fn new(ody_home: PathBuf, outgoing: Arc<OutgoingMessageSender>) -> Self {
        let path = ody_home.join(STORE_DIR).join(STORE_FILE);
        let store = VisualWorkspaceStore::load(path);
        Self {
            store: Arc::new(Mutex::new(store)),
            outgoing,
        }
    }

    pub(crate) async fn project_upsert(
        &self,
        params: VisualProjectUpsertParams,
    ) -> Result<VisualProjectUpsertResponse, JSONRPCErrorError> {
        let response = self.with_store(|store| store.project_upsert(params))?;
        self.notify(&response.project.id, None, "project").await;
        Ok(response)
    }

    pub(crate) async fn project_list(
        &self,
        _params: VisualProjectListParams,
    ) -> Result<VisualProjectListResponse, JSONRPCErrorError> {
        self.with_store(|store| Ok(store.project_list()))
    }

    pub(crate) async fn artifact_create(
        &self,
        params: VisualArtifactCreateParams,
    ) -> Result<VisualArtifactCreateResponse, JSONRPCErrorError> {
        let response = self.with_store(|store| store.artifact_create(params))?;
        self.notify(
            &response.artifact.project_id,
            Some(response.artifact.id.clone()),
            "artifact",
        )
        .await;
        Ok(response)
    }

    pub(crate) async fn artifact_list(
        &self,
        params: VisualArtifactListParams,
    ) -> Result<VisualArtifactListResponse, JSONRPCErrorError> {
        self.with_store(|store| Ok(store.artifact_list(params)))
    }

    pub(crate) async fn patch_apply(
        &self,
        params: VisualPatchApplyParams,
    ) -> Result<VisualPatchApplyResponse, JSONRPCErrorError> {
        let response = self.with_store(|store| store.patch_apply(params))?;
        self.notify(
            &response.artifact.project_id,
            Some(response.artifact.id.clone()),
            "patch",
        )
        .await;
        Ok(response)
    }

    pub(crate) async fn snapshot_create(
        &self,
        params: VisualSnapshotCreateParams,
    ) -> Result<VisualSnapshotCreateResponse, JSONRPCErrorError> {
        let response = self.with_store(|store| store.snapshot_create(params))?;
        self.notify(
            &response.snapshot.project_id,
            Some(response.snapshot.artifact_id.clone()),
            "snapshot",
        )
        .await;
        Ok(response)
    }

    pub(crate) async fn preview_open(
        &self,
        params: VisualPreviewOpenParams,
    ) -> Result<VisualPreviewOpenResponse, JSONRPCErrorError> {
        self.with_store(|store| store.preview_open(params))
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut VisualWorkspaceStore) -> Result<T, JSONRPCErrorError>,
    ) -> Result<T, JSONRPCErrorError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| internal_error("visual workspace store lock poisoned"))?;
        operation(&mut store)
    }

    async fn notify(&self, project_id: &str, artifact_id: Option<String>, kind: &str) {
        self.outgoing
            .send_server_notification(ServerNotification::VisualWorkspaceChanged(
                VisualWorkspaceChangedNotification {
                    project_id: project_id.to_owned(),
                    artifact_id,
                    kind: kind.to_owned(),
                    updated_at_ms: now_ms(),
                },
            ))
            .await;
    }
}

impl VisualWorkspaceStore {
    fn load(path: PathBuf) -> Self {
        let backup = path.with_extension("json.bak");
        for candidate in [&path, &backup] {
            if let Ok(raw) = fs::read(candidate)
                && let Ok(mut state) = serde_json::from_slice::<Self>(&raw)
                && state.schema_version == VISUAL_WORKSPACE_PROTOCOL_VERSION
            {
                state.path = path.clone();
                return state;
            }
        }
        Self {
            schema_version: VISUAL_WORKSPACE_PROTOCOL_VERSION,
            path,
            ..Default::default()
        }
    }

    fn project_upsert(
        &mut self,
        params: VisualProjectUpsertParams,
    ) -> Result<VisualProjectUpsertResponse, JSONRPCErrorError> {
        validate_id(&params.id, "project id")?;
        validate_id(&params.idempotency_key, "idempotency key")?;
        if params.name.trim().is_empty() || params.name.len() > 200 {
            return Err(invalid_params(
                "visual project name must be 1..=200 characters",
            ));
        }
        let idempotency_key = idempotency_key("project", &params.idempotency_key);
        if let Some(existing_id) = self.idempotency.get(&idempotency_key) {
            let project = self.projects.get(existing_id).cloned().ok_or_else(|| {
                internal_error("visual workspace idempotency record references a missing project")
            })?;
            return Ok(VisualProjectUpsertResponse { project });
        }
        let now = now_ms();
        let project = self
            .projects
            .entry(params.id.clone())
            .or_insert_with(|| VisualProject {
                id: params.id.clone(),
                name: params.name.clone(),
                schema_version: VISUAL_WORKSPACE_PROTOCOL_VERSION,
                created_at_ms: now,
                updated_at_ms: now,
            });
        project.name = params.name;
        project.updated_at_ms = now;
        let project = project.clone();
        self.idempotency.insert(idempotency_key, project.id.clone());
        self.persist()?;
        Ok(VisualProjectUpsertResponse { project })
    }

    fn project_list(&self) -> VisualProjectListResponse {
        let mut projects = self.projects.values().cloned().collect::<Vec<_>>();
        projects.sort_by_key(|project| std::cmp::Reverse(project.updated_at_ms));
        VisualProjectListResponse { projects }
    }

    fn artifact_create(
        &mut self,
        params: VisualArtifactCreateParams,
    ) -> Result<VisualArtifactCreateResponse, JSONRPCErrorError> {
        validate_id(&params.id, "artifact id")?;
        validate_id(&params.project_id, "project id")?;
        validate_id(&params.idempotency_key, "idempotency key")?;
        if !self.projects.contains_key(&params.project_id) {
            return Err(invalid_params("visual project does not exist"));
        }
        if params.html.len() > MAX_HTML_BYTES || !params.html.to_ascii_lowercase().contains("<html")
        {
            return Err(invalid_params(
                "visual artifact must be a complete HTML document no larger than 1 MiB",
            ));
        }
        if params.name.trim().is_empty() || params.name.len() > 200 {
            return Err(invalid_params(
                "visual artifact name must be 1..=200 characters",
            ));
        }
        let idempotency_key = idempotency_key("artifact", &params.idempotency_key);
        if let Some(existing_id) = self.idempotency.get(&idempotency_key) {
            let artifact = self.artifacts.get(existing_id).cloned().ok_or_else(|| {
                internal_error("visual workspace idempotency record references a missing artifact")
            })?;
            return Ok(VisualArtifactCreateResponse { artifact });
        }
        if self.artifacts.contains_key(&params.id) {
            return Err(invalid_params("visual artifact id already exists"));
        }
        let lineage_id = params.lineage_id.unwrap_or_else(|| params.id.clone());
        validate_id(&lineage_id, "lineage id")?;
        let revision = self
            .artifacts
            .values()
            .filter(|artifact| {
                artifact.project_id == params.project_id && artifact.lineage_id == lineage_id
            })
            .map(|artifact| artifact.revision)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        if let Some(parent_id) = params.parent_artifact_id.as_deref() {
            let parent = self
                .artifacts
                .get(parent_id)
                .ok_or_else(|| invalid_params("parent artifact does not exist"))?;
            if parent.project_id != params.project_id || parent.lineage_id != lineage_id {
                return Err(invalid_params(
                    "parent artifact must belong to the same visual project and lineage",
                ));
            }
        }
        let now = now_ms();
        let artifact = VisualArtifact {
            id: params.id,
            project_id: params.project_id,
            lineage_id,
            revision,
            parent_artifact_id: params.parent_artifact_id,
            name: params.name,
            html: params.html,
            revision_reason: params.revision_reason,
            patch_set: None,
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.idempotency
            .insert(idempotency_key, artifact.id.clone());
        self.artifacts.insert(artifact.id.clone(), artifact.clone());
        self.persist()?;
        Ok(VisualArtifactCreateResponse { artifact })
    }

    fn artifact_list(&self, params: VisualArtifactListParams) -> VisualArtifactListResponse {
        let mut artifacts = self
            .artifacts
            .values()
            .filter(|artifact| artifact.project_id == params.project_id)
            .filter(|artifact| {
                params
                    .lineage_id
                    .as_ref()
                    .is_none_or(|lineage| artifact.lineage_id == *lineage)
            })
            .cloned()
            .collect::<Vec<_>>();
        artifacts.sort_by_key(|artifact| (artifact.lineage_id.clone(), artifact.revision));
        VisualArtifactListResponse { artifacts }
    }

    fn patch_apply(
        &mut self,
        params: VisualPatchApplyParams,
    ) -> Result<VisualPatchApplyResponse, JSONRPCErrorError> {
        validate_id(&params.project_id, "project id")?;
        validate_id(&params.idempotency_key, "idempotency key")?;
        let idempotency_key = idempotency_key("patch", &params.idempotency_key);
        if let Some(existing_id) = self.idempotency.get(&idempotency_key) {
            let artifact = self.artifacts.get(existing_id).cloned().ok_or_else(|| {
                internal_error(
                    "visual workspace idempotency record references a missing patched artifact",
                )
            })?;
            return Ok(VisualPatchApplyResponse { artifact });
        }
        let patch = params.patch_set;
        validate_id(&patch.id, "patch set id")?;
        validate_id(&patch.result_artifact_id, "result artifact id")?;
        let base = self
            .artifacts
            .get(&patch.base_artifact_id)
            .cloned()
            .ok_or_else(|| invalid_params("base artifact does not exist"))?;
        if base.project_id != params.project_id {
            return Err(invalid_params(
                "patch base artifact belongs to another project",
            ));
        }
        if self.artifacts.contains_key(&patch.result_artifact_id) {
            return Err(invalid_params("patch result artifact id already exists"));
        }
        if patch.operations.is_empty() {
            return Err(invalid_params("patch set requires at least one operation"));
        }
        let mut html = base.html.clone();
        for operation in &patch.operations {
            match operation {
                VisualPatchOperation::SetStyle {
                    element_id,
                    declarations,
                } => {
                    validate_id(element_id, "element id")?;
                    if declarations.is_empty() {
                        return Err(invalid_params("setStyle requires at least one declaration"));
                    }
                    html = apply_style_patch(&html, element_id, declarations)?;
                }
            }
        }
        let revision = self
            .artifacts
            .values()
            .filter(|artifact| {
                artifact.project_id == params.project_id && artifact.lineage_id == base.lineage_id
            })
            .map(|artifact| artifact.revision)
            .max()
            .unwrap_or(base.revision)
            .saturating_add(1);
        let now = now_ms();
        let artifact = VisualArtifact {
            id: patch.result_artifact_id.clone(),
            project_id: params.project_id,
            lineage_id: base.lineage_id,
            revision,
            parent_artifact_id: Some(base.id),
            name: base.name,
            html,
            revision_reason: ody_app_server_protocol::VisualRevisionReason::StylePatch,
            patch_set: Some(patch),
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.idempotency
            .insert(idempotency_key, artifact.id.clone());
        self.artifacts.insert(artifact.id.clone(), artifact.clone());
        self.persist()?;
        Ok(VisualPatchApplyResponse { artifact })
    }

    fn snapshot_create(
        &mut self,
        params: VisualSnapshotCreateParams,
    ) -> Result<VisualSnapshotCreateResponse, JSONRPCErrorError> {
        validate_id(&params.id, "snapshot id")?;
        validate_id(&params.idempotency_key, "idempotency key")?;
        let idempotency_key = idempotency_key("snapshot", &params.idempotency_key);
        if let Some(existing_id) = self.idempotency.get(&idempotency_key) {
            let snapshot = self.snapshots.get(existing_id).cloned().ok_or_else(|| {
                internal_error("visual workspace idempotency record references a missing snapshot")
            })?;
            return Ok(VisualSnapshotCreateResponse { snapshot });
        }
        if self.snapshots.contains_key(&params.id) {
            return Err(invalid_params("visual snapshot id already exists"));
        }
        let artifact = self
            .artifacts
            .get(&params.artifact_id)
            .ok_or_else(|| invalid_params("snapshot artifact does not exist"))?;
        if artifact.project_id != params.project_id {
            return Err(invalid_params(
                "snapshot artifact belongs to another project",
            ));
        }
        let bytes = STANDARD
            .decode(&params.data_base64)
            .map_err(|_| invalid_params("snapshot data must be valid base64"))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES
            || params.width == 0
            || params.height == 0
            || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err(invalid_params(
                "snapshot exceeds 12 MiB or has invalid dimensions",
            ));
        }
        let snapshot = VisualSnapshot {
            id: params.id,
            project_id: params.project_id,
            artifact_id: params.artifact_id,
            viewport: params.viewport,
            width: params.width,
            height: params.height,
            data_base64: params.data_base64,
            created_at_ms: now_ms(),
        };
        self.idempotency
            .insert(idempotency_key, snapshot.id.clone());
        self.snapshots.insert(snapshot.id.clone(), snapshot.clone());
        self.persist()?;
        Ok(VisualSnapshotCreateResponse { snapshot })
    }

    fn preview_open(
        &self,
        params: VisualPreviewOpenParams,
    ) -> Result<VisualPreviewOpenResponse, JSONRPCErrorError> {
        let artifact = self
            .artifacts
            .get(&params.artifact_id)
            .cloned()
            .ok_or_else(|| invalid_params("visual artifact does not exist"))?;
        if artifact.project_id != params.project_id {
            return Err(invalid_params("visual artifact belongs to another project"));
        }
        Ok(VisualPreviewOpenResponse {
            preview_session_id: Uuid::now_v7().to_string(),
            artifact,
        })
    }

    fn persist(&self) -> Result<(), JSONRPCErrorError> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| internal_error("visual workspace store path has no parent"))?;
        fs::create_dir_all(directory).map_err(|error| {
            internal_error(format!("failed to create visual workspace store: {error}"))
        })?;
        let serialized = serde_json::to_vec_pretty(self).map_err(|error| {
            internal_error(format!(
                "failed to serialize visual workspace store: {error}"
            ))
        })?;
        let temporary = self
            .path
            .with_extension(format!("json.{}.tmp", Uuid::now_v7()));
        fs::write(&temporary, serialized).map_err(|error| {
            internal_error(format!("failed to write visual workspace store: {error}"))
        })?;
        if self.path.exists() {
            let _ = fs::copy(&self.path, self.path.with_extension("json.bak"));
        }
        fs::rename(&temporary, &self.path).map_err(|error| {
            internal_error(format!("failed to commit visual workspace store: {error}"))
        })
    }
}

fn apply_style_patch(
    html: &str,
    element_id: &str,
    declarations: &[ody_app_server_protocol::VisualStyleDeclaration],
) -> Result<String, JSONRPCErrorError> {
    let needle = format!("data-ody-id=\"{element_id}\"");
    let attribute_at = html
        .find(&needle)
        .ok_or_else(|| invalid_params("patch element no longer exists in artifact source"))?;
    let tag_start = html[..attribute_at]
        .rfind('<')
        .ok_or_else(|| invalid_params("patch element has invalid HTML source"))?;
    let tag_end = html[attribute_at..]
        .find('>')
        .map(|offset| attribute_at + offset)
        .ok_or_else(|| invalid_params("patch element has invalid HTML source"))?;
    let tag = &html[tag_start..=tag_end];
    let mut rendered = declarations
        .iter()
        .map(|declaration| {
            if !declaration.property.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            }) || declaration.property.is_empty()
                || declaration.value.is_empty()
                || declaration.value.len() > 500
                || declaration.value.to_ascii_lowercase().contains("url(")
            {
                Err(invalid_params("patch contains an unsafe CSS declaration"))
            } else {
                Ok(format!(
                    "{}: {};",
                    declaration.property,
                    declaration.value.trim()
                ))
            }
        })
        .collect::<Result<String, _>>()?;
    if let Some(style_at) = tag.find("style=\"") {
        let value_start = tag_start + style_at + "style=\"".len();
        let value_end = html[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .ok_or_else(|| invalid_params("patch element has an unterminated style attribute"))?;
        let mut output = html.to_string();
        output.insert_str(value_end, &format!(" {rendered}"));
        Ok(output)
    } else {
        rendered.insert(0, ' ');
        let mut output = html.to_string();
        output.insert_str(tag_end, &format!("style=\"{rendered}\""));
        Ok(output)
    }
}

fn validate_id(value: &str, label: &str) -> Result<(), JSONRPCErrorError> {
    if value.is_empty()
        || value.len() > 200
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(invalid_params(format!(
            "{label} must be 1..=200 URL-safe characters"
        )));
    }
    Ok(())
}

fn idempotency_key(kind: &str, key: &str) -> String {
    format!("{kind}:{key}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_app_server_protocol::VisualPatchSet;
    use ody_app_server_protocol::VisualRevisionReason;
    use ody_app_server_protocol::VisualStyleDeclaration;
    use ody_app_server_protocol::VisualViewport;

    fn test_store() -> (tempfile::TempDir, VisualWorkspaceStore) {
        let directory = tempfile::tempdir().expect("create temporary visual workspace directory");
        let store = VisualWorkspaceStore::load(directory.path().join(STORE_FILE));
        (directory, store)
    }

    fn create_project(store: &mut VisualWorkspaceStore) {
        store
            .project_upsert(VisualProjectUpsertParams {
                id: "project-1".to_owned(),
                name: "Checkout".to_owned(),
                idempotency_key: "retry-key".to_owned(),
            })
            .expect("create project");
    }

    #[test]
    fn persists_a_revision_chain_and_keeps_resource_idempotency_isolated() {
        let (directory, mut store) = test_store();
        create_project(&mut store);

        let base = store
            .artifact_create(VisualArtifactCreateParams {
                id: "artifact-1".to_owned(),
                project_id: "project-1".to_owned(),
                lineage_id: None,
                parent_artifact_id: None,
                name: "Checkout page".to_owned(),
                html: "<html><body><button data-ody-id=\"buy\">Buy</button></body></html>"
                    .to_owned(),
                revision_reason: VisualRevisionReason::Generate,
                idempotency_key: "retry-key".to_owned(),
            })
            .expect("create base artifact")
            .artifact;

        let patched = store
            .patch_apply(VisualPatchApplyParams {
                project_id: "project-1".to_owned(),
                patch_set: VisualPatchSet {
                    id: "patch-1".to_owned(),
                    base_artifact_id: base.id.clone(),
                    result_artifact_id: "artifact-2".to_owned(),
                    operations: vec![VisualPatchOperation::SetStyle {
                        element_id: "buy".to_owned(),
                        declarations: vec![VisualStyleDeclaration {
                            property: "background-color".to_owned(),
                            value: "#2d6cdf".to_owned(),
                        }],
                    }],
                    created_at_ms: 1,
                },
                idempotency_key: "retry-key".to_owned(),
            })
            .expect("apply patch")
            .artifact;

        assert_eq!(base.revision, 1);
        assert_eq!(patched.revision, 2);
        assert!(patched.html.contains("background-color: #2d6cdf;"));
        assert_eq!(
            store
                .artifact_list(VisualArtifactListParams {
                    project_id: "project-1".to_owned(),
                    lineage_id: Some(base.lineage_id.clone()),
                })
                .artifacts
                .len(),
            2
        );

        let png = STANDARD.encode(b"\x89PNG\r\n\x1a\n");
        let snapshot = store
            .snapshot_create(VisualSnapshotCreateParams {
                id: "snapshot-1".to_owned(),
                project_id: "project-1".to_owned(),
                artifact_id: patched.id.clone(),
                viewport: VisualViewport::Desktop,
                width: 1280,
                height: 720,
                data_base64: png,
                idempotency_key: "retry-key".to_owned(),
            })
            .expect("create snapshot")
            .snapshot;
        assert_eq!(snapshot.id, "snapshot-1");

        let restored = VisualWorkspaceStore::load(directory.path().join(STORE_FILE));
        assert_eq!(restored.projects.len(), 1);
        assert_eq!(restored.artifacts.len(), 2);
        assert_eq!(restored.snapshots.len(), 1);
    }

    #[test]
    fn rejects_unsafe_style_values_and_non_png_snapshots() {
        let (_directory, mut store) = test_store();
        create_project(&mut store);
        let base = store
            .artifact_create(VisualArtifactCreateParams {
                id: "artifact-1".to_owned(),
                project_id: "project-1".to_owned(),
                lineage_id: None,
                parent_artifact_id: None,
                name: "Page".to_owned(),
                html: "<html><body><main data-ody-id=\"root\"></main></body></html>".to_owned(),
                revision_reason: VisualRevisionReason::Generate,
                idempotency_key: "artifact-key".to_owned(),
            })
            .expect("create base artifact")
            .artifact;

        let unsafe_patch = store.patch_apply(VisualPatchApplyParams {
            project_id: "project-1".to_owned(),
            patch_set: VisualPatchSet {
                id: "patch-1".to_owned(),
                base_artifact_id: base.id.clone(),
                result_artifact_id: "artifact-2".to_owned(),
                operations: vec![VisualPatchOperation::SetStyle {
                    element_id: "root".to_owned(),
                    declarations: vec![VisualStyleDeclaration {
                        property: "background".to_owned(),
                        value: "url(https://example.invalid/track)".to_owned(),
                    }],
                }],
                created_at_ms: 1,
            },
            idempotency_key: "patch-key".to_owned(),
        });
        assert!(unsafe_patch.is_err());

        let invalid_snapshot = store.snapshot_create(VisualSnapshotCreateParams {
            id: "snapshot-1".to_owned(),
            project_id: "project-1".to_owned(),
            artifact_id: base.id,
            viewport: VisualViewport::Mobile,
            width: 390,
            height: 844,
            data_base64: STANDARD.encode(b"not a png"),
            idempotency_key: "snapshot-key".to_owned(),
        });
        assert!(invalid_snapshot.is_err());
    }
}
