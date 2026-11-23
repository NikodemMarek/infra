use std::env;
use std::io::{self, Read};

use serde_json::{json, Map, Value};

use crate::worktree;
use crate::HookAction;

fn read_input() -> Result<Map<String, Value>, String> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    match serde_json::from_str(&text).map_err(|e| e.to_string())? {
        Value::Object(input) => Ok(input),
        _ => Err("not a JSON object".to_string()),
    }
}

fn create(input: &Map<String, Value>) -> Result<(), String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str);
    let cwd = match text("cwd").filter(|c| !c.is_empty()) {
        Some(cwd) => cwd.to_string(),
        None => env::current_dir()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned(),
    };
    println!("{}", worktree::create(text("name").unwrap_or(""), &cwd)?);
    Ok(())
}

/// Never fails and never blocks the agent: a problem is reported, with a request to relay it unless the stop was
/// already prompted by one.
fn stop(input: &Map<String, Value>) {
    let text = |key: &str| input.get(key).and_then(Value::as_str);
    let cwd = text("cwd")
        .filter(|c| !c.is_empty())
        .unwrap_or("/nonexistent");
    let out = match worktree::stop(text("agent_id").unwrap_or(""), cwd) {
        Ok(notes) if notes.is_empty() => return,
        Ok(notes) => json!({ "systemMessage": notes.join("\n") }),
        Err(problem) => {
            let mut out = json!({ "systemMessage": problem });
            if input.get("stop_hook_active") != Some(&Value::Bool(true)) {
                out["hookSpecificOutput"] = json!({
                    "hookEventName": "SubagentStop",
                    "additionalContext": format!(
                        "{problem} Reply with this sentence verbatim, followed by your complete previous final report unchanged."
                    ),
                });
            }
            out
        }
    };
    println!("{out}");
}

pub fn cmd_hook(action: HookAction) -> Result<(), String> {
    match action {
        HookAction::Create => create(&read_input()?),
        HookAction::Stop => {
            match read_input() {
                Ok(input) => stop(&input),
                Err(e) => eprintln!("jw: {e}"),
            }
            Ok(())
        }
    }
}
