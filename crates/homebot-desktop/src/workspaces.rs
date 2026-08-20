//! Server-authoritative repository and per-chat workspace projection.

use homebot_protocol::{
    ChatWorkspaceSummary, RepositoryWorkspaceSummary, ServerEvent, ServerEventBody, WorkspaceMode,
};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct WorkspaceProjection {
    repositories: BTreeMap<Uuid, RepositoryWorkspaceSummary>,
    chats: BTreeMap<Uuid, ChatWorkspaceSummary>,
}

#[derive(Clone, Debug)]
pub enum WorkspaceCommand {
    RegisterRepository {
        root_path: String,
        name: Option<String>,
    },
    Attach {
        chat_id: Uuid,
        workspace_id: Uuid,
        mode: WorkspaceMode,
        base_ref: Option<String>,
        branch_name: Option<String>,
    },
    Detach {
        chat_id: Uuid,
    },
    LoadBranches {
        workspace_id: Uuid,
    },
}

impl WorkspaceProjection {
    pub fn hydrate(
        &mut self,
        repositories: Vec<RepositoryWorkspaceSummary>,
        chats: Vec<ChatWorkspaceSummary>,
    ) {
        self.repositories = repositories
            .into_iter()
            .map(|workspace| (workspace.id, workspace))
            .collect();
        self.chats = chats
            .into_iter()
            .map(|workspace| (workspace.chat_id, workspace))
            .collect();
    }

    pub fn apply(&mut self, event: &ServerEvent) {
        match &event.body {
            ServerEventBody::RepositoryWorkspaceChanged { workspace } => {
                self.repositories.insert(workspace.id, workspace.clone());
            }
            ServerEventBody::ChatWorkspaceChanged { workspace } => {
                self.chats.insert(workspace.chat_id, workspace.clone());
            }
            ServerEventBody::ChatWorkspaceRemoved { chat_id } => {
                self.chats.remove(chat_id);
            }
            _ => {}
        }
    }

    pub fn repositories(&self) -> impl Iterator<Item = &RepositoryWorkspaceSummary> {
        self.repositories.values()
    }

    pub fn apply_repository(&mut self, workspace: RepositoryWorkspaceSummary) {
        self.repositories.insert(workspace.id, workspace);
    }

    pub fn apply_chat(&mut self, workspace: ChatWorkspaceSummary) {
        self.chats.insert(workspace.chat_id, workspace);
    }

    pub fn remove_chat(&mut self, chat_id: Uuid) {
        self.chats.remove(&chat_id);
    }

    #[must_use]
    pub fn for_chat(&self, chat_id: Uuid) -> Option<&ChatWorkspaceSummary> {
        self.chats.get(&chat_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homebot_protocol::{PROTOCOL_VERSION, WorkingTreeCondition, WorkspaceMode};

    #[test]
    fn snapshot_and_events_replace_only_server_owned_workspace_state() {
        let repository_id = Uuid::now_v7();
        let chat_id = Uuid::now_v7();
        let repository = RepositoryWorkspaceSummary {
            id: repository_id,
            name: "HomeBot".to_owned(),
            root_path: "/repo".to_owned(),
            current_branch: Some("main".to_owned()),
            condition: WorkingTreeCondition::Dirty,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let workspace = ChatWorkspaceSummary {
            chat_id,
            workspace_id: repository_id,
            mode: WorkspaceMode::Isolated,
            effective_path: "/managed/chat".to_owned(),
            branch_name: Some("homebot/chat".to_owned()),
            base_ref: Some("main".to_owned()),
            condition: WorkingTreeCondition::Clean,
            updated_at_unix_ms: 2,
        };
        let mut projection = WorkspaceProjection::default();
        projection.hydrate(vec![repository], vec![workspace]);
        assert!(projection.for_chat(chat_id).is_some());
        projection.apply(&ServerEvent {
            protocol_version: PROTOCOL_VERSION,
            sequence: 3,
            event_id: Uuid::now_v7(),
            body: ServerEventBody::ChatWorkspaceRemoved { chat_id },
        });
        assert!(projection.for_chat(chat_id).is_none());
        assert_eq!(projection.repositories().count(), 1);
    }
}
