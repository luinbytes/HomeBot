//! Native desktop notification intents and exact `HomeBot` deep links.

use std::sync::mpsc::Sender;

use homebot_protocol::{
    ActivityKind, ActivityStatus, ApprovalStatus, MessageStatus, ServerEvent, ServerEventBody,
};
use notify_rust::Notification;
#[cfg(not(target_os = "macos"))]
use notify_rust::Urgency;
use uuid::Uuid;

use crate::settings::{DesktopSettings, NotificationTopic};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationKind {
    Finished,
    NeedsInput,
    NeedsApproval,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeepLink {
    pub bot_id: Option<Uuid>,
    pub chat_id: Uuid,
    pub message_id: Option<Uuid>,
    pub activity_id: Option<Uuid>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationIntent {
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub deep_link: DeepLink,
}

pub trait NotificationSink {
    /// Presents the notification using the host desktop.
    ///
    /// # Errors
    ///
    /// Returns a safe platform error when the notification service is unavailable.
    fn show(&self, intent: NotificationIntent) -> Result<(), String>;
}

#[derive(Debug)]
pub struct SystemNotificationSink {
    deep_links: Sender<DeepLink>,
}

impl SystemNotificationSink {
    #[must_use]
    pub const fn new(deep_links: Sender<DeepLink>) -> Self {
        Self { deep_links }
    }
}

impl NotificationSink for SystemNotificationSink {
    fn show(&self, intent: NotificationIntent) -> Result<(), String> {
        let mut notification = Notification::new();
        notification
            .appname("HomeBot")
            .summary(&intent.title)
            .body(&intent.body)
            .action("open", "Open HomeBot");
        #[cfg(not(target_os = "macos"))]
        notification.urgency(match intent.kind {
            NotificationKind::Finished => Urgency::Low,
            NotificationKind::NeedsInput
            | NotificationKind::NeedsApproval
            | NotificationKind::Error => Urgency::Critical,
        });
        let handle = notification
            .show()
            .map_err(|_| "Desktop notifications are unavailable".to_owned())?;
        let sender = self.deep_links.clone();
        std::thread::spawn(move || {
            handle.wait_for_action(|action| {
                if matches!(action, "open" | "default") {
                    let _ = sender.send(intent.deep_link);
                }
            });
        });
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct NotificationCenter {
    last_sequence: u64,
}

impl NotificationCenter {
    #[must_use]
    pub fn observe(
        &mut self,
        event: &ServerEvent,
        bot_id: Option<Uuid>,
        window_focused: bool,
        settings: &DesktopSettings,
    ) -> Option<NotificationIntent> {
        if event.sequence <= self.last_sequence {
            return None;
        }
        self.last_sequence = event.sequence;
        if window_focused && !settings.notifications.when_focused {
            return None;
        }
        match &event.body {
            ServerEventBody::ActivityChanged { activity }
                if activity.kind == ActivityKind::Interaction
                    && activity.requires_attention
                    && activity.status == ActivityStatus::Pending
                    && settings.notifications.includes(NotificationTopic::Approval) =>
            {
                Some(NotificationIntent {
                    kind: NotificationKind::NeedsInput,
                    title: "Input needed".to_owned(),
                    body: activity.title.clone(),
                    deep_link: DeepLink {
                        bot_id,
                        chat_id: activity.chat_id,
                        message_id: activity.message_id,
                        activity_id: Some(activity.id),
                    },
                })
            }
            ServerEventBody::ActivityChanged { activity }
                if activity.status == ActivityStatus::Succeeded
                    && settings.notifications.includes(NotificationTopic::Finished) =>
            {
                Some(NotificationIntent {
                    kind: NotificationKind::Finished,
                    title: "Bot finished".to_owned(),
                    body: activity.title.clone(),
                    deep_link: DeepLink {
                        bot_id,
                        chat_id: activity.chat_id,
                        message_id: activity.message_id,
                        activity_id: Some(activity.id),
                    },
                })
            }
            ServerEventBody::ActivityChanged { activity }
                if activity.status == ActivityStatus::Failed
                    && settings.notifications.includes(NotificationTopic::Error) =>
            {
                Some(NotificationIntent {
                    kind: NotificationKind::Error,
                    title: "Bot needs attention".to_owned(),
                    body: activity.title.clone(),
                    deep_link: DeepLink {
                        bot_id,
                        chat_id: activity.chat_id,
                        message_id: activity.message_id,
                        activity_id: Some(activity.id),
                    },
                })
            }
            ServerEventBody::ApprovalChanged { approval }
                if approval.status == ApprovalStatus::Pending
                    && settings.notifications.includes(NotificationTopic::Approval) =>
            {
                Some(NotificationIntent {
                    kind: NotificationKind::NeedsApproval,
                    title: "Approval needed".to_owned(),
                    body: approval.title.clone(),
                    deep_link: DeepLink {
                        bot_id,
                        chat_id: approval.chat_id,
                        message_id: approval.message_id,
                        activity_id: None,
                    },
                })
            }
            ServerEventBody::MessageChanged { message }
                if message.status == MessageStatus::Failed
                    && settings.notifications.includes(NotificationTopic::Error) =>
            {
                Some(NotificationIntent {
                    kind: NotificationKind::Error,
                    title: "Bot encountered an error".to_owned(),
                    body: "Open the chat to review or retry.".to_owned(),
                    deep_link: DeepLink {
                        bot_id,
                        chat_id: message.chat_id,
                        message_id: Some(message.id),
                        activity_id: None,
                    },
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use homebot_protocol::{
        ActivityDetail, ActivityKind, ActivityPresentation, ActivitySummary, RiskLevel,
    };

    use super::*;

    fn completed_event(sequence: u64) -> ServerEvent {
        ServerEvent {
            protocol_version: homebot_protocol::PROTOCOL_VERSION,
            sequence,
            event_id: Uuid::from_u128(u128::from(sequence)),
            body: ServerEventBody::ActivityChanged {
                activity: ActivitySummary {
                    id: Uuid::from_u128(5),
                    chat_id: Uuid::from_u128(4),
                    message_id: Some(Uuid::from_u128(3)),
                    title: "Release checks passed".to_owned(),
                    detail: "89 checks".to_owned(),
                    kind: ActivityKind::Terminal,
                    presentation: ActivityPresentation {
                        risk: RiskLevel::Low,
                        detail: ActivityDetail::Generic {
                            summary: "Complete".to_owned(),
                        },
                        copy_text: None,
                        open_artifact_id: None,
                    },
                    status: ActivityStatus::Succeeded,
                    requires_attention: false,
                    started_at_ms: 1,
                    finished_at_ms: Some(2),
                },
            },
        }
    }

    fn input_event(sequence: u64) -> ServerEvent {
        let mut event = completed_event(sequence);
        event.body = ServerEventBody::ActivityChanged {
            activity: ActivitySummary {
                kind: ActivityKind::Interaction,
                status: ActivityStatus::Pending,
                requires_attention: true,
                title: "Choose an account".to_owned(),
                finished_at_ms: None,
                ..match &event.body {
                    ServerEventBody::ActivityChanged { activity } => activity.clone(),
                    _ => unreachable!(),
                }
            },
        };
        event
    }

    #[test]
    fn notifications_respect_focus_dedupe_and_exact_deep_links() {
        let mut center = NotificationCenter::default();
        let settings = DesktopSettings::default();
        let event = completed_event(9);
        assert!(center.observe(&event, None, true, &settings).is_none());
        let event = completed_event(10);
        let intent = center
            .observe(&event, Some(Uuid::from_u128(2)), false, &settings)
            .unwrap_or_else(|| panic!("expected a notification"));
        assert_eq!(intent.deep_link.chat_id, Uuid::from_u128(4));
        assert_eq!(intent.deep_link.activity_id, Some(Uuid::from_u128(5)));
        assert!(center.observe(&event, None, false, &settings).is_none());
        let input = center
            .observe(&input_event(11), Some(Uuid::from_u128(2)), false, &settings)
            .unwrap_or_else(|| panic!("expected an input notification"));
        assert_eq!(input.kind, NotificationKind::NeedsInput);
        assert_eq!(input.title, "Input needed");
        assert_eq!(input.deep_link.activity_id, Some(Uuid::from_u128(5)));
    }
}
