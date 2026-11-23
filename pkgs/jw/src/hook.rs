use std::env;
use std::io::{self, Read};

use serde_json::{Map, Value};

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

pub fn cmd_hook(action: HookAction) -> Result<(), String> {
    let input = read_input()?;
    let text = |key: &str| input.get(key).and_then(Value::as_str);
    match action {
        HookAction::Create => {
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
    }
}
