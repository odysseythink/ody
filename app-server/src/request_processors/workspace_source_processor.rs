use std::sync::Arc;
use std::sync::Mutex;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceSourceIndexParams;
use ody_app_server_protocol::WorkspaceSourceIndexResponse;
use ody_app_server_protocol::WorkspaceSourceQueryKind;
use ody_app_server_protocol::WorkspaceSourceResolveMatch;
use ody_app_server_protocol::WorkspaceSourceResolveParams;
use ody_app_server_protocol::WorkspaceSourceResolveResponse;

use crate::error_code::invalid_params;
use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;

const RESOLVE_DEFAULT_LIMIT: usize = 20;
const RESOLVE_MAX_LIMIT: usize = 100;

#[derive(Clone)]
pub(crate) struct WorkspaceSourceRequestProcessor {
    project_store: Arc<Mutex<WorkspaceProjectStore>>,
}

impl WorkspaceSourceRequestProcessor {
    pub(crate) fn new(project_store: Arc<Mutex<WorkspaceProjectStore>>) -> Self {
        Self { project_store }
    }

    pub(crate) async fn index(
        &self,
        params: WorkspaceSourceIndexParams,
    ) -> Result<WorkspaceSourceIndexResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let index = crate::workspace_source_index::index_project(&project).await;
        Ok(WorkspaceSourceIndexResponse { index })
    }

    pub(crate) async fn resolve(
        &self,
        params: WorkspaceSourceResolveParams,
    ) -> Result<WorkspaceSourceResolveResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let value = params.value.trim();
        if value.is_empty() {
            return Err(invalid_params("resolve value must not be empty"));
        }
        let limit = params
            .limit
            .map(|v| v as usize)
            .unwrap_or(RESOLVE_DEFAULT_LIMIT)
            .clamp(1, RESOLVE_MAX_LIMIT);
        let index = crate::workspace_source_index::index_project(&project).await;
        let needle = value.to_lowercase();
        let mut matches = Vec::new();
        for (artifact, source_ref) in index.artifacts.iter().zip(index.refs.iter()) {
            let hit = match params.kind {
                WorkspaceSourceQueryKind::Name => artifact.name.to_lowercase() == needle,
                WorkspaceSourceQueryKind::Symbol => source_ref.symbol.to_lowercase() == needle,
                WorkspaceSourceQueryKind::RoutePath => {
                    artifact.route_path.as_deref() == Some(value)
                        || source_ref.route_path.as_deref() == Some(value)
                }
            };
            if hit {
                matches.push(WorkspaceSourceResolveMatch {
                    artifact: artifact.clone(),
                    source_ref: source_ref.clone(),
                });
                if matches.len() >= limit {
                    break;
                }
            }
        }
        Ok(WorkspaceSourceResolveResponse { matches })
    }

    fn project(&self, project_id: &str) -> Result<WorkspaceProjectRef, JSONRPCErrorError> {
        let store = self.project_store.lock().map_err(|_| {
            crate::error_code::internal_error("workspace project store lock poisoned")
        })?;
        store
            .projects
            .get(project_id)
            .cloned()
            .ok_or_else(|| unknown_project(project_id))
    }
}

fn unknown_project(id: &str) -> JSONRPCErrorError {
    invalid_params(format!("unknown workspace project id: {id}"))
}
