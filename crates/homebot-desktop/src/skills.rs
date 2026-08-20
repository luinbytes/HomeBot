//! Read-only projection of the authoritative server Skill library.

use homebot_protocol::{ServerEvent, ServerEventBody, SkillSummary};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct SkillProjection {
    skills: BTreeMap<Uuid, SkillSummary>,
}

impl SkillProjection {
    pub fn hydrate(&mut self, skills: Vec<SkillSummary>) {
        self.skills = skills.into_iter().map(|skill| (skill.id, skill)).collect();
    }

    pub fn apply(&mut self, event: &ServerEvent) {
        match &event.body {
            ServerEventBody::SkillChanged { skill } => {
                self.skills.insert(skill.id, skill.clone());
            }
            ServerEventBody::SkillRemoved { skill_id } => {
                self.skills.remove(skill_id);
            }
            _ => {}
        }
    }

    pub fn skills(&self) -> impl Iterator<Item = &SkillSummary> {
        self.skills.values()
    }

    #[must_use]
    pub fn assigned_to(&self, bot_id: Uuid) -> Vec<&SkillSummary> {
        self.skills
            .values()
            .filter(|skill| skill.bot_ids.contains(&bot_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homebot_protocol::{PROTOCOL_VERSION, SkillDefinition};

    fn skill(id: Uuid) -> SkillSummary {
        SkillSummary {
            id,
            name: "Review".to_owned(),
            description: String::new(),
            active_version_id: Uuid::now_v7(),
            version: 1,
            definition: SkillDefinition {
                instructions: "Review carefully".to_owned(),
                context: Vec::new(),
                tools: Vec::new(),
            },
            bot_ids: Vec::new(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn event(body: ServerEventBody) -> ServerEvent {
        ServerEvent {
            protocol_version: PROTOCOL_VERSION,
            sequence: 1,
            event_id: Uuid::now_v7(),
            body,
        }
    }

    #[test]
    fn projection_hydrates_updates_and_removes_server_state() {
        let id = Uuid::now_v7();
        let mut projection = SkillProjection::default();
        projection.hydrate(vec![skill(id)]);
        let mut changed = skill(id);
        changed.version = 2;
        projection.apply(&event(ServerEventBody::SkillChanged { skill: changed }));
        assert_eq!(
            projection.skills().next().map(|skill| skill.version),
            Some(2)
        );
        projection.apply(&event(ServerEventBody::SkillRemoved { skill_id: id }));
        assert_eq!(projection.skills().count(), 0);
    }
}
