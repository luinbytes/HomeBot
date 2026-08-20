//! Provider-neutral, versioned Skill definitions and deterministic prompt assembly.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_INSTRUCTIONS_BYTES: usize = 32 * 1_024;
const MAX_CONTEXT_ITEMS: usize = 64;
const MAX_CONTEXT_BYTES: usize = 256 * 1_024;
const MAX_TOOL_REFERENCES: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillDefinition {
    pub instructions: String,
    #[serde(default)]
    pub context: Vec<SkillContext>,
    #[serde(default)]
    pub tools: Vec<SkillToolReference>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillContext {
    pub label: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillToolReference {
    pub plugin_name: String,
    pub tool_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedSkill {
    pub skill_id: Uuid,
    pub version_id: Uuid,
    pub name: String,
    pub version: u32,
    pub definition: SkillDefinition,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("Skill instructions are required and must fit within the size limit")]
    InvalidInstructions,
    #[error("Skill context is invalid or exceeds the size limit")]
    InvalidContext,
    #[error("Skill tool references are invalid or exceed the limit")]
    InvalidTools,
    #[error("Assembled Skill instructions exceed the size limit")]
    AssemblyTooLarge,
}

/// Validates a bounded, portable Skill bundle.
///
/// # Errors
/// Returns a stable validation error for blank, oversized, duplicate, or malformed data.
pub fn validate(definition: &SkillDefinition) -> Result<(), SkillError> {
    if definition.instructions.trim().is_empty()
        || definition.instructions.len() > MAX_INSTRUCTIONS_BYTES
    {
        return Err(SkillError::InvalidInstructions);
    }
    if definition.context.len() > MAX_CONTEXT_ITEMS
        || definition
            .context
            .iter()
            .map(|item| item.content.len())
            .sum::<usize>()
            > MAX_CONTEXT_BYTES
        || definition
            .context
            .iter()
            .any(|item| !valid_name(&item.label, 128) || item.content.len() > MAX_CONTEXT_BYTES)
    {
        return Err(SkillError::InvalidContext);
    }
    if definition.tools.len() > MAX_TOOL_REFERENCES
        || definition
            .tools
            .iter()
            .any(|tool| !valid_identifier(&tool.plugin_name) || !valid_identifier(&tool.tool_name))
    {
        return Err(SkillError::InvalidTools);
    }
    let mut contexts = definition
        .context
        .iter()
        .map(|item| item.label.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    contexts.sort_unstable();
    if contexts.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(SkillError::InvalidContext);
    }
    let mut tools = definition
        .tools
        .iter()
        .map(|tool| {
            format!(
                "{}.{}",
                tool.plugin_name.to_ascii_lowercase(),
                tool.tool_name.to_ascii_lowercase()
            )
        })
        .collect::<Vec<_>>();
    tools.sort_unstable();
    if tools.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(SkillError::InvalidTools);
    }
    Ok(())
}

/// Produces the same provider-neutral instruction block for the same applied versions.
///
/// Skills, context items, and tool references are sorted independently of database/API order.
/// Tool references describe intended tools but do not grant plugin capability authority.
///
/// # Errors
/// Returns a validation or aggregate-size error.
pub fn assemble(skills: &[AppliedSkill]) -> Result<String, SkillError> {
    let mut skills = skills.to_vec();
    for skill in &skills {
        validate(&skill.definition)?;
    }
    skills.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.skill_id.cmp(&right.skill_id))
            .then_with(|| left.version.cmp(&right.version))
    });
    let mut output = String::new();
    for skill in skills {
        output.push_str("## Skill: ");
        output.push_str(skill.name.trim());
        output.push_str(" (version ");
        output.push_str(&skill.version.to_string());
        output.push_str(")\n");
        output.push_str(skill.definition.instructions.trim());
        output.push('\n');
        let mut context = skill.definition.context;
        context.sort_by_key(|item| item.label.to_ascii_lowercase());
        for item in context {
            output.push_str("### Context: ");
            output.push_str(item.label.trim());
            output.push('\n');
            output.push_str(item.content.trim());
            output.push('\n');
        }
        let mut tools = skill.definition.tools;
        tools.sort_by(|left, right| {
            left.plugin_name
                .to_ascii_lowercase()
                .cmp(&right.plugin_name.to_ascii_lowercase())
                .then_with(|| {
                    left.tool_name
                        .to_ascii_lowercase()
                        .cmp(&right.tool_name.to_ascii_lowercase())
                })
        });
        if !tools.is_empty() {
            output.push_str("### Tool references (capability policy still applies)\n");
            for tool in tools {
                output.push_str("- ");
                output.push_str(&tool.plugin_name);
                output.push('.');
                output.push_str(&tool.tool_name);
                output.push('\n');
            }
        }
    }
    if output.len() > MAX_CONTEXT_BYTES + MAX_INSTRUCTIONS_BYTES {
        return Err(SkillError::AssemblyTooLarge);
    }
    Ok(output)
}

fn valid_name(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, id: Uuid) -> AppliedSkill {
        AppliedSkill {
            skill_id: id,
            version_id: Uuid::now_v7(),
            name: name.to_owned(),
            version: 2,
            definition: SkillDefinition {
                instructions: format!("Apply {name}"),
                context: vec![SkillContext {
                    label: "Guide".to_owned(),
                    content: "Use the project conventions.".to_owned(),
                }],
                tools: vec![SkillToolReference {
                    plugin_name: "Repository".to_owned(),
                    tool_name: "status".to_owned(),
                }],
            },
        }
    }

    #[test]
    fn assembly_is_order_independent_and_preserves_versions() -> Result<(), SkillError> {
        let alpha = skill("Alpha", Uuid::now_v7());
        let beta = skill("Beta", Uuid::now_v7());
        let forward = assemble(&[alpha.clone(), beta.clone()])?;
        let reverse = assemble(&[beta, alpha])?;
        assert_eq!(forward, reverse);
        assert!(forward.contains("Alpha (version 2)"));
        assert!(forward.contains("capability policy still applies"));
        Ok(())
    }

    #[test]
    fn duplicate_context_and_tools_are_rejected() {
        let mut applied = skill("Alpha", Uuid::now_v7());
        applied.definition.context.push(SkillContext {
            label: "guide".to_owned(),
            content: "Duplicate".to_owned(),
        });
        assert!(matches!(
            validate(&applied.definition),
            Err(SkillError::InvalidContext)
        ));
    }
}
