use super::*;
use crate::error_code::internal_error;

#[derive(Clone)]
pub(crate) struct AccountRequestProcessor {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
}

impl AccountRequestProcessor {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        config_manager: ConfigManager,
    ) -> Self {
        Self {
            auth_manager,
            thread_manager,
            outgoing,
            config,
            config_manager,
        }
    }

    pub(crate) async fn login_account(
        &self,
        request_id: ConnectionRequestId,
        params: LoginAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.login_v2(request_id, params).await.map(|()| None)
    }

    pub(crate) async fn logout_account(
        &self,
        request_id: ConnectionRequestId,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.logout_v2(request_id).await.map(|()| None)
    }

    pub(crate) async fn get_account(
        &self,
        params: GetAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_response(params)
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

    pub(crate) async fn get_account_rate_limits(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Err(internal_error(
            "account/rateLimits/read is not supported in this build",
        ))
    }

    pub(crate) async fn get_account_token_usage(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Err(internal_error(
            "account/usage/read is not supported in this build",
        ))
    }

    pub(crate) async fn get_workspace_messages(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Err(internal_error(
            "account/workspaceMessages/read is not supported in this build",
        ))
    }

    pub(crate) async fn send_add_credits_nudge_email(
        &self,
        _params: SendAddCreditsNudgeEmailParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Err(internal_error(
            "account/sendAddCreditsNudgeEmail is not supported in this build",
        ))
    }

    pub(crate) async fn consume_account_rate_limit_reset_credit(
        &self,
        _params: ConsumeAccountRateLimitResetCreditParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Err(internal_error(
            "account/rateLimitResetCredit/consume is not supported in this build",
        ))
    }

    fn current_account_updated_notification(&self) -> AccountUpdatedNotification {
        let auth = self.auth_manager.auth_cached();
        AccountUpdatedNotification {
            auth_mode: auth.as_ref().map(OdyAuth::api_auth_mode),
            plan_type: None,
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
        auth: Option<OdyAuth>,
    ) {
        thread_manager
            .plugins_manager()
            .set_auth_mode(auth.as_ref().map(OdyAuth::api_auth_mode));

        Self::spawn_effective_plugins_changed_task(Arc::clone(thread_manager), config_manager.clone());
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
        params: LoginAccountParams,
    ) -> Result<(), JSONRPCErrorError> {
        match params {
            LoginAccountParams::ApiKey { api_key } => {
                self.login_api_key_v2(request_id, LoginApiKeyParams { api_key })
                    .await;
            }
        }
        Ok(())
    }

    async fn login_api_key_common(
        &self,
        params: &LoginApiKeyParams,
    ) -> std::result::Result<(), JSONRPCErrorError> {
        match login_with_api_key(
            &self.config.ody_home,
            &params.api_key,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        ) {
            Ok(()) => {
                self.auth_manager.reload().await;
                Ok(())
            }
            Err(err) => Err(internal_error(format!("failed to save api key: {err}"))),
        }
    }

    async fn login_api_key_v2(&self, request_id: ConnectionRequestId, params: LoginApiKeyParams) {
        let result = self
            .login_api_key_common(&params)
            .await
            .map(|()| LoginAccountResponse::ApiKey {});
        let logged_in = result.is_ok();
        self.outgoing.send_result(request_id, result).await;

        if logged_in {
            self.send_login_success_notifications().await;
        }
    }

    async fn send_login_success_notifications(&self) {
        Self::maybe_refresh_plugin_caches_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        let payload_login_completed = AccountLoginCompletedNotification {
            login_id: None,
            success: true,
            error: None,
        };
        self.outgoing
            .send_server_notification(ServerNotification::AccountLoginCompleted(
                payload_login_completed,
            ))
            .await;

        self.outgoing
            .send_server_notification(ServerNotification::AccountUpdated(
                self.current_account_updated_notification(),
            ))
            .await;
    }

    async fn logout_common(&self) -> std::result::Result<Option<AuthMode>, JSONRPCErrorError> {
        match self.auth_manager.logout().await {
            Ok(_) => {}
            Err(err) => {
                return Err(internal_error(format!("logout failed: {err}")));
            }
        }

        Self::maybe_refresh_plugin_caches_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        // Reflect the current auth method after logout (likely None).
        Ok(self
            .auth_manager
            .auth_cached()
            .as_ref()
            .map(OdyAuth::api_auth_mode))
    }

    async fn logout_v2(&self, request_id: ConnectionRequestId) -> Result<(), JSONRPCErrorError> {
        let result = self.logout_common().await;
        let account_updated =
            result
                .as_ref()
                .ok()
                .cloned()
                .map(|auth_mode| AccountUpdatedNotification {
                    auth_mode,
                    plan_type: None,
                });
        self.outgoing
            .send_result(request_id, result.map(|_| LogoutAccountResponse {}))
            .await;

        if let Some(payload) = account_updated {
            self.outgoing
                .send_server_notification(ServerNotification::AccountUpdated(payload))
                .await;
        }
        Ok(())
    }

    async fn get_auth_status_response(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<GetAuthStatusResponse, JSONRPCErrorError> {
        let include_token = params.include_token.unwrap_or(false);

        // Determine whether auth is required based on the active model provider.
        // If a custom provider is configured with `requires_odysseythink_auth == false`,
        // then no auth step is required; otherwise, default to requiring auth.
        let requires_odysseythink_auth = self.config.model_provider.requires_odysseythink_auth;

        let response = if !requires_odysseythink_auth {
            GetAuthStatusResponse {
                auth_method: None,
                auth_token: None,
                requires_odysseythink_auth: Some(false),
            }
        } else {
            let auth = self.auth_manager.auth().await;
            match auth {
                Some(auth) => {
                    let auth_mode = auth.api_auth_mode();
                    let (reported_auth_method, token_opt) = {
                        match auth.get_token() {
                            Ok(token) if !token.is_empty() => {
                                let tok = if include_token { Some(token) } else { None };
                                (Some(auth_mode), tok)
                            }
                            Ok(_) => (None, None),
                            Err(err) => {
                                tracing::warn!("failed to get token for auth status: {err}");
                                (None, None)
                            }
                        }
                    };
                    GetAuthStatusResponse {
                        auth_method: reported_auth_method,
                        auth_token: token_opt,
                        requires_odysseythink_auth: Some(true),
                    }
                }
                None => GetAuthStatusResponse {
                    auth_method: None,
                    auth_token: None,
                    requires_odysseythink_auth: Some(true),
                },
            }
        };

        Ok(response)
    }

    async fn get_account_response(
        &self,
        _params: GetAccountParams,
    ) -> Result<GetAccountResponse, JSONRPCErrorError> {
        let provider = create_model_provider(
            self.config.model_provider.clone(),
            Some(self.auth_manager.clone()),
        );
        let account_state = match provider.account_state() {
            Ok(account_state) => account_state,
            Err(err) => return Err(invalid_request(err.to_string())),
        };
        let account = account_state.account.map(Account::from);

        Ok(GetAccountResponse {
            account,
            requires_odysseythink_auth: account_state.requires_odysseythink_auth,
        })
    }
}
