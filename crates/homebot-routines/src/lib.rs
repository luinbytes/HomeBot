//! Versioned routine definitions, demonstration recording, and deterministic replay.

mod schedule;

pub use schedule::{
    MissedRunPolicy, OverlapPolicy, RetryPolicy, RoutineSchedule, RoutineTriggerDefinition,
    RoutineTriggerSource, ScheduleError, due_occurrences, next_occurrence,
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineInputKind {
    Text,
    Number,
    Boolean,
    FileReference,
    SecretReference,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineInput {
    pub key: String,
    pub label: String,
    pub kind: RoutineInputKind,
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedOutput {
    pub key: String,
    pub description: String,
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutineStep {
    BotPrompt {
        bot_id: Uuid,
        prompt_template: String,
        requires_approval: bool,
    },
    PluginTool {
        plugin_id: Uuid,
        tool_name: String,
        arguments: Value,
        requires_approval: bool,
    },
    RecordOutput {
        output_key: String,
        value_template: String,
    },
}

impl RoutineStep {
    #[must_use]
    pub const fn requires_approval(&self) -> bool {
        match self {
            Self::BotPrompt {
                requires_approval, ..
            }
            | Self::PluginTool {
                requires_approval, ..
            } => *requires_approval,
            Self::RecordOutput { .. } => false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineDefinition {
    pub inputs: Vec<RoutineInput>,
    pub steps: Vec<RoutineStep>,
    pub expected_outputs: Vec<ExpectedOutput>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedAction {
    pub actor: RecordedActor,
    pub step: RoutineStep,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedActor {
    User,
    Bot,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineExecutionResult {
    pub step_index: u32,
    pub status: RoutineStepStatus,
    pub output: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineStepStatus {
    Planned,
    Succeeded,
    ApprovalRequired,
    Failed,
}

#[derive(Debug, thiserror::Error)]
pub enum RoutineError {
    #[error("routine must contain at least one structured step")]
    Empty,
    #[error("routine definition is invalid: {0}")]
    Invalid(&'static str),
    #[error("routine step failed")]
    StepFailed,
}

/// Validates bounded structured routine data.
///
/// # Errors
///
/// Returns a stable validation error for empty, oversized, or malformed definitions.
pub fn validate(definition: &RoutineDefinition) -> Result<(), RoutineError> {
    if definition.steps.is_empty() || definition.steps.len() > 256 {
        return Err(RoutineError::Empty);
    }
    let valid_key = |value: &str| {
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    };
    if definition.inputs.iter().any(|input| !valid_key(&input.key))
        || definition
            .expected_outputs
            .iter()
            .any(|output| !valid_key(&output.key))
    {
        return Err(RoutineError::Invalid("input/output key"));
    }
    for step in &definition.steps {
        match step {
            RoutineStep::BotPrompt {
                prompt_template, ..
            } if prompt_template.trim().is_empty() || prompt_template.len() > 32_768 => {
                return Err(RoutineError::Invalid("Bot prompt"));
            }
            RoutineStep::PluginTool {
                tool_name,
                arguments,
                ..
            } if tool_name.is_empty()
                || tool_name.len() > 128
                || !tool_name.is_ascii()
                || !arguments.is_object() =>
            {
                return Err(RoutineError::Invalid("plugin tool"));
            }
            RoutineStep::RecordOutput {
                output_key,
                value_template,
            } if !valid_key(output_key)
                || value_template.len() > 32_768
                || !definition
                    .expected_outputs
                    .iter()
                    .any(|output| output.key == *output_key) =>
            {
                return Err(RoutineError::Invalid("recorded output"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Converts recorded structured actions into an editable draft definition.
///
/// # Errors
///
/// Returns a validation error when no usable actions were recorded.
pub fn definition_from_recording(
    actions: Vec<RecordedAction>,
) -> Result<RoutineDefinition, RoutineError> {
    let expected_outputs = actions
        .iter()
        .filter_map(|action| match &action.step {
            RoutineStep::RecordOutput { output_key, .. } => Some(ExpectedOutput {
                key: output_key.clone(),
                description: "Captured during demonstration".to_owned(),
                required: true,
            }),
            _ => None,
        })
        .collect();
    let definition = RoutineDefinition {
        inputs: Vec::new(),
        steps: actions.into_iter().map(|action| action.step).collect(),
        expected_outputs,
    };
    validate(&definition)?;
    Ok(definition)
}

#[async_trait]
pub trait RoutineActionExecutor: Send + Sync {
    async fn validate_step(&self, step: &RoutineStep, inputs: &Value) -> Result<(), RoutineError>;
    /// Returns whether authoritative policy requires an approval before this
    /// step can run. The default preserves definition-authored behavior.
    async fn approval_required(
        &self,
        step: &RoutineStep,
        _inputs: &Value,
    ) -> Result<bool, RoutineError> {
        Ok(step.requires_approval())
    }
    async fn execute_step(&self, step: &RoutineStep, inputs: &Value)
    -> Result<Value, RoutineError>;
}

/// Validates then plans or sequentially executes an immutable routine version.
///
/// # Errors
///
/// Returns validation or executor errors and stops before later steps.
pub async fn replay(
    executor: &dyn RoutineActionExecutor,
    definition: &RoutineDefinition,
    inputs: &Value,
    dry_run: bool,
) -> Result<Vec<RoutineExecutionResult>, RoutineError> {
    validate(definition)?;
    if !inputs.is_object() {
        return Err(RoutineError::Invalid("run inputs"));
    }
    validate_inputs(definition, inputs)?;
    let mut results = Vec::with_capacity(definition.steps.len());
    for (index, step) in definition.steps.iter().enumerate() {
        executor.validate_step(step, inputs).await?;
        let step_index = u32::try_from(index).unwrap_or(u32::MAX);
        if dry_run {
            results.push(RoutineExecutionResult {
                step_index,
                status: RoutineStepStatus::Planned,
                output: None,
            });
        } else if executor.approval_required(step, inputs).await? {
            results.push(RoutineExecutionResult {
                step_index,
                status: RoutineStepStatus::ApprovalRequired,
                output: None,
            });
            break;
        } else {
            let output = executor.execute_step(step, inputs).await?;
            results.push(RoutineExecutionResult {
                step_index,
                status: RoutineStepStatus::Succeeded,
                output: Some(output),
            });
        }
    }
    Ok(results)
}

fn validate_inputs(definition: &RoutineDefinition, inputs: &Value) -> Result<(), RoutineError> {
    let object = inputs
        .as_object()
        .ok_or(RoutineError::Invalid("run inputs"))?;
    if object
        .keys()
        .any(|key| !definition.inputs.iter().any(|input| input.key == *key))
    {
        return Err(RoutineError::Invalid("unknown run input"));
    }
    for input in &definition.inputs {
        let Some(value) = object.get(&input.key) else {
            if input.required {
                return Err(RoutineError::Invalid("required run input"));
            }
            continue;
        };
        let valid = match input.kind {
            RoutineInputKind::Text | RoutineInputKind::FileReference => {
                value.as_str().is_some_and(|text| text.len() <= 32_768)
            }
            RoutineInputKind::SecretReference => value
                .as_str()
                .is_some_and(|text| Uuid::parse_str(text).is_ok()),
            RoutineInputKind::Number => value.is_number(),
            RoutineInputKind::Boolean => value.is_boolean(),
        };
        if !valid {
            return Err(RoutineError::Invalid("run input type"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixtureExecutor(AtomicUsize);
    #[async_trait]
    impl RoutineActionExecutor for FixtureExecutor {
        async fn validate_step(
            &self,
            _step: &RoutineStep,
            _inputs: &Value,
        ) -> Result<(), RoutineError> {
            Ok(())
        }
        async fn execute_step(
            &self,
            _step: &RoutineStep,
            _inputs: &Value,
        ) -> Result<Value, RoutineError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"redacted":true}))
        }
    }

    fn definition() -> RoutineDefinition {
        RoutineDefinition {
            inputs: Vec::new(),
            steps: vec![
                RoutineStep::BotPrompt {
                    bot_id: Uuid::nil(),
                    prompt_template: "Summarise {{topic}}".to_owned(),
                    requires_approval: false,
                },
                RoutineStep::PluginTool {
                    plugin_id: Uuid::nil(),
                    tool_name: "publish".to_owned(),
                    arguments: serde_json::json!({}),
                    requires_approval: true,
                },
            ],
            expected_outputs: Vec::new(),
        }
    }

    #[tokio::test]
    async fn dry_run_has_no_side_effects_and_manual_run_stops_for_approval()
    -> Result<(), RoutineError> {
        let executor = FixtureExecutor(AtomicUsize::new(0));
        let planned = replay(&executor, &definition(), &serde_json::json!({}), true).await?;
        assert!(
            planned
                .iter()
                .all(|step| step.status == RoutineStepStatus::Planned)
        );
        assert_eq!(executor.0.load(Ordering::SeqCst), 0);
        let run = replay(&executor, &definition(), &serde_json::json!({}), false).await?;
        assert_eq!(executor.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            run.last().map(|step| step.status),
            Some(RoutineStepStatus::ApprovalRequired)
        );
        Ok(())
    }

    #[test]
    fn recording_preserves_structured_steps_without_coordinate_actions() -> Result<(), RoutineError>
    {
        let recorded = definition_from_recording(vec![RecordedAction {
            actor: RecordedActor::User,
            step: definition().steps[0].clone(),
        }])?;
        assert!(matches!(recorded.steps[0], RoutineStep::BotPrompt { .. }));
        assert!(
            !serde_json::to_string(&recorded)
                .unwrap_or_default()
                .contains("coordinate")
        );
        Ok(())
    }

    #[tokio::test]
    async fn recording_preserves_approval_boundaries_across_replay() -> Result<(), RoutineError> {
        let definition = definition_from_recording(vec![RecordedAction {
            actor: RecordedActor::Bot,
            step: RoutineStep::BotPrompt {
                bot_id: Uuid::nil(),
                prompt_template: "Publish the reviewed result".to_owned(),
                requires_approval: true,
            },
        }])?;
        assert!(definition.steps[0].requires_approval());
        let executor = FixtureExecutor(AtomicUsize::new(0));
        let result = replay(&executor, &definition, &serde_json::json!({}), false).await?;
        assert_eq!(
            result.first().map(|step| step.status),
            Some(RoutineStepStatus::ApprovalRequired)
        );
        assert_eq!(executor.0.load(Ordering::SeqCst), 0);
        Ok(())
    }
}
