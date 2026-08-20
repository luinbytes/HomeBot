use homebot_protocol::{BotColor, BotPermissionProfile, BotProviderStatus, BotShape, BotSummary};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Connected,
    Connecting,
    Disconnected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BotEditorDraft {
    pub bot_id: Option<Uuid>,
    pub name: String,
    pub title: String,
    pub description: String,
    pub shape: BotShape,
    pub color: BotColor,
    pub provider_profile_id: Option<Uuid>,
    pub permission_profile: BotPermissionProfile,
}

impl Default for BotEditorDraft {
    fn default() -> Self {
        Self {
            bot_id: None,
            name: String::new(),
            title: String::new(),
            description: String::new(),
            shape: BotShape::RoundedSquare,
            color: BotColor::Violet,
            provider_profile_id: None,
            permission_profile: BotPermissionProfile::AskBeforeChanges,
        }
    }
}

impl BotEditorDraft {
    #[must_use]
    pub fn for_bot(bot: &BotSummary) -> Self {
        Self {
            bot_id: Some(bot.id),
            name: bot.name.clone(),
            title: bot.title.clone(),
            description: bot.description.clone(),
            shape: bot.shape,
            color: bot.color,
            provider_profile_id: bot.advanced.provider_profile_id,
            permission_profile: bot.advanced.permission_profile,
        }
    }

    /// Validates obvious identity errors before server submission.
    ///
    /// # Errors
    ///
    /// Returns the first local field or duplicate-name error.
    pub fn validate(&self, roster: &[BotSummary]) -> Result<(), EditorError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(EditorError::EmptyName);
        }
        if name.chars().count() > 48 {
            return Err(EditorError::NameTooLong);
        }
        if self.title.trim().chars().count() > 80 {
            return Err(EditorError::TitleTooLong);
        }
        if roster
            .iter()
            .any(|bot| Some(bot.id) != self.bot_id && bot.name.trim().eq_ignore_ascii_case(name))
        {
            return Err(EditorError::DuplicateName);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorError {
    EmptyName,
    NameTooLong,
    TitleTooLong,
    DuplicateName,
}

#[derive(Clone, Debug)]
pub struct BotRosterModel {
    pub bots: Vec<BotSummary>,
    pub selected: Option<Uuid>,
    pub connection: ConnectionState,
    pub editor: Option<BotEditorDraft>,
    pub show_archived: bool,
}

impl Default for BotRosterModel {
    fn default() -> Self {
        Self {
            bots: Vec::new(),
            selected: None,
            connection: ConnectionState::Connecting,
            editor: None,
            show_archived: false,
        }
    }
}

impl BotRosterModel {
    pub fn apply_snapshot(&mut self, bots: Vec<BotSummary>) {
        self.bots = bots;
        self.connection = ConnectionState::Connected;
        if self
            .selected
            .is_some_and(|id| !self.bots.iter().any(|bot| bot.id == id))
        {
            self.selected = None;
        }
    }

    pub fn apply_change(&mut self, changed: BotSummary) {
        if let Some(existing) = self.bots.iter_mut().find(|bot| bot.id == changed.id) {
            *existing = changed;
        } else {
            self.bots.push(changed);
        }
        self.bots.sort_by_key(|bot| bot.name.to_lowercase());
    }

    #[must_use]
    pub fn visible_bots(&self) -> Vec<&BotSummary> {
        self.bots
            .iter()
            .filter(|bot| self.show_archived || !bot.archived)
            .collect()
    }

    #[must_use]
    pub fn provider_warning(&self, bot_id: Uuid) -> bool {
        self.bots
            .iter()
            .find(|bot| bot.id == bot_id)
            .is_some_and(|bot| bot.provider == BotProviderStatus::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homebot_protocol::{BotAdvancedSettings, BotAttention};

    fn bot(id: Uuid, name: &str) -> BotSummary {
        BotSummary {
            id,
            name: name.to_owned(),
            title: "Helper".to_owned(),
            description: String::new(),
            shape: BotShape::RoundedSquare,
            color: BotColor::Violet,
            archived: false,
            unread_count: 0,
            attention: BotAttention::None,
            provider: BotProviderStatus::NotConfigured,
            advanced: BotAdvancedSettings {
                provider_profile_id: None,
                permission_profile: BotPermissionProfile::AskBeforeChanges,
            },
        }
    }

    #[test]
    fn editor_rejects_duplicate_names_and_roster_tracks_archive() {
        let first = bot(Uuid::now_v7(), "Nova");
        let mut model = BotRosterModel::default();
        model.apply_snapshot(vec![first.clone()]);
        let draft = BotEditorDraft {
            name: " nova ".to_owned(),
            ..BotEditorDraft::default()
        };
        assert_eq!(draft.validate(&model.bots), Err(EditorError::DuplicateName));
        let mut archived = first;
        archived.archived = true;
        model.apply_change(archived);
        assert!(model.visible_bots().is_empty());
        model.show_archived = true;
        assert_eq!(model.visible_bots().len(), 1);
    }
}
