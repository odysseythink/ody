use super::*;

#[derive(Clone)]
pub(crate) struct AuthRequestProcessor {
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
}

impl AuthRequestProcessor {
    pub(crate) fn new(
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        config_manager: ConfigManager,
    ) -> Self {
        Self {
            thread_manager,
            outgoing,
            config,
            config_manager,
        }
    }

    pub(crate) async fn login(
        &self,
        request_id: ConnectionRequestId,
        params: LoginParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.login_v2(request_id, params).await.map(|()| None)
    }

    pub(crate) async fn logout(
        &self,
        request_id: ConnectionRequestId,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.logout_v2(request_id).await.map(|()| None)
    }

    pub(crate) async fn get_auth_state(
        &self,
        params: GetAuthStateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_auth_state_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_auth_status(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_auth_status_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    fn auth_json_path(&self) -> PathBuf {
        self.config.ody_home.join("auth.json").to_path_buf()
    }

    fn write_auth_json(
        &self,
        auth_mode: &str,
        api_key: Option<&str>,
    ) -> std::result::Result<(), JSONRPCErrorError> {
        let path = self.auth_json_path();
        let value = serde_json::json!({
            "auth_mode": auth_mode,
            "odysseythink_api_key": api_key,
        });
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&value).unwrap_or_default(),
        )
        .map_err(|err| internal_error(format!("failed to write auth.json: {err}")))?;
        Ok(())
    }

    fn delete_auth_json(&self) -> std::result::Result<(), JSONRPCErrorError> {
        let path = self.auth_json_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(internal_error(format!("failed to delete auth.json: {err}"))),
        }
    }

    fn read_auth_json(&self) -> std::result::Result<Option<(AuthMode, String)>, JSONRPCErrorError> {
        let path = self.auth_json_path();
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(internal_error(format!("failed to read auth.json: {err}"))),
        };
        let value: serde_json::Value = serde_json::from_str(&contents)
            .map_err(|err| internal_error(format!("invalid auth.json: {err}")))?;
        let auth_mode = value
            .get("auth_mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let api_key = value
            .get("odysseythink_api_key")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        match (auth_mode, api_key) {
            ("api_key", Some(api_key)) => Ok(Some((AuthMode::ApiKey, api_key))),
            _ => Ok(None),
        }
    }

    fn current_auth_mode(&self) -> std::result::Result<Option<AuthMode>, JSONRPCErrorError> {
        self.read_auth_json().map(|opt| opt.map(|(mode, _)| mode))
    }

    fn current_auth_updated_notification(&self) -> AuthUpdatedNotification {
        AuthUpdatedNotification {
            auth_mode: self.current_auth_mode().unwrap_or(None),
        }
    }

    // Named `maybe_refresh_plugin_caches_for_current_config` for historical reasons: it used to
    // kick off an async refresh of the remote hosted plugin catalog cache after an auth change,
    // and only cleared local plugin/skill caches once that remote refresh completed. The remote
    // catalog has been removed, so this now just applies the new auth mode and eagerly clears
    // local caches so plugin/skill state re-resolves for the new auth.
    async fn maybe_refresh_plugin_caches_for_current_config(
        config_manager: &ConfigManager,
        thread_manager: &Arc<ThreadManager>,
        auth_mode: Option<AuthMode>,
    ) {
        thread_manager.plugins_manager().set_auth_mode(auth_mode);

        Self::spawn_effective_plugins_changed_task(
            Arc::clone(thread_manager),
            config_manager.clone(),
        );
    }

    fn spawn_effective_plugins_changed_task(
        thread_manager: Arc<ThreadManager>,
        config_manager: ConfigManager,
    ) {
        tokio::spawn(async move {
            thread_manager.plugins_manager().clear_cache();
            thread_manager.skills_service().clear_cache();
            if thread_manager.list_thread_ids().await.is_empty() {
                return;
            }
            crate::mcp_refresh::queue_best_effort_refresh(&thread_manager, &config_manager).await;
        });
    }

    async fn login_v2(
        &self,
        request_id: ConnectionRequestId,
        params: LoginParams,
    ) -> Result<(), JSONRPCErrorError> {
        match params {
            LoginParams::ApiKey { api_key } => {
                self.login_api_key_v2(request_id, LoginApiKeyParams { api_key })
                    .await;
            }
        }
        Ok(())
    }

    async fn login_api_key_v2(&self, request_id: ConnectionRequestId, params: LoginApiKeyParams) {
        let result: std::result::Result<LoginResponse, ody_app_server_protocol::JSONRPCErrorError> =
            self.write_auth_json("api_key", Some(&params.api_key))
                .map(|_| LoginResponse::ApiKey {});
        let logged_in = result.is_ok();
        self.outgoing.send_result(request_id, result).await;

        if logged_in {
            self.send_login_success_notifications().await;
        }
    }

    async fn send_login_success_notifications(&self) {
        let auth_mode = self.current_auth_mode().unwrap_or(None);
        Self::maybe_refresh_plugin_caches_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            auth_mode,
        )
        .await;

        let payload_login_completed = LoginCompletedNotification {
            login_id: None,
            success: true,
            error: None,
        };
        self.outgoing
            .send_server_notification(ServerNotification::LoginCompleted(payload_login_completed))
            .await;

        self.outgoing
            .send_server_notification(ServerNotification::AuthUpdated(
                self.current_auth_updated_notification(),
            ))
            .await;
    }

    async fn logout_common(&self) -> std::result::Result<Option<AuthMode>, JSONRPCErrorError> {
        self.delete_auth_json()?;
        Ok(None)
    }

    async fn logout_v2(&self, request_id: ConnectionRequestId) -> Result<(), JSONRPCErrorError> {
        let result = self.logout_common().await;
        let auth_updated = result
            .as_ref()
            .ok()
            .cloned()
            .map(|auth_mode| AuthUpdatedNotification { auth_mode });
        self.outgoing
            .send_result(request_id, result.map(|_| LogoutResponse {}))
            .await;

        if let Some(payload) = auth_updated {
            Self::maybe_refresh_plugin_caches_for_current_config(
                &self.config_manager,
                &self.thread_manager,
                payload.auth_mode,
            )
            .await;
            self.outgoing
                .send_server_notification(ServerNotification::AuthUpdated(payload))
                .await;
        }
        Ok(())
    }

    async fn get_auth_status_response(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<GetAuthStatusResponse, JSONRPCErrorError> {
        let (auth_method, auth_token) = match self.read_auth_json()? {
            Some((AuthMode::ApiKey, api_key)) => {
                let token = if params.include_token == Some(true) {
                    Some(api_key)
                } else {
                    None
                };
                (Some(AuthMode::ApiKey), token)
            }
            _ => (None, None),
        };
        Ok(GetAuthStatusResponse {
            auth_method,
            auth_token,
        })
    }

    async fn get_auth_state_response(
        &self,
        _params: GetAuthStateParams,
    ) -> Result<GetAuthStateResponse, JSONRPCErrorError> {
        let auth_state = match self.read_auth_json()? {
            Some((AuthMode::ApiKey, _)) => Some(AuthState::ApiKey {}),
            _ => None,
        };

        Ok(GetAuthStateResponse { auth_state })
    }
}
