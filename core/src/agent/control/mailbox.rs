use super::AgentControl;
use ody_protocol::ThreadId;
use ody_protocol::error::OdyErr;
use ody_protocol::error::Result as OdyResult;
use ody_protocol::protocol::InterAgentCommunication;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use tracing::warn;

/// Control-plane mailbox retaining queue-only inter-agent mail for agents
/// whose threads are currently unloaded from memory (e.g. after v2 residency
/// eviction). Trigger-turn mail keeps its target loaded, so only queue-only
/// mail ever lands here. Ported from upstream openai/codex#50087.
///
/// All deliveries go through a single async delivery lock: senders first
/// retain their mail (in-lock FIFO) and then drain-and-deliver under the
/// lock, so concurrent senders cannot interleave deliveries out of mailbox
/// order, and a failed delivery requeues the unsent remainder at the front
/// instead of dropping it.
#[derive(Default)]
pub(super) struct RuntimeMailbox {
    retained: Mutex<HashMap<ThreadId, VecDeque<InterAgentCommunication>>>,
    delivery: tokio::sync::Mutex<()>,
}

impl RuntimeMailbox {
    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<ThreadId, VecDeque<InterAgentCommunication>>> {
        self.retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn retain(&self, thread_id: ThreadId, mail: InterAgentCommunication) {
        self.lock().entry(thread_id).or_default().push_back(mail);
    }

    pub(super) fn retain_all(&self, thread_id: ThreadId, mails: Vec<InterAgentCommunication>) {
        if mails.is_empty() {
            return;
        }
        self.lock().entry(thread_id).or_default().extend(mails);
    }

    /// Requeues mails that failed delivery ahead of anything retained since.
    /// The requeued mails are all older than the current queue contents, so
    /// prepending preserves the global FIFO order.
    pub(super) fn retain_front(
        &self,
        thread_id: ThreadId,
        mails: VecDeque<InterAgentCommunication>,
    ) {
        if mails.is_empty() {
            return;
        }
        let mut retained = self.lock();
        let queue = retained.entry(thread_id).or_default();
        let mut restored = mails;
        restored.append(queue);
        *queue = restored;
    }

    /// Removes and returns all retained mail for `thread_id` in FIFO order.
    pub(super) fn take(&self, thread_id: ThreadId) -> Vec<InterAgentCommunication> {
        self.lock()
            .remove(&thread_id)
            .map(Vec::from)
            .unwrap_or_default()
    }

    /// Drops retained mail for an agent that is being removed for good.
    pub(super) fn discard(&self, thread_id: ThreadId) {
        self.lock().remove(&thread_id);
    }

    /// Serializes drain-and-deliver across concurrent senders.
    pub(super) async fn lock_delivery(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.delivery.lock().await
    }

    #[cfg(test)]
    pub(super) fn retained_for(&self, thread_id: ThreadId) -> Vec<InterAgentCommunication> {
        self.lock()
            .get(&thread_id)
            .map(|mails| mails.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl AgentControl {
    /// Delivers retained mail to a loaded agent in FIFO order, holding the
    /// mailbox delivery lock so concurrent senders cannot interleave. When a
    /// delivery fails without the agent dying, the unsent remainder (including
    /// the failed mail) is requeued at the front; when the agent died, its
    /// mailbox has already been discarded and the mail is not resurrected.
    pub(super) async fn deliver_retained_mail(&self, thread_id: ThreadId) -> OdyResult<()> {
        let _delivery_guard = self.runtime_mailbox.lock_delivery().await;
        let state = self.upgrade()?;
        if state.get_thread(thread_id).await.is_err() {
            // Unloaded (e.g. evicted): mail stays retained until the next reload.
            return Ok(());
        }
        let mut mails: VecDeque<InterAgentCommunication> =
            self.runtime_mailbox.take(thread_id).into();
        while let Some(mail) = mails.pop_front() {
            match self
                .send_inter_agent_communication(thread_id, mail.clone())
                .await
            {
                Ok(_) => {}
                Err(err @ OdyErr::InternalAgentDied) => return Err(err),
                Err(err) => {
                    mails.push_front(mail);
                    self.runtime_mailbox.retain_front(thread_id, mails);
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Flushes retained mail after a reload, logging delivery failures: the
    /// unsent remainder stays retained and is retried on the next delivery.
    pub(super) async fn flush_retained_mail(&self, thread_id: ThreadId) {
        if let Err(err) = self.deliver_retained_mail(thread_id).await {
            warn!("failed to deliver retained mail to reloaded agent {thread_id}: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::AgentPath;

    fn mail(content: &str) -> InterAgentCommunication {
        InterAgentCommunication::new(
            AgentPath::root(),
            AgentPath::try_from("/root/worker").expect("agent path"),
            Vec::new(),
            content.to_string(),
            /*trigger_turn*/ false,
        )
    }

    #[test]
    fn retain_front_restores_failed_delivery_ahead_of_newer_mail() {
        let mailbox = RuntimeMailbox::default();
        let thread_id = ThreadId::new();
        mailbox.retain(thread_id, mail("one"));
        mailbox.retain(thread_id, mail("two"));

        let mut taken: VecDeque<_> = mailbox.take(thread_id).into();
        // "one" failed to deliver; "three" arrived while delivery was in flight.
        mailbox.retain(thread_id, mail("three"));
        mailbox.retain_front(thread_id, taken.split_off(0));

        assert_eq!(
            mailbox
                .retained_for(thread_id)
                .iter()
                .map(|mail| mail.content.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn discard_drops_retained_mail() {
        let mailbox = RuntimeMailbox::default();
        let thread_id = ThreadId::new();
        mailbox.retain(thread_id, mail("one"));
        mailbox.discard(thread_id);
        assert!(mailbox.retained_for(thread_id).is_empty());
    }
}
