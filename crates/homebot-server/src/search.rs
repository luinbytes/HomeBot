//! Owner-authorised global search with immutable navigation targets.

use super::{AppState, bots::ApiError};
use axum::{
    Json,
    extract::{Query, State},
};
use homebot_protocol::{GlobalSearchResponse, SearchResultKind, SearchResultSummary, SearchStatus};
use homebot_storage::{SearchRecord, SearchRecordKind};
use serde::Deserialize;
use std::fmt::Write;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: u32,
}

const fn default_limit() -> u32 {
    40
}

pub(super) async fn global(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<GlobalSearchResponse>, ApiError> {
    let normalized = query.q.trim();
    if normalized.chars().count() > 200 {
        return Err(ApiError::validation("Search query is too long"));
    }
    if query.limit == 0 || query.limit > 100 {
        return Err(ApiError::validation(
            "Search result limit must be from 1 to 100",
        ));
    }
    if !normalized.is_empty() {
        let terms = normalized
            .split(|character: char| !character.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .collect::<Vec<_>>();
        if terms.is_empty() || terms.len() > 8 || terms.iter().any(|term| term.chars().count() < 2)
        {
            return Err(ApiError::validation(
                "Search needs one to eight terms of at least two characters",
            ));
        }
    }
    let (status, results) = match state
        .storage
        .search(state.owner_id, normalized, query.limit)
        .await
    {
        Ok(records) => (
            SearchStatus::Ready,
            records.into_iter().map(summary).collect(),
        ),
        Err(_) => (SearchStatus::Unavailable, Vec::new()),
    };
    Ok(Json(GlobalSearchResponse {
        query: normalized.to_owned(),
        status,
        results,
    }))
}

fn summary(record: SearchRecord) -> SearchResultSummary {
    let deep_link = if let Some(routine_id) = record.routine_id {
        format!("homebot://routine/{routine_id}")
    } else if let Some(chat_id) = record.chat_id {
        let mut target = format!("homebot://chat/{chat_id}");
        if let Some(message_id) = record.message_id {
            let _ = write!(target, "?message={message_id}");
            if let Some(artifact_id) = record.artifact_id {
                let _ = write!(target, "&artifact={artifact_id}");
            }
        } else if let Some(artifact_id) = record.artifact_id {
            let _ = write!(target, "?artifact={artifact_id}");
        }
        target
    } else {
        "homebot://search".to_owned()
    };
    SearchResultSummary {
        kind: match record.kind {
            SearchRecordKind::Message => SearchResultKind::Message,
            SearchRecordKind::File => SearchResultKind::File,
            SearchRecordKind::Link => SearchResultKind::Link,
            SearchRecordKind::Routine => SearchResultKind::Routine,
        },
        title: record.title,
        snippet: record.snippet,
        deep_link,
        chat_id: record.chat_id,
        message_id: record.message_id,
        artifact_id: record.artifact_id,
        routine_id: record.routine_id,
        created_at_ms: record.created_at_ms,
    }
}
