//! Server-authoritative repository and per-chat workspace projection.

use homebot_protocol::{
    ChatWorkspaceSummary, PullRequestMetadata, PullRequestMutationResponse,
    RepositoryWorkspaceSummary, ServerEvent, ServerEventBody, VcsRemoteMutationResponse, VcsStatus,
    WorkingTreeDiffResponse, WorkspaceMode,
};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct WorkspaceProjection {
    repositories: BTreeMap<Uuid, RepositoryWorkspaceSummary>,
    chats: BTreeMap<Uuid, ChatWorkspaceSummary>,
    vcs: BTreeMap<Uuid, VcsStatus>,
    diffs: BTreeMap<(Uuid, bool), WorkingTreeDiffResponse>,
    remote_mutations: BTreeMap<Uuid, VcsRemoteMutationResponse>,
    pull_requests: BTreeMap<Uuid, PullRequestMetadata>,
    pull_request_mutations: BTreeMap<Uuid, PullRequestMutationResponse>,
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
    LoadStatus {
        chat_id: Uuid,
    },
    LoadDiff {
        chat_id: Uuid,
        staged: bool,
    },
    Commit {
        chat_id: Uuid,
        message: String,
        stage_all: bool,
    },
    CreateBranch {
        chat_id: Uuid,
        branch: String,
        start_point: Option<String>,
    },
    Push {
        chat_id: Uuid,
        request_id: Uuid,
        idempotency_key: Uuid,
        remote: String,
        branch: String,
        set_upstream: bool,
        approval_id: Option<Uuid>,
    },
    LoadPullRequest {
        chat_id: Uuid,
        remote: String,
        head_branch: String,
        base_branch: String,
    },
    CreatePullRequest {
        chat_id: Uuid,
        request_id: Uuid,
        idempotency_key: Uuid,
        remote: String,
        head_branch: String,
        base_branch: String,
        title: String,
        body: String,
        draft: bool,
        approval_id: Option<Uuid>,
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
                self.vcs.remove(chat_id);
                self.diffs.retain(|(candidate, _), _| candidate != chat_id);
                self.remote_mutations.remove(chat_id);
                self.pull_requests.remove(chat_id);
                self.pull_request_mutations.remove(chat_id);
            }
            ServerEventBody::VcsStatusChanged { chat_id, status } => {
                self.vcs.insert(*chat_id, status.clone());
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
        self.vcs.remove(&chat_id);
        self.diffs.retain(|(candidate, _), _| *candidate != chat_id);
        self.remote_mutations.remove(&chat_id);
        self.pull_requests.remove(&chat_id);
        self.pull_request_mutations.remove(&chat_id);
    }

    #[must_use]
    pub fn for_chat(&self, chat_id: Uuid) -> Option<&ChatWorkspaceSummary> {
        self.chats.get(&chat_id)
    }

    pub fn apply_vcs_status(&mut self, chat_id: Uuid, status: VcsStatus) {
        self.vcs.insert(chat_id, status);
    }

    pub fn apply_vcs_diff(&mut self, chat_id: Uuid, diff: WorkingTreeDiffResponse) {
        self.diffs.insert((chat_id, diff.staged), diff);
    }

    pub fn apply_remote_mutation(&mut self, chat_id: Uuid, response: VcsRemoteMutationResponse) {
        self.remote_mutations.insert(chat_id, response);
    }

    #[must_use]
    pub fn vcs_status(&self, chat_id: Uuid) -> Option<&VcsStatus> {
        self.vcs.get(&chat_id)
    }

    #[must_use]
    pub fn vcs_diff(&self, chat_id: Uuid, staged: bool) -> Option<&WorkingTreeDiffResponse> {
        self.diffs.get(&(chat_id, staged))
    }

    #[must_use]
    pub fn remote_mutation(&self, chat_id: Uuid) -> Option<&VcsRemoteMutationResponse> {
        self.remote_mutations.get(&chat_id)
    }

    pub fn apply_pull_request(&mut self, chat_id: Uuid, metadata: PullRequestMetadata) {
        self.pull_requests.insert(chat_id, metadata);
    }

    pub fn apply_pull_request_mutation(
        &mut self,
        chat_id: Uuid,
        response: PullRequestMutationResponse,
    ) {
        if let Some(result) = response.result.clone()
            && let Some(metadata) = self.pull_requests.get_mut(&chat_id)
        {
            metadata.current = Some(result);
        }
        self.pull_request_mutations.insert(chat_id, response);
    }

    #[must_use]
    pub fn pull_request(&self, chat_id: Uuid) -> Option<&PullRequestMetadata> {
        self.pull_requests.get(&chat_id)
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

    #[test]
    fn vcs_projection_changes_only_from_server_responses_and_events() {
        let chat_id = Uuid::now_v7();
        let status = homebot_protocol::VcsStatus {
            head_oid: Some("0123456789012345678901234567890123456789".to_owned()),
            branch: Some("homebot/change".to_owned()),
            detached: false,
            upstream: Some("origin/homebot/change".to_owned()),
            ahead: 1,
            behind: 0,
            conflicted: false,
            entries: vec![homebot_protocol::VcsStatusEntry {
                path: "README.md".to_owned(),
                previous_path: None,
                staged: None,
                unstaged: Some(homebot_protocol::VcsChangeKind::Modified),
                conflicted: false,
            }],
            remotes: vec![homebot_protocol::VcsRemoteSummary {
                name: "origin".to_owned(),
                fetch_configured: true,
                push_configured: true,
            }],
        };
        let mut projection = WorkspaceProjection::default();
        projection.apply(&ServerEvent {
            protocol_version: PROTOCOL_VERSION,
            sequence: 1,
            event_id: Uuid::now_v7(),
            body: ServerEventBody::VcsStatusChanged {
                chat_id,
                status: status.clone(),
            },
        });
        assert_eq!(projection.vcs_status(chat_id), Some(&status));
        projection.apply_vcs_diff(
            chat_id,
            homebot_protocol::WorkingTreeDiffResponse {
                staged: false,
                patch: "diff --git".to_owned(),
                files: Vec::new(),
            },
        );
        assert_eq!(
            projection
                .vcs_diff(chat_id, false)
                .map(|diff| diff.patch.as_str()),
            Some("diff --git")
        );
    }
}
