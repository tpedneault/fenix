//! The session's modes, config options, commands and permission options,
//! shaped as claude-agent-acp 0.99.1 builds them (`session-mode.ts`,
//! `buildConfigOptions`, `permissions/options/*`).

use serde_json::{json, Value};

/// What a session is set to. Kept in the stored record, so a resumed or
/// loaded session continues in the mode it had, as Claude's transcript does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Settings {
    pub mode: String,
    pub model: String,
    pub effort: String,
    pub fast: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: "default".into(),
            model: "default".into(),
            effort: "default".into(),
            fast: false,
        }
    }
}

const MODELS: [(&str, &str, &str); 4] = [
    (
        "default",
        "Default (recommended)",
        "Opus 5.5 · Most capable for complex work",
    ),
    ("opus", "Opus", "Opus 5.5 · Most capable for complex work"),
    ("sonnet", "Sonnet", "Sonnet 5 · Best for everyday tasks"),
    ("haiku", "Haiku", "Haiku 4.5 · Fastest for quick answers"),
];

const EFFORTS: [&str; 5] = ["default", "low", "medium", "high", "max"];

/// The modes in Claude's order; `bypassPermissions` only on the `--bypass`
/// cue, as the adapter offers it only when allowed.
pub(super) fn modes(bypass: bool) -> Vec<Value> {
    let mut modes = vec![
        json!({"id": "default", "name": "Manual", "description": "Always ask before making changes"}),
        json!({"id": "acceptEdits", "name": "Accept edits", "description": "Automatically accept all file edits"}),
        json!({"id": "plan", "name": "Plan", "description": "Create a plan before making changes"}),
        json!({"id": "auto", "name": "Auto", "description": "Claude handles permission decisions"}),
    ];
    if bypass {
        modes.push(json!({"id": "bypassPermissions", "name": "Bypass permissions", "description": "Accepts all permissions"}));
    }
    modes
}

pub(super) fn is_mode(id: &str, bypass: bool) -> bool {
    modes(bypass).iter().any(|mode| mode["id"] == id)
}

pub(super) fn mode_state(settings: &Settings, bypass: bool) -> Value {
    json!({"currentModeId": settings.mode, "availableModes": modes(bypass)})
}

/// `mode`, `model`, `effort`, `fast`, in that order. `fast` is a boolean
/// only for a client that said it takes them
/// (`clientCapabilities.session.configOptions.boolean`), else an on/off select.
pub(super) fn config_options(settings: &Settings, bypass: bool, boolean: bool) -> Value {
    let mode_options: Vec<Value> = modes(bypass)
        .iter()
        .map(|mode| json!({"value": mode["id"], "name": mode["name"], "description": mode["description"]}))
        .collect();
    let model_options: Vec<Value> = MODELS
        .iter()
        .map(|(value, name, description)| json!({"value": value, "name": name, "description": description}))
        .collect();
    let effort_options: Vec<Value> = EFFORTS
        .iter()
        .map(|value| json!({"value": value, "name": capitalized(value)}))
        .collect();
    let fast_base = json!({
        "id": "fast",
        "name": "Fast mode",
        "description": "Faster responses on supported models",
        "category": "model_config",
    });
    let mut fast = fast_base.as_object().cloned().unwrap_or_default();
    if boolean {
        fast.insert("type".into(), json!("boolean"));
        fast.insert("currentValue".into(), json!(settings.fast));
    } else {
        fast.insert("type".into(), json!("select"));
        fast.insert(
            "currentValue".into(),
            json!(if settings.fast { "on" } else { "off" }),
        );
        fast.insert(
            "options".into(),
            json!([{"value": "on", "name": "On"}, {"value": "off", "name": "Off"}]),
        );
    }
    json!([
        {"id": "mode", "name": "Mode", "description": "Session permission mode", "category": "mode",
         "type": "select", "currentValue": settings.mode, "options": mode_options},
        {"id": "model", "name": "Model", "description": "AI model to use", "category": "model",
         "type": "select", "currentValue": settings.model, "options": model_options},
        {"id": "effort", "name": "Effort", "description": "Available effort levels for this model",
         "category": "thought_level", "type": "select", "currentValue": settings.effort, "options": effort_options},
        Value::Object(fast),
    ])
}

/// Applies `session/set_config_option`, or says why it can't.
pub(super) fn set_option(
    settings: &mut Settings,
    id: &str,
    value: &Value,
    bypass: bool,
) -> Result<(), String> {
    let text = value.as_str();
    let bad = || format!("Invalid value for config option {id}: {value}");
    match id {
        "mode" => match text {
            Some(mode) if is_mode(mode, bypass) => settings.mode = mode.into(),
            _ => return Err(format!("Invalid Mode: {}", text.unwrap_or_default())),
        },
        "model" => match text {
            Some(model) if MODELS.iter().any(|(m, ..)| *m == model) => {
                settings.model = model.into()
            }
            _ => return Err(bad()),
        },
        "effort" => match text {
            Some(effort) if EFFORTS.contains(&effort) => settings.effort = effort.into(),
            _ => return Err(bad()),
        },
        "fast" => {
            settings.fast = match (value.as_bool(), text) {
                (Some(on), _) => on,
                (_, Some("on")) => true,
                (_, Some("off")) => false,
                _ => return Err(bad()),
            }
        }
        _ => return Err(format!("Unknown config option: {id}")),
    }
    Ok(())
}

/// A few of Claude Code's commands, with the input hints the adapter
/// passes on (`argumentHint`), and its own `/mcp`.
pub(super) fn commands() -> Value {
    json!([
        {"name": "compact", "description": "Clear conversation history but keep a summary in context. Optional: /compact [instructions for summarization]",
         "input": {"hint": "<optional custom summarization instructions>"}},
        {"name": "init", "description": "Initialize a new CLAUDE.md file with codebase documentation", "input": null},
        {"name": "review", "description": "Review a pull request", "input": {"hint": "[pr-number]"}},
        {"name": "security-review", "description": "Complete a security review of the pending changes on the current branch", "input": null},
        {"name": "mcp", "description": "Show the MCP servers and their status, or reconnect, enable, or disable a server",
         "input": {"hint": "[reconnect|enable|disable [<server>|all]]"}},
    ])
}

/// Edit and Write: once, for the session, or no.
pub(super) fn edit_options() -> Value {
    json!([
        {"optionId": "allow-once", "name": "Yes", "kind": "allow_once"},
        {"optionId": "allow-with-updates", "name": "Yes, allow all edits during this session", "kind": "allow_always"},
        {"optionId": "reject", "name": "No", "kind": "reject_once"},
    ])
}

/// Bash: the "always" choice names the command's first word, as Claude's
/// prefix rules do.
pub(super) fn bash_options(prefix: &str) -> Value {
    json!([
        {"optionId": "allow-once", "name": "Yes", "kind": "allow_once"},
        {"optionId": "allow-with-updates", "name": format!("Yes, and don't ask again for {prefix} commands"), "kind": "allow_always"},
        {"optionId": "reject", "name": "No", "kind": "reject_once"},
    ])
}

/// ExitPlanMode's choices; each `exit-plan-*` names the mode it switches to.
pub(super) fn exit_plan_options(bypass: bool) -> Value {
    let mut options = vec![
        json!({"optionId": "exit-plan-auto", "name": "Yes, and use auto mode", "kind": "allow_always"}),
    ];
    if bypass {
        options.push(json!({"optionId": "exit-plan-bypass", "name": "Yes, and bypass permissions", "kind": "allow_always"}));
    }
    options.extend([
        json!({"optionId": "exit-plan-accept-edits", "name": "Yes, auto-accept edits", "kind": "allow_always"}),
        json!({"optionId": "exit-plan-default", "name": "Yes, manually approve edits", "kind": "allow_once"}),
        json!({"optionId": "reject", "name": "No, keep planning", "kind": "reject_once"}),
    ]);
    Value::Array(options)
}

/// The mode an `exit-plan-*` option switches to.
pub(super) fn exit_plan_mode(option: &str) -> Option<&'static str> {
    match option {
        "exit-plan-auto" => Some("auto"),
        "exit-plan-bypass" => Some("bypassPermissions"),
        "exit-plan-accept-edits" => Some("acceptEdits"),
        "exit-plan-default" => Some("default"),
        _ => None,
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
