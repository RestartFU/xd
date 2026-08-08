//! Display-only unified patches built from tool payloads.
//!
//! A file-editing tool call already carries everything an inline diff needs, so
//! this path wants no repository, Git executable, filesystem read, or post-tool
//! snapshot. The result is the marker `file_change`, a newline, and the patch —
//! the shape every client reads.

use serde_json::Value;

pub const PREFIX: &str = "file_change\n";
pub const LIMIT: usize = 256 * 1024;
const BUILD_LIMIT: usize = LIMIT + 1;
const TRUNCATION_NOTICE: &str = "… diff truncated …";

/// Keeps patch construction bounded before the final truncation pass. Tool
/// payloads can carry multi-megabyte files.
struct Patch {
    text: String,
    limit: usize,
    truncated: bool,
}

impl Patch {
    fn new() -> Self {
        Self {
            text: String::new(),
            limit: BUILD_LIMIT,
            truncated: false,
        }
    }

    fn push(&mut self, value: &str) -> &mut Self {
        if value.is_empty() {
            return self;
        }
        let remaining = self.remaining();
        if remaining == 0 {
            self.truncated = true;
            return self;
        }
        if value.len() <= remaining {
            self.text.push_str(value);
            return self;
        }
        self.text.push_str(floor_char_boundary(value, remaining));
        self.truncated = true;
        self
    }

    fn full(&self) -> bool {
        self.truncated || self.text.len() >= self.limit
    }

    fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.text.len())
    }

    fn finish(self) -> String {
        let mut text = self.text;
        if self.truncated && text.len() <= LIMIT {
            text.push('\n');
            text.push_str(TRUNCATION_NOTICE);
        }
        text
    }
}

/// The `file_change` row for a tool call that edits files, or `None` when the
/// call carries no rendered change.
pub fn build(name: &str, input: Option<&Value>) -> Option<String> {
    let input = input?;
    let patch = match name.to_ascii_lowercase().as_str() {
        "file_change" | "filechange" => codex(input),
        "edit" | "edit_file" => edit(input),
        "write" | "write_file" => write(input),
        "multiedit" => multi_edit(input),
        "notebookedit" | "notebook_edit" => notebook_edit(input),
        "apply_patch" | "patch" => patch(input),
        _ => None,
    }?;
    let patch = truncate(patch);
    let patch = patch.trim_end();
    if patch.is_empty() {
        return None;
    }
    Some(format!("{PREFIX}{patch}"))
}

fn codex(input: &Value) -> Option<String> {
    let changes = input.get("changes")?.as_array()?;
    let mut out = Patch::new();
    let mut rendered = false;
    for change in changes {
        let Some(path) = string(change, "path").or_else(|| string(change, "filePath")) else {
            continue;
        };
        let diff = string(change, "diff").unwrap_or_default();
        let kind_node = change.get("kind");
        let kind = kind_node
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                kind_node.and_then(|kind| {
                    string(kind, "type").or_else(|| {
                        kind.as_object()
                            .and_then(|kind| kind.keys().next())
                            .map(String::from)
                    })
                })
            })
            .unwrap_or_else(|| "update".into())
            .to_ascii_lowercase();

        let rendered_change = match kind.as_str() {
            "add" => new_file(&path, diff),
            "delete" => deleted_file(&path, diff),
            _ => {
                let move_path = kind_node.and_then(|kind| {
                    string(kind, "move_path")
                        .or_else(|| string(kind, "movePath"))
                        .or_else(|| string(kind, "path"))
                });
                updated_file(&path, move_path.unwrap_or_else(|| path.clone()), diff)
            }
        };
        if rendered {
            out.push("\n");
        }
        out.push(&rendered_change);
        rendered = true;
        if out.full() {
            break;
        }
    }
    rendered.then(|| out.finish())
}

fn edit(input: &Value) -> Option<String> {
    let path = file_path(input)?;
    let old = string(input, "old_string").or_else(|| string(input, "oldString"));
    let new = string(input, "new_string").or_else(|| string(input, "newString"));
    if old.is_none() && new.is_none() {
        return None;
    }
    Some(replacement(
        &path,
        old.unwrap_or_default(),
        new.unwrap_or_default(),
    ))
}

fn write(input: &Value) -> Option<String> {
    let path = file_path(input)?;
    let content = string(input, "content")?;
    Some(new_file(&path, content))
}

fn multi_edit(input: &Value) -> Option<String> {
    let path = file_path(input)?;
    let edits = input.get("edits")?.as_array()?;
    let mut out = Patch::new();
    let mut rendered = false;
    for item in edits {
        if !item.is_object() {
            continue;
        }
        let old = string(item, "old_string")
            .or_else(|| string(item, "oldString"))
            .unwrap_or_default();
        let new = string(item, "new_string")
            .or_else(|| string(item, "newString"))
            .unwrap_or_default();
        if rendered {
            out.push("\n");
        }
        out.push(&replacement(&path, old, new));
        rendered = true;
        if out.full() {
            break;
        }
    }
    rendered.then(|| out.finish())
}

fn notebook_edit(input: &Value) -> Option<String> {
    let path = file_path(input)?;
    let old = string(input, "old_source").or_else(|| string(input, "oldSource"));
    let new = string(input, "new_source").or_else(|| string(input, "newSource"));
    if old.is_none() && new.is_none() {
        return None;
    }
    Some(replacement(
        &path,
        old.unwrap_or_default(),
        new.unwrap_or_default(),
    ))
}

fn patch(input: &Value) -> Option<String> {
    let raw = string(input, "patch")
        .or_else(|| string(input, "diff"))
        .or_else(|| string(input, "input"))?;
    if raw.starts_with("diff --git ") {
        return Some(raw);
    }
    apply_patch(&raw)
}

fn apply_patch(raw: &str) -> Option<String> {
    let input_truncated = raw.len() > BUILD_LIMIT;
    let source = if input_truncated {
        format!("{}\n*** End Patch", floor_char_boundary(raw, BUILD_LIMIT))
    } else {
        raw.to_owned()
    };

    let lines = source.lines().collect::<Vec<_>>();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return None;
    }

    let mut out = Patch::new();
    let mut rendered_any = false;
    let mut index = 1;
    let finish = lines.len() - 1;
    while index < finish {
        let (kind, path) = apply_header(lines[index])?;
        index += 1;

        let mut move_path = None;
        if kind == ApplyKind::Update
            && index < finish
            && let Some(target) = lines[index].strip_prefix("*** Move to: ")
        {
            let target = target.trim();
            if target.is_empty() {
                return None;
            }
            move_path = Some(target.to_owned());
            index += 1;
        }

        let start = index;
        while index < finish && !apply_file_header(lines[index]) {
            index += 1;
        }
        let mut body = &lines[start..index];
        if body.last() == Some(&"*** End of File") {
            body = &body[..body.len() - 1];
        }
        let rendered = match kind {
            ApplyKind::Add => apply_add(&path, body),
            ApplyKind::Delete => apply_delete(&path, body),
            ApplyKind::Update => apply_update(&path, move_path.as_deref().unwrap_or(&path), body),
        }?;
        if rendered_any {
            out.push("\n");
        }
        out.push(&rendered);
        rendered_any = true;
        if out.full() {
            break;
        }
    }
    if !rendered_any {
        return None;
    }

    let mut result = out.finish();
    if input_truncated && result.len() <= LIMIT {
        result = format!("{}\n{TRUNCATION_NOTICE}", result.trim_end());
    }
    Some(result)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApplyKind {
    Add,
    Delete,
    Update,
}

fn apply_header(line: &str) -> Option<(ApplyKind, String)> {
    for (prefix, kind) in [
        ("*** Add File: ", ApplyKind::Add),
        ("*** Delete File: ", ApplyKind::Delete),
        ("*** Update File: ", ApplyKind::Update),
    ] {
        if let Some(path) = line.strip_prefix(prefix) {
            let path = path.trim();
            if path.is_empty() {
                return None;
            }
            return Some((kind, path.to_owned()));
        }
    }
    None
}

fn apply_file_header(line: &str) -> bool {
    line.starts_with("*** Add File: ")
        || line.starts_with("*** Delete File: ")
        || line.starts_with("*** Update File: ")
}

fn apply_add(path: &str, body: &[&str]) -> Option<String> {
    if body.is_empty() || !body.iter().all(|line| line.starts_with('+')) {
        return None;
    }
    let content = body
        .iter()
        .map(|line| &line[1..])
        .collect::<Vec<_>>()
        .join("\n");
    Some(new_file(path, format!("{content}\n")))
}

fn apply_delete(path: &str, body: &[&str]) -> Option<String> {
    if body.is_empty() {
        return Some(deleted_file(path, String::new()));
    }
    if !body.iter().all(|line| line.starts_with('-')) {
        return None;
    }
    let content = body
        .iter()
        .map(|line| &line[1..])
        .collect::<Vec<_>>()
        .join("\n");
    Some(deleted_file(path, format!("{content}\n")))
}

fn apply_update(old_path: &str, new_path: &str, body: &[&str]) -> Option<String> {
    if body.is_empty() && old_path == new_path {
        return None;
    }

    let mut hunks: Vec<Vec<&str>> = Vec::new();
    for line in body {
        if line.starts_with("@@") {
            hunks.push(Vec::new());
        } else if let Some(hunk) = hunks.last_mut() {
            if line.is_empty() || !matches!(line.as_bytes()[0], b' ' | b'+' | b'-') {
                return None;
            }
            hunk.push(line);
        } else {
            return None;
        }
    }
    if hunks.is_empty() && old_path == new_path {
        return None;
    }
    if hunks.iter().any(Vec::is_empty) {
        return None;
    }

    let mut out = Patch::new();
    file_header(&mut out, old_path, new_path);
    if old_path != new_path {
        out.push("rename from ");
        out.push(&safe_path(old_path));
        out.push("\nrename to ");
        out.push(&safe_path(new_path));
        out.push("\n");
    }
    if !hunks.is_empty() {
        out.push("--- a/");
        out.push(&safe_path(old_path));
        out.push("\n+++ b/");
        out.push(&safe_path(new_path));
        out.push("\n");
        for hunk in &hunks {
            let old_count = hunk.iter().filter(|line| !line.starts_with('+')).count();
            let new_count = hunk.iter().filter(|line| !line.starts_with('-')).count();
            let old_start = usize::from(old_count != 0);
            let new_start = usize::from(new_count != 0);
            out.push(&format!(
                "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
            ));
            for line in hunk {
                out.push(line);
                out.push("\n");
                if out.full() {
                    break;
                }
            }
            if out.full() {
                break;
            }
        }
    }
    Some(out.finish())
}

fn replacement(path: &str, old_text: String, new_text: String) -> String {
    let mut out = Patch::new();
    file_header(&mut out, path, path);
    out.push("--- a/");
    out.push(&safe_path(path));
    out.push("\n+++ b/");
    out.push(&safe_path(path));
    out.push(&format!(
        "\n@@ -1,{} +1,{} @@\n",
        line_count(&old_text),
        line_count(&new_text)
    ));
    prefix_lines(&mut out, &old_text, '-');
    prefix_lines(&mut out, &new_text, '+');
    out.finish()
}

fn new_file(path: &str, content: impl AsRef<str>) -> String {
    let content = content.as_ref();
    let mut out = Patch::new();
    file_header(&mut out, path, path);
    out.push("new file mode 100644\n--- /dev/null\n+++ b/");
    out.push(&safe_path(path));
    out.push(&format!("\n@@ -0,0 +1,{} @@\n", line_count(content)));
    prefix_lines(&mut out, content, '+');
    out.finish()
}

fn deleted_file(path: &str, content: impl AsRef<str>) -> String {
    let content = content.as_ref();
    let mut out = Patch::new();
    file_header(&mut out, path, path);
    out.push("deleted file mode 100644\n--- a/");
    out.push(&safe_path(path));
    out.push(&format!(
        "\n+++ /dev/null\n@@ -1,{} +0,0 @@\n",
        line_count(content)
    ));
    prefix_lines(&mut out, content, '-');
    out.finish()
}

fn updated_file(old_path: &str, new_path: String, diff: String) -> String {
    if diff.starts_with("diff --git ") {
        return diff;
    }
    let mut out = Patch::new();
    file_header(&mut out, old_path, &new_path);
    if old_path != new_path {
        out.push("rename from ");
        out.push(&safe_path(old_path));
        out.push("\nrename to ");
        out.push(&safe_path(&new_path));
        out.push("\n");
    }
    if !diff.starts_with("--- ") {
        out.push("--- a/");
        out.push(&safe_path(old_path));
        out.push("\n+++ b/");
        out.push(&safe_path(&new_path));
        out.push("\n");
    }
    out.push(&diff);
    if !diff.ends_with('\n') {
        out.push("\n");
    }
    out.finish()
}

fn file_header(out: &mut Patch, old_path: &str, new_path: &str) {
    out.push("diff --git a/");
    out.push(&safe_path(old_path));
    out.push(" b/");
    out.push(&safe_path(new_path));
    out.push("\n");
}

fn prefix_lines(out: &mut Patch, text: &str, prefix: char) {
    if text.is_empty() {
        return;
    }
    let sample = floor_char_boundary(text, out.remaining());
    for line in sample.lines() {
        out.push(&format!("{prefix}{line}\n"));
        if out.full() {
            break;
        }
    }
    if sample.len() < text.len() {
        out.truncated = true;
    }
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let bytes = &text.as_bytes()[..text.len().min(BUILD_LIMIT)];
    let count = bytes.iter().filter(|byte| **byte == b'\n').count();
    if bytes.last() == Some(&b'\n') {
        count
    } else {
        count + 1
    }
}

fn file_path(input: &Value) -> Option<String> {
    for key in [
        "file_path",
        "filePath",
        "notebook_path",
        "notebookPath",
        "path",
    ] {
        if let Some(path) = string(input, key) {
            return Some(path);
        }
    }
    None
}

fn string(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn safe_path(path: &str) -> String {
    path.replace(['\n', '\r', '\t'], " ")
}

fn truncate(patch: String) -> String {
    if patch.len() <= LIMIT {
        return patch;
    }
    format!(
        "{}\n{TRUNCATION_NOTICE}",
        floor_char_boundary(&patch, LIMIT)
    )
}

/// The longest prefix of `text` that fits in `limit` bytes without splitting a
/// character. Tool payloads are arbitrary UTF-8, so a byte slice can land mid
/// character.
fn floor_char_boundary(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_claude_edit_and_write_diffs_from_tool_arguments() {
        let edit = build(
            "Edit",
            Some(&serde_json::json!({
                "file_path": "README.md",
                "old_string": "before\n",
                "new_string": "after\n"
            })),
        )
        .expect("an edit carries a diff");
        assert!(edit.starts_with("file_change\ndiff --git "));
        assert!(edit.contains("-before"));
        assert!(edit.contains("+after"));

        let write = build(
            "Write",
            Some(&serde_json::json!({
                "file_path": "new.txt",
                "content": "created\n"
            })),
        )
        .expect("a write carries a diff");
        assert!(write.contains("new file mode 100644"));
        assert!(write.contains("+created"));

        assert_eq!(
            build("Read", Some(&serde_json::json!({"file_path": "a"}))),
            None
        );
        assert_eq!(
            build("Edit", Some(&serde_json::json!({"file_path": "a"}))),
            None
        );
    }

    #[test]
    fn builds_codex_file_changes_without_a_git_repository() {
        let summary = build(
            "file_change",
            Some(&serde_json::json!({
                "changes": [
                    {"path": "src/new.rs", "kind": {"type": "add"}, "diff": "println!();\n"},
                    {
                        "path": "src/old.rs",
                        "kind": {"type": "update", "move_path": null},
                        "diff": "@@ -1 +1 @@\n-old\n+new\n"
                    }
                ]
            })),
        )
        .expect("codex changes carry a diff");

        assert!(summary.starts_with("file_change\ndiff --git "));
        assert!(summary.contains("+++ b/src/new.rs"));
        assert!(summary.contains("+println!();"));
        assert!(summary.contains("@@ -1 +1 @@"));
        assert!(summary.contains("-old"));
    }

    #[test]
    fn builds_apply_patch_diffs_without_reading_a_repository() {
        let summary = build(
            "apply_patch",
            Some(&serde_json::json!({
                "patch": "*** Begin Patch\n*** Update File: src/old.rs\n*** Move to: src/new.rs\n\
                          @@\n-puts :old\n+puts :new\n*** Add File: notes.txt\n+hello\n\
                          *** Delete File: gone.txt\n*** End Patch"
            })),
        )
        .expect("an apply patch carries a diff");

        assert!(summary.starts_with("file_change\ndiff --git "));
        assert!(summary.contains("rename from src/old.rs"));
        assert!(summary.contains("rename to src/new.rs"));
        assert!(summary.contains("-puts :old"));
        assert!(summary.contains("+puts :new"));
        assert!(summary.contains("new file mode 100644"));
        assert!(summary.contains("deleted file mode 100644"));

        assert_eq!(
            build(
                "apply_patch",
                Some(&serde_json::json!({"patch": "*** Begin Patch\nnot a file\n*** End Patch"}))
            ),
            None
        );
    }

    #[test]
    fn builds_notebook_edits_from_source_arguments() {
        let summary = build(
            "NotebookEdit",
            Some(&serde_json::json!({
                "notebook_path": "analysis.ipynb",
                "old_source": "before()",
                "new_source": "after()"
            })),
        )
        .expect("a notebook edit carries a diff");

        assert!(summary.contains("--- a/analysis.ipynb"));
        assert!(summary.contains("-before()"));
        assert!(summary.contains("+after()"));
    }

    #[test]
    fn bounds_generated_diffs_while_building_them() {
        let content = format!("{}\nnever reached\n", "é".repeat(2 * 1024 * 1024));
        let summary = build(
            "Write",
            Some(&serde_json::json!({"file_path": "generated.txt", "content": content})),
        )
        .expect("a bounded write still carries a diff");

        assert!(summary.len() < LIMIT + 128);
        assert!(summary.contains(TRUNCATION_NOTICE));
        assert!(!summary.contains("never reached"));
    }

    #[test]
    fn shares_one_output_budget_across_multi_edit_patches() {
        let edits = (0..32)
            .map(|index| {
                serde_json::json!({
                    "old_string": format!("old {index}\n"),
                    "new_string": format!("{}\n", "x".repeat(512 * 1024))
                })
            })
            .collect::<Vec<_>>();
        let summary = build(
            "MultiEdit",
            Some(&serde_json::json!({"file_path": "generated.txt", "edits": edits})),
        )
        .expect("a bounded multi edit still carries a diff");

        assert!(summary.len() < LIMIT + 128);
        assert!(summary.contains(TRUNCATION_NOTICE));
        assert!(summary.contains("old 0"));
        assert!(!summary.contains("old 31"));
    }

    #[test]
    fn bounds_apply_patch_parsing_before_splitting_lines() {
        let body = (0..200_000)
            .map(|index| format!("+generated {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let summary = build(
            "apply_patch",
            Some(&serde_json::json!({
                "patch": format!("*** Begin Patch\n*** Add File: generated.txt\n{body}\n*** End Patch")
            })),
        )
        .expect("a bounded apply patch still carries a diff");

        assert!(summary.len() < LIMIT + 128);
        assert!(summary.contains("new file mode 100644"));
        assert!(summary.contains(TRUNCATION_NOTICE));
        assert!(!summary.contains("generated 199999"));
    }

    #[test]
    fn passes_through_patches_git_already_produced() {
        let patch = "diff --git a/new.txt b/new.txt\n+created\n";
        assert_eq!(
            build("apply_patch", Some(&serde_json::json!({"patch": patch}))).as_deref(),
            Some("file_change\ndiff --git a/new.txt b/new.txt\n+created")
        );
        assert_eq!(
            build(
                "apply_patch",
                Some(&serde_json::json!({"patch": "not a diff"}))
            ),
            None
        );
    }
}
