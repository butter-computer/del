//! A stable Del chat delegates execution to its current native session.
use super::*;

pub(super) struct RoutedBackend {
    pub inner: Arc<dyn AgentBackend>,
    pub chat: SessionId,
    pub native: SessionId,
    pub context: Option<std::path::PathBuf>,
}

impl RoutedBackend {
    fn handle(&self, mut handle: SessionHandle) -> Result<SessionHandle> {
        if handle.id != self.native {
            return Err(AgentError::Backend(
                "The provider changed the active session identity".into(),
            ));
        }
        handle.id = self.chat.clone();
        Ok(handle)
    }
}

#[async_trait::async_trait]
impl AgentBackend for RoutedBackend {
    fn provider(&self) -> Provider {
        self.inner.provider()
    }
    async fn session_process(&self, _: &SessionId) -> Option<(u32, bool)> {
        self.inner.session_process(&self.native).await
    }
    async fn list_commands(&self, cwd: &Path) -> Result<Vec<crate::ComposerCommand>> {
        self.inner.list_commands(cwd).await
    }
    async fn list_models(&self, cwd: &Path) -> Result<Vec<crate::ModelInfo>> {
        self.inner.list_models(cwd).await
    }
    async fn invoke_command(
        &self,
        _: &SessionId,
        command: crate::ComposerCommand,
        input: UserInput,
    ) -> Result<()> {
        self.inner
            .invoke_command(&self.native, command, input)
            .await
    }
    async fn control_goal(&self, _: &SessionId, action: crate::GoalAction) -> Result<()> {
        self.inner.control_goal(&self.native, action).await
    }
    async fn agent_goal(
        &self,
        _: &SessionId,
        action: Option<crate::GoalAction>,
    ) -> Result<Option<crate::Goal>> {
        self.inner.agent_goal(&self.native, action).await
    }
    async fn generate_title(&self, context: &str) -> Result<Option<String>> {
        self.inner.generate_title(context).await
    }
    async fn generate_session_title(&self, _: &SessionId, context: &str) -> Result<Option<String>> {
        self.inner
            .generate_session_title(&self.native, context)
            .await
    }
    async fn session_title(&self, _: &SessionId, cwd: &Path) -> Result<Option<String>> {
        self.inner.session_title(&self.native, cwd).await
    }
    async fn session_display_title(&self, _: &SessionId, cwd: &Path) -> Result<Option<String>> {
        self.inner.session_display_title(&self.native, cwd).await
    }
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        self.inner.start_session(config).await
    }
    async fn send_prompt(&self, _: &SessionId, input: UserInput) -> Result<()> {
        self.inner.send_prompt(&self.native, input).await
    }
    async fn steer(&self, _: &SessionId, text: String) -> Result<()> {
        self.inner.steer(&self.native, text).await
    }
    async fn steer_input(&self, _: &SessionId, input: UserInput) -> Result<()> {
        self.inner.steer_input(&self.native, input).await
    }
    async fn interrupt(&self, _: &SessionId) -> Result<()> {
        self.inner.interrupt(&self.native).await
    }
    async fn resume(&self, config: SessionConfig, _: &SessionId) -> Result<SessionHandle> {
        let handle = match &self.context {
            Some(context) => {
                self.inner
                    .resume_with_context(config, &self.native, context)
                    .await?
            }
            None => self.inner.resume(config, &self.native).await?,
        };
        self.handle(handle)
    }
    async fn resume_transferred(
        &self,
        config: SessionConfig,
        _: &SessionId,
        path: Option<&Path>,
    ) -> Result<SessionHandle> {
        if self.inner.provider() == Provider::Claude
            && let Some(context) = &self.context
        {
            return self.handle(
                self.inner
                    .resume_with_context(config, &self.native, context)
                    .await?,
            );
        }
        self.handle(
            self.inner
                .resume_transferred(config, &self.native, path)
                .await?,
        )
    }
    async fn reconfigure(&self, config: SessionConfig, _: &SessionId) -> Result<SessionHandle> {
        self.handle(self.inner.reconfigure(config, &self.native).await?)
    }
    async fn respond_approval(
        &self,
        _: &SessionId,
        approval: &str,
        response: ApprovalResponse,
    ) -> Result<()> {
        self.inner
            .respond_approval(&self.native, approval, response)
            .await
    }
    async fn close(&self, _: &SessionId) -> Result<()> {
        self.inner.close(&self.native).await
    }
}
