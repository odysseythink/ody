use super::*;
use crate::error_code::internal_error;

#[derive(Clone)]
pub(crate) struct AccountRequestProcessor {
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
}

impl AccountRequestProcessor {
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
        AccountUpdatedNotification {
            auth_mode: None,
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
    ) {
        thread_manager
            .plugins_manager()
            .set_auth_mode(None);

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
        _params: &LoginApiKeyParams,
    ) -> std::result::Result<(), JSONRPCErrorError> {
        Ok(())
    }

    async fn login_api_key_v2(&self, request_id: ConnectionRequestId, _params: LoginApiKeyParams) {
        let result: std::result::Result<LoginAccountResponse, ody_app_server_protocol::JSONRPCErrorError> = Ok(LoginAccountResponse::ApiKey {});
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
        Self::maybe_refresh_plugin_caches_for_current_config(
            &self.config_manager,
            &self.thread_manager,
        )
        .await;

        Ok(None)
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
        _params: GetAuthStatusParams,
    ) -> Result<GetAuthStatusResponse, JSONRPCErrorError> {
        Ok(GetAuthStatusResponse {
            auth_method: None,
            auth_token: None,
        })
    }

    async fn get_account_response(
        &self,
        _params: GetAccountParams,
    ) -> Result<GetAccountResponse, JSONRPCErrorError> {
        let provider = create_model_provider(
            self.config.model_provider.clone(),
        );
        provider
            .account_state()
            .map_err(|err| invalid_request(err.to_string()))?;
        let account: Option<Account> = None;

        Ok(GetAccountResponse {
            account,
        })
    }
}
