//! Reconnect-safe desktop projection for multi-Bot group chats.

use std::collections::HashSet;

use homebot_protocol::{
    GroupChatSummary, GroupParticipantSummary, GroupTimelineResponse, MessageSummary,
    OwnershipHandoffSummary, SequenceDisposition, ServerEvent, ServerEventBody, classify_sequence,
};
use uuid::Uuid;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GroupComposerDraft {
    pub content: String,
    pub mentioned_bot_ids: Vec<Uuid>,
    pub shared_context_message_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupTimelineCommand {
    Send(GroupComposerDraft),
    Rename(String),
    AddParticipant(Uuid),
    RemoveParticipant(Uuid),
    Handoff {
        from_bot_id: Uuid,
        to_bot_id: Uuid,
        message_id: Option<Uuid>,
        reason: String,
    },
    Stop,
}

#[derive(Clone, Debug, Default)]
pub struct GroupTimelineModel {
    pub group: Option<GroupChatSummary>,
    pub participants: Vec<GroupParticipantSummary>,
    pub messages: Vec<MessageSummary>,
    pub handoffs: Vec<OwnershipHandoffSummary>,
    pub composer: GroupComposerDraft,
    pub cursor: u64,
    pub needs_snapshot: bool,
    applied_event_ids: HashSet<Uuid>,
    commands: Vec<GroupTimelineCommand>,
}

impl GroupTimelineModel {
    pub fn hydrate(&mut self, timeline: GroupTimelineResponse) {
        self.group = Some(timeline.group);
        self.participants = timeline.participants;
        self.messages = timeline.messages;
        self.handoffs = timeline.handoffs;
        self.cursor = timeline.boundary_sequence;
        self.needs_snapshot = false;
        self.applied_event_ids.clear();
        self.sort();
    }

    pub fn apply_event(&mut self, event: ServerEvent) -> GroupReconcileOutcome {
        if self.applied_event_ids.contains(&event.event_id)
            || classify_sequence(self.cursor, event.sequence) == SequenceDisposition::Duplicate
        {
            return GroupReconcileOutcome::Duplicate;
        }
        if classify_sequence(self.cursor, event.sequence) == SequenceDisposition::Gap {
            self.needs_snapshot = true;
            return GroupReconcileOutcome::Gap;
        }
        let chat_id = self.group.as_ref().map(|group| group.id);
        let relevant = match event.body {
            ServerEventBody::GroupChatChanged { group } if Some(group.id) == chat_id => {
                self.group = Some(group);
                true
            }
            ServerEventBody::GroupParticipantChanged { participant }
                if Some(participant.chat_id) == chat_id =>
            {
                upsert(&mut self.participants, participant, |item| item.bot_id);
                true
            }
            ServerEventBody::GroupParticipantRemoved {
                chat_id: event_chat,
                bot_id,
            } if Some(event_chat) == chat_id => {
                self.participants
                    .retain(|participant| participant.bot_id != bot_id);
                true
            }
            ServerEventBody::GroupHandoffRecorded { handoff }
                if Some(handoff.chat_id) == chat_id =>
            {
                upsert(&mut self.handoffs, handoff, |item| item.id);
                true
            }
            ServerEventBody::MessageChanged { message } if Some(message.chat_id) == chat_id => {
                upsert(&mut self.messages, message, |item| item.id);
                true
            }
            _ => false,
        };
        self.cursor = event.sequence;
        self.applied_event_ids.insert(event.event_id);
        self.sort();
        if relevant {
            GroupReconcileOutcome::Applied
        } else {
            GroupReconcileOutcome::Unrelated
        }
    }

    /// Queues a user group message after checking mentions are participants.
    ///
    /// # Errors
    ///
    /// Rejects empty content and mentions outside the current group.
    pub fn submit(&mut self) -> Result<(), GroupComposerError> {
        if self.composer.content.trim().is_empty() {
            return Err(GroupComposerError::EmptyComposer);
        }
        if self.composer.mentioned_bot_ids.iter().any(|bot_id| {
            !self
                .participants
                .iter()
                .any(|participant| participant.bot_id == *bot_id)
        }) {
            return Err(GroupComposerError::UnknownMention);
        }
        self.composer.content = self.composer.content.trim().to_owned();
        self.commands
            .push(GroupTimelineCommand::Send(std::mem::take(
                &mut self.composer,
            )));
        Ok(())
    }

    pub fn handoff(&mut self, to_bot_id: Uuid, message_id: Option<Uuid>, reason: &str) {
        let Some(group) = &self.group else { return };
        if !group.stop_requested
            && group.ownership_bot_id != to_bot_id
            && self
                .participants
                .iter()
                .any(|participant| participant.bot_id == to_bot_id)
            && !reason.trim().is_empty()
        {
            self.commands.push(GroupTimelineCommand::Handoff {
                from_bot_id: group.ownership_bot_id,
                to_bot_id,
                message_id,
                reason: reason.trim().to_owned(),
            });
        }
    }

    pub fn stop(&mut self) {
        if self
            .group
            .as_ref()
            .is_some_and(|group| !group.stop_requested)
        {
            self.commands.push(GroupTimelineCommand::Stop);
        }
    }

    pub fn rename(&mut self, title: &str) {
        let title = title.trim();
        if !title.is_empty() && title.chars().count() <= 120 {
            self.commands
                .push(GroupTimelineCommand::Rename(title.to_owned()));
        }
    }

    pub fn add_participant(&mut self, bot_id: Uuid) {
        if self.participants.len() < 6
            && !self
                .participants
                .iter()
                .any(|participant| participant.bot_id == bot_id)
        {
            self.commands
                .push(GroupTimelineCommand::AddParticipant(bot_id));
        }
    }

    pub fn remove_participant(&mut self, bot_id: Uuid) {
        if self.participants.len() > 2
            && self
                .group
                .as_ref()
                .is_some_and(|group| group.ownership_bot_id != bot_id)
            && self
                .participants
                .iter()
                .any(|participant| participant.bot_id == bot_id)
        {
            self.commands
                .push(GroupTimelineCommand::RemoveParticipant(bot_id));
        }
    }

    #[must_use]
    pub fn take_commands(&mut self) -> Vec<GroupTimelineCommand> {
        std::mem::take(&mut self.commands)
    }

    fn sort(&mut self) {
        self.participants
            .sort_by_key(|participant| participant.bot_id);
        self.messages.sort_by_key(|message| message.created_at_ms);
        self.handoffs.sort_by_key(|handoff| handoff.created_at_ms);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupReconcileOutcome {
    Applied,
    Duplicate,
    Gap,
    Unrelated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupComposerError {
    EmptyComposer,
    UnknownMention,
}

fn upsert<T>(values: &mut Vec<T>, changed: T, key: impl Fn(&T) -> Uuid) {
    let id = key(&changed);
    if let Some(existing) = values.iter_mut().find(|value| key(value) == id) {
        *existing = changed;
    } else {
        values.push(changed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homebot_protocol::{GroupBotStatus, GroupParticipantRole, PROTOCOL_VERSION};

    #[test]
    fn three_bot_group_handoff_stop_and_gap_are_projected() {
        let chat_id = Uuid::now_v7();
        let bots = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        let mut model = GroupTimelineModel::default();
        model.hydrate(GroupTimelineResponse {
            group: GroupChatSummary {
                id: chat_id,
                title: "Release team".to_owned(),
                ownership_bot_id: bots[0],
                coordination_max_turns: 8,
                coordination_turns_used: 1,
                max_parallel_bots: 3,
                stop_requested: false,
            },
            participants: bots
                .into_iter()
                .enumerate()
                .map(|(index, bot_id)| GroupParticipantSummary {
                    chat_id,
                    bot_id,
                    role: if index == 0 {
                        GroupParticipantRole::Owner
                    } else {
                        GroupParticipantRole::Member
                    },
                    status: GroupBotStatus::Running,
                    active_operation_id: Some(Uuid::now_v7()),
                    updated_at_ms: 1,
                })
                .collect(),
            messages: Vec::new(),
            handoffs: Vec::new(),
            boundary_sequence: 4,
        });
        model.composer.content = "@Patch verify it".to_owned();
        model.composer.mentioned_bot_ids = vec![bots[1]];
        assert!(model.submit().is_ok());
        model.handoff(bots[2], None, "Own final verification");
        model.stop();
        assert_eq!(model.take_commands().len(), 3);

        let outcome = model.apply_event(ServerEvent {
            protocol_version: PROTOCOL_VERSION,
            sequence: 6,
            event_id: Uuid::now_v7(),
            body: ServerEventBody::GroupChatChanged {
                group: model.group.clone().unwrap_or_else(|| unreachable!()),
            },
        });
        assert_eq!(outcome, GroupReconcileOutcome::Gap);
        assert!(model.needs_snapshot);
    }
}
