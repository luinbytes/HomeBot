use std::collections::HashSet;

use homebot_protocol::{
    ActivitySummary, ApprovalStatus, ApprovalSummary, ChatSummary, ChatTimelineResponse,
    CheckpointRestoreSummary, ContextCompactionStrategy, InteractionMode, MessagePart,
    MessageStatus, MessageSummary, QueuedPromptSummary, SequenceDisposition, ServerEvent,
    ServerEventBody, TurnCheckpointSummary, WorkingContextSummary, classify_sequence,
};
use uuid::Uuid;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerDraft {
    pub content: String,
    pub attachment_ids: Vec<Uuid>,
    pub reply_to_message_id: Option<Uuid>,
    pub mentioned_bot_ids: Vec<Uuid>,
    pub skill_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimelineCommand {
    Send(ComposerDraft),
    Queue(ComposerDraft),
    Steer(ComposerDraft),
    Stop,
    Retry(Uuid),
    DecideApproval {
        approval_id: Uuid,
        allow: bool,
    },
    LoadCheckpointDiff {
        from_checkpoint_id: Uuid,
        to_checkpoint_id: Uuid,
    },
    RestoreCheckpoint(Uuid),
    SetInteractionMode(InteractionMode),
    CompactContext {
        strategy: ContextCompactionStrategy,
        target_tokens: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScrollAnchor {
    pub at_bottom: bool,
    pub unseen_updates: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconcileOutcome {
    Applied,
    Duplicate,
    Gap,
    Unrelated,
}

#[derive(Clone, Debug)]
pub struct TimelineModel {
    pub chat: Option<ChatSummary>,
    pub messages: Vec<MessageSummary>,
    pub activities: Vec<ActivitySummary>,
    pub approvals: Vec<ApprovalSummary>,
    pub queued_prompts: Vec<QueuedPromptSummary>,
    pub working_context: Option<WorkingContextSummary>,
    pub checkpoints: Vec<TurnCheckpointSummary>,
    pub last_restore: Option<CheckpointRestoreSummary>,
    pub composer: ComposerDraft,
    pub cursor: u64,
    pub needs_snapshot: bool,
    pub scroll: ScrollAnchor,
    applied_event_ids: HashSet<Uuid>,
    commands: Vec<TimelineCommand>,
}

impl Default for TimelineModel {
    fn default() -> Self {
        Self {
            chat: None,
            messages: Vec::new(),
            activities: Vec::new(),
            approvals: Vec::new(),
            queued_prompts: Vec::new(),
            working_context: None,
            checkpoints: Vec::new(),
            last_restore: None,
            composer: ComposerDraft::default(),
            cursor: 0,
            needs_snapshot: false,
            scroll: ScrollAnchor {
                at_bottom: true,
                unseen_updates: 0,
            },
            applied_event_ids: HashSet::new(),
            commands: Vec::new(),
        }
    }
}

impl TimelineModel {
    pub fn hydrate(&mut self, timeline: ChatTimelineResponse) {
        self.chat = Some(timeline.chat);
        self.messages = timeline.messages;
        self.activities = timeline.activities;
        self.approvals = timeline.approvals;
        self.queued_prompts = timeline.queued_prompts;
        self.working_context = timeline.working_context;
        self.checkpoints = timeline.checkpoints;
        self.last_restore = None;
        self.cursor = timeline.boundary_sequence;
        self.needs_snapshot = false;
        self.applied_event_ids.clear();
        self.sort_all();
    }

    pub fn apply_event(&mut self, event: ServerEvent) -> ReconcileOutcome {
        if self.applied_event_ids.contains(&event.event_id) {
            return ReconcileOutcome::Duplicate;
        }
        match classify_sequence(self.cursor, event.sequence) {
            SequenceDisposition::Duplicate => return ReconcileOutcome::Duplicate,
            SequenceDisposition::Gap => {
                self.needs_snapshot = true;
                return ReconcileOutcome::Gap;
            }
            SequenceDisposition::Next => {}
        }
        let relevant = self.apply_body(event.body);
        self.cursor = event.sequence;
        self.applied_event_ids.insert(event.event_id);
        if relevant {
            self.content_grew();
            ReconcileOutcome::Applied
        } else {
            ReconcileOutcome::Unrelated
        }
    }

    fn apply_body(&mut self, body: ServerEventBody) -> bool {
        let chat_id = self.chat.as_ref().map(|chat| chat.id);
        match body {
            ServerEventBody::ChatChanged { chat } if Some(chat.id) == chat_id => {
                self.chat = Some(chat);
                true
            }
            ServerEventBody::MessageChanged { message } if Some(message.chat_id) == chat_id => {
                upsert(&mut self.messages, message, |message| message.id);
                self.messages.sort_by_key(|message| message.created_at_ms);
                true
            }
            ServerEventBody::MessageDelta {
                chat_id: event_chat,
                message_id,
                delta,
            } if Some(event_chat) == chat_id => {
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .find(|message| message.id == message_id)
                {
                    append_delta(message, &delta);
                    true
                } else {
                    self.needs_snapshot = true;
                    false
                }
            }
            ServerEventBody::ActivityChanged { activity } if Some(activity.chat_id) == chat_id => {
                upsert(&mut self.activities, activity, |activity| activity.id);
                self.activities
                    .sort_by_key(|activity| activity.started_at_ms);
                true
            }
            ServerEventBody::ApprovalChanged { approval } if Some(approval.chat_id) == chat_id => {
                upsert(&mut self.approvals, approval, |approval| approval.id);
                self.approvals
                    .sort_by_key(|approval| approval.created_at_ms);
                true
            }
            ServerEventBody::QueuedPromptChanged { prompt } if Some(prompt.chat_id) == chat_id => {
                upsert(&mut self.queued_prompts, prompt, |prompt| prompt.id);
                self.queued_prompts.sort_by_key(|prompt| prompt.position);
                true
            }
            ServerEventBody::QueuedPromptRemoved {
                chat_id: event_chat,
                prompt_id,
            } if Some(event_chat) == chat_id => {
                self.queued_prompts.retain(|prompt| prompt.id != prompt_id);
                true
            }
            ServerEventBody::WorkingContextChanged { context }
                if Some(context.chat_id) == chat_id =>
            {
                self.working_context = Some(context);
                true
            }
            ServerEventBody::TurnCheckpointChanged { checkpoint }
                if Some(checkpoint.chat_id) == chat_id =>
            {
                upsert(&mut self.checkpoints, checkpoint, |checkpoint| {
                    checkpoint.id
                });
                self.checkpoints
                    .sort_by_key(|checkpoint| checkpoint.created_at_unix_ms);
                true
            }
            ServerEventBody::CheckpointRestored { restore } if Some(restore.chat_id) == chat_id => {
                self.last_restore = Some(restore);
                true
            }
            _ => false,
        }
    }

    /// Queues a send, follow-up, or steering command from the current composer.
    ///
    /// # Errors
    ///
    /// Returns `EmptyComposer` when there is no text or attachment.
    pub fn submit(&mut self, steer: bool) -> Result<(), ComposerError> {
        if self.composer.content.trim().is_empty() && self.composer.attachment_ids.is_empty() {
            return Err(ComposerError::EmptyComposer);
        }
        self.composer.content = self.composer.content.trim().to_owned();
        let draft = std::mem::take(&mut self.composer);
        let running = self.chat.as_ref().is_some_and(|chat| chat.running);
        self.commands.push(if steer && running {
            TimelineCommand::Steer(draft)
        } else if running {
            TimelineCommand::Queue(draft)
        } else {
            TimelineCommand::Send(draft)
        });
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.chat.as_ref().is_some_and(|chat| chat.running) {
            self.commands.push(TimelineCommand::Stop);
        }
    }

    pub fn set_interaction_mode(&mut self, mode: InteractionMode) {
        if self
            .working_context
            .as_ref()
            .is_some_and(|context| context.interaction_mode != mode)
        {
            self.commands
                .push(TimelineCommand::SetInteractionMode(mode));
        }
    }

    pub fn compact_context(&mut self, strategy: ContextCompactionStrategy) {
        if self.chat.as_ref().is_some_and(|chat| !chat.running) {
            self.commands.push(TimelineCommand::CompactContext {
                strategy,
                target_tokens: None,
            });
        }
    }

    pub fn retry(&mut self, message_id: Uuid) {
        if self
            .messages
            .iter()
            .any(|message| message.id == message_id && message.status == MessageStatus::Failed)
        {
            self.commands.push(TimelineCommand::Retry(message_id));
        }
    }

    pub fn decide_approval(&mut self, approval_id: Uuid, allow: bool) {
        if self.approvals.iter().any(|approval| {
            approval.id == approval_id && approval.status == ApprovalStatus::Pending
        }) {
            self.commands
                .push(TimelineCommand::DecideApproval { approval_id, allow });
        }
    }

    pub fn load_checkpoint_diff(&mut self, from_checkpoint_id: Uuid, to_checkpoint_id: Uuid) {
        self.commands.push(TimelineCommand::LoadCheckpointDiff {
            from_checkpoint_id,
            to_checkpoint_id,
        });
    }

    pub fn restore_checkpoint(&mut self, checkpoint_id: Uuid) {
        if self.chat.as_ref().is_some_and(|chat| !chat.running)
            && self
                .checkpoints
                .iter()
                .any(|checkpoint| checkpoint.id == checkpoint_id)
        {
            self.commands
                .push(TimelineCommand::RestoreCheckpoint(checkpoint_id));
        }
    }

    pub fn set_at_bottom(&mut self, at_bottom: bool) {
        self.scroll.at_bottom = at_bottom;
        if at_bottom {
            self.scroll.unseen_updates = 0;
        }
    }

    #[must_use]
    pub fn take_commands(&mut self) -> Vec<TimelineCommand> {
        std::mem::take(&mut self.commands)
    }

    fn content_grew(&mut self) {
        if !self.scroll.at_bottom {
            self.scroll.unseen_updates = self.scroll.unseen_updates.saturating_add(1);
        }
    }

    fn sort_all(&mut self) {
        self.messages.sort_by_key(|message| message.created_at_ms);
        self.activities
            .sort_by_key(|activity| activity.started_at_ms);
        self.approvals
            .sort_by_key(|approval| approval.created_at_ms);
        self.queued_prompts.sort_by_key(|prompt| prompt.position);
    }
}

fn upsert<T, F>(values: &mut Vec<T>, changed: T, key: F)
where
    F: Fn(&T) -> Uuid,
{
    let changed_id = key(&changed);
    if let Some(existing) = values.iter_mut().find(|value| key(value) == changed_id) {
        *existing = changed;
    } else {
        values.push(changed);
    }
}

fn append_delta(message: &mut MessageSummary, delta: &str) {
    if let Some(MessagePart::Text { text, .. }) = message
        .parts
        .iter_mut()
        .rev()
        .find(|part| matches!(part, MessagePart::Text { .. }))
    {
        text.push_str(delta);
    } else {
        let ordinal = u32::try_from(message.parts.len()).unwrap_or(u32::MAX);
        message.parts.push(MessagePart::Text {
            id: Uuid::now_v7(),
            ordinal,
            text: delta.to_owned(),
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposerError {
    EmptyComposer,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::performance::{
        CHAT_OPEN_BUDGET, CONCURRENT_BOT_BUDGET, LARGE_TRANSCRIPT_MESSAGES, STREAM_FRAME_BUDGET,
    };
    use homebot_protocol::{MessageAuthor, PROTOCOL_VERSION, QueuedPromptKind};
    use std::time::Instant;

    fn chat(id: Uuid, running: bool) -> ChatSummary {
        ChatSummary {
            id,
            title: "Nova".to_owned(),
            bot_id: Uuid::now_v7(),
            unread_count: 0,
            running,
            queued_count: 0,
            last_sequence: 0,
        }
    }

    fn message(chat_id: Uuid, id: Uuid, status: MessageStatus) -> MessageSummary {
        MessageSummary {
            id,
            chat_id,
            author: MessageAuthor::Bot,
            author_bot_id: None,
            status,
            parts: vec![MessagePart::Text {
                id: Uuid::now_v7(),
                ordinal: 0,
                text: "Hel".to_owned(),
            }],
            reply_to_message_id: None,
            mentioned_bot_ids: Vec::new(),
            shared_context_message_ids: Vec::new(),
            applied_skills: Vec::new(),
            created_at_ms: 1,
            completed_at_ms: None,
            error: None,
        }
    }

    fn event(sequence: u64, body: ServerEventBody) -> ServerEvent {
        ServerEvent {
            protocol_version: PROTOCOL_VERSION,
            sequence,
            event_id: Uuid::now_v7(),
            body,
        }
    }

    #[test]
    fn reconnect_deduplicates_streaming_and_detects_gaps() {
        let chat_id = Uuid::now_v7();
        let message_id = Uuid::now_v7();
        let mut model = TimelineModel::default();
        model.hydrate(ChatTimelineResponse {
            chat: chat(chat_id, true),
            messages: vec![message(chat_id, message_id, MessageStatus::Streaming)],
            activities: Vec::new(),
            approvals: Vec::new(),
            queued_prompts: Vec::new(),
            working_context: None,
            checkpoints: Vec::new(),
            boundary_sequence: 10,
        });
        let delta = event(
            11,
            ServerEventBody::MessageDelta {
                chat_id,
                message_id,
                delta: "lo".to_owned(),
            },
        );
        assert_eq!(model.apply_event(delta.clone()), ReconcileOutcome::Applied);
        assert_eq!(model.apply_event(delta), ReconcileOutcome::Duplicate);
        let MessagePart::Text { text, .. } = &model.messages[0].parts[0] else {
            panic!("expected text")
        };
        assert_eq!(text, "Hello");
        assert_eq!(
            model.apply_event(event(
                13,
                ServerEventBody::Ping {
                    nonce: Uuid::now_v7()
                }
            )),
            ReconcileOutcome::Gap
        );
        assert!(model.needs_snapshot);
    }

    #[test]
    fn composer_supports_send_queue_steer_stop_retry_and_scroll_anchor() {
        let chat_id = Uuid::now_v7();
        let failed_id = Uuid::now_v7();
        let mut model = TimelineModel::default();
        model.hydrate(ChatTimelineResponse {
            chat: chat(chat_id, true),
            messages: vec![message(chat_id, failed_id, MessageStatus::Failed)],
            activities: Vec::new(),
            approvals: Vec::new(),
            queued_prompts: Vec::new(),
            working_context: None,
            checkpoints: Vec::new(),
            boundary_sequence: 0,
        });
        model.composer.content = " Follow up ".to_owned();
        assert!(model.submit(false).is_ok());
        model.composer.content = "Use the other file".to_owned();
        assert!(model.submit(true).is_ok());
        model.stop();
        model.retry(failed_id);
        assert!(matches!(model.take_commands().as_slice(), [
            TimelineCommand::Queue(_),
            TimelineCommand::Steer(_),
            TimelineCommand::Stop,
            TimelineCommand::Retry(id)
        ] if *id == failed_id));

        model.set_at_bottom(false);
        let changed = message(chat_id, Uuid::now_v7(), MessageStatus::Completed);
        assert_eq!(
            model.apply_event(event(
                1,
                ServerEventBody::MessageChanged { message: changed }
            )),
            ReconcileOutcome::Applied
        );
        assert_eq!(model.scroll.unseen_updates, 1);
        model.set_at_bottom(true);
        assert_eq!(model.scroll.unseen_updates, 0);
    }

    #[test]
    fn queued_prompt_projection_preserves_server_kind_order_and_removal() {
        let chat_id = Uuid::now_v7();
        let steering_id = Uuid::now_v7();
        let follow_id = Uuid::now_v7();
        let prompt =
            |id: Uuid, kind: QueuedPromptKind, content: &str, position: u32| QueuedPromptSummary {
                id,
                chat_id,
                content: content.to_owned(),
                attachment_ids: Vec::new(),
                skill_ids: Vec::new(),
                kind,
                position,
                created_at_ms: i64::from(position),
            };
        let mut model = TimelineModel::default();
        model.hydrate(ChatTimelineResponse {
            chat: chat(chat_id, true),
            messages: Vec::new(),
            activities: Vec::new(),
            approvals: Vec::new(),
            queued_prompts: vec![
                prompt(steering_id, QueuedPromptKind::Steering, "Redirect", 0),
                prompt(follow_id, QueuedPromptKind::FollowUp, "Then test", 1),
            ],
            working_context: None,
            checkpoints: Vec::new(),
            boundary_sequence: 10,
        });
        assert_eq!(model.queued_prompts[0].kind, QueuedPromptKind::Steering);
        assert_eq!(
            model.apply_event(event(
                11,
                ServerEventBody::QueuedPromptRemoved {
                    chat_id,
                    prompt_id: steering_id,
                }
            )),
            ReconcileOutcome::Applied
        );
        assert_eq!(
            model.apply_event(event(
                12,
                ServerEventBody::QueuedPromptChanged {
                    prompt: prompt(follow_id, QueuedPromptKind::FollowUp, "Then test", 0)
                }
            )),
            ReconcileOutcome::Applied
        );
        assert_eq!(model.queued_prompts.len(), 1);
        assert_eq!(model.queued_prompts[0].position, 0);
    }

    #[test]
    fn working_context_projection_and_commands_remain_server_authoritative() {
        let chat_id = Uuid::now_v7();
        let mut model = TimelineModel {
            chat: Some(chat(chat_id, false)),
            ..TimelineModel::default()
        };
        let context = WorkingContextSummary {
            chat_id,
            provider_profile_id: Uuid::now_v7(),
            interaction_mode: InteractionMode::Default,
            plan_mode_available: true,
            compaction_available: true,
            reset_available: true,
            used_tokens: Some(800),
            context_window_tokens: Some(4_000),
            compaction_status: homebot_protocol::ContextCompactionStatus::Idle,
            generation: 0,
            compacted_at_ms: None,
            error_message: None,
            updated_at_ms: 1,
        };
        assert_eq!(
            model.apply_event(event(
                1,
                ServerEventBody::WorkingContextChanged {
                    context: context.clone()
                }
            )),
            ReconcileOutcome::Applied
        );
        assert_eq!(model.working_context, Some(context));
        model.set_interaction_mode(InteractionMode::Plan);
        model.compact_context(ContextCompactionStrategy::Compact);
        assert!(matches!(
            model.take_commands().as_slice(),
            [
                TimelineCommand::SetInteractionMode(InteractionMode::Plan),
                TimelineCommand::CompactContext {
                    strategy: ContextCompactionStrategy::Compact,
                    target_tokens: None,
                }
            ]
        ));
    }

    #[test]
    fn checkpoint_projection_and_commands_remain_server_authoritative() {
        let chat_id = Uuid::now_v7();
        let workspace_id = Uuid::now_v7();
        let before_id = Uuid::now_v7();
        let after_id = Uuid::now_v7();
        let mut model = TimelineModel::default();
        model.hydrate(ChatTimelineResponse {
            chat: chat(chat_id, false),
            messages: Vec::new(),
            activities: Vec::new(),
            approvals: Vec::new(),
            queued_prompts: Vec::new(),
            working_context: None,
            checkpoints: vec![homebot_protocol::TurnCheckpointSummary {
                id: before_id,
                chat_id,
                workspace_id,
                message_id: None,
                phase: homebot_protocol::CheckpointPhase::BeforeTurn,
                created_at_unix_ms: 1,
            }],
            boundary_sequence: 4,
        });
        assert_eq!(
            model.apply_event(event(
                5,
                ServerEventBody::TurnCheckpointChanged {
                    checkpoint: homebot_protocol::TurnCheckpointSummary {
                        id: after_id,
                        chat_id,
                        workspace_id,
                        message_id: None,
                        phase: homebot_protocol::CheckpointPhase::AfterTurn,
                        created_at_unix_ms: 2,
                    }
                }
            )),
            ReconcileOutcome::Applied
        );
        model.load_checkpoint_diff(before_id, after_id);
        model.restore_checkpoint(before_id);
        assert!(matches!(
            model.take_commands().as_slice(),
            [
                TimelineCommand::LoadCheckpointDiff {
                    from_checkpoint_id,
                    to_checkpoint_id
                },
                TimelineCommand::RestoreCheckpoint(checkpoint_id)
            ] if *from_checkpoint_id == before_id
                && *to_checkpoint_id == after_id
                && *checkpoint_id == before_id
        ));
    }

    #[test]
    fn large_transcript_and_concurrent_stream_projections_meet_release_budgets() {
        let chat_id = Uuid::now_v7();
        let messages = (0..LARGE_TRANSCRIPT_MESSAGES)
            .rev()
            .map(|index| {
                let mut value = message(chat_id, Uuid::now_v7(), MessageStatus::Completed);
                value.created_at_ms = i64::try_from(index).unwrap_or(i64::MAX);
                value
            })
            .collect();
        let started = Instant::now();
        let mut large = TimelineModel::default();
        large.hydrate(ChatTimelineResponse {
            chat: chat(chat_id, false),
            messages,
            activities: Vec::new(),
            approvals: Vec::new(),
            queued_prompts: Vec::new(),
            working_context: None,
            checkpoints: Vec::new(),
            boundary_sequence: 0,
        });
        assert!(
            started.elapsed() <= CHAT_OPEN_BUDGET,
            "10,000-message projection exceeded the chat-open budget"
        );
        assert_eq!(large.messages.len(), LARGE_TRANSCRIPT_MESSAGES);
        assert!(
            large
                .messages
                .windows(2)
                .all(|pair| pair[0].created_at_ms <= pair[1].created_at_ms)
        );

        let mut projections = (0..CONCURRENT_BOT_BUDGET)
            .map(|_| {
                let chat_id = Uuid::now_v7();
                let message_id = Uuid::now_v7();
                let mut model = TimelineModel::default();
                model.hydrate(ChatTimelineResponse {
                    chat: chat(chat_id, true),
                    messages: vec![message(chat_id, message_id, MessageStatus::Streaming)],
                    activities: Vec::new(),
                    approvals: Vec::new(),
                    queued_prompts: Vec::new(),
                    working_context: None,
                    checkpoints: Vec::new(),
                    boundary_sequence: 0,
                });
                (model, chat_id, message_id)
            })
            .collect::<Vec<_>>();
        let frames_per_bot = 250_u64;
        let streaming_started = Instant::now();
        for sequence in 1..=frames_per_bot {
            for (model, chat_id, message_id) in &mut projections {
                assert_eq!(
                    model.apply_event(event(
                        sequence,
                        ServerEventBody::MessageDelta {
                            chat_id: *chat_id,
                            message_id: *message_id,
                            delta: "x".to_owned(),
                        },
                    )),
                    ReconcileOutcome::Applied
                );
            }
        }
        let operations = frames_per_bot * u64::try_from(CONCURRENT_BOT_BUDGET).unwrap_or(u64::MAX);
        assert!(
            streaming_started.elapsed()
                <= STREAM_FRAME_BUDGET * u32::try_from(operations).unwrap_or(u32::MAX),
            "multi-Bot streaming projection exceeded the per-frame budget"
        );
    }
}
