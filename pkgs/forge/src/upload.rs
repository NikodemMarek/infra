use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::api::{quote, Api};
use crate::cmd::Result;
use crate::target::{Kind, Target};

#[derive(Deserialize, Serialize)]
pub struct Upload {
    url: String,
}

pub fn upload(target: &Target, file: &Path) -> Result<Upload> {
    if !file.is_file() {
        return Err(format!("{}: no such file", file.display()));
    }
    if matches!(target.kind, Kind::GitHub) {
        return Err("github has no file upload API".into());
    }
    // an absolute path cannot be mistaken for `@-`, which makes the CLI read stdin
    let file = std::path::absolute(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let api = Api::connect(target)?;
    api.post_file(
        &format!("projects/{}/uploads", quote(&target.project)),
        &file,
    )
}
