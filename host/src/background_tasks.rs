//! Read-only view of work owned by a kept Claude process. Paths come exclusively
//! from Claude's task messages/tool results, never from a client request.
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::subagent::SubagentStatus;

const TAIL_BYTES: u64 = 4096;
const TAIL_LINES: usize = 20;
const MAX_PENDING: usize = 200;

#[derive(Clone, Debug)]
struct Task {
    id: String,
    tool_id: String,
    description: String,
    kind: String,
    started_at: u64,
    output_file: Option<String>,
}

#[derive(Default)]
pub(crate) struct Tasks {
    live: HashMap<String, Task>,
    tools: HashMap<String, Value>,
    stream: HashMap<u64, (String, String)>,
    completed: HashSet<String>,
}

impl Tasks {
    pub(crate) fn len(&self) -> usize {
        self.live.len()
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
    pub(crate) fn clear(&mut self) {
        self.live.clear();
        self.tools.clear();
        self.stream.clear();
        self.completed.clear();
    }

    fn finish(&mut self, id: &str, tool_id: &str) {
        if self.completed.len() >= MAX_PENDING * 2 {
            self.completed.clear();
        }
        self.completed.insert(id.into());
        if !tool_id.is_empty() {
            self.completed.insert(tool_id.into());
        }
        if let Some(task) = self.live.remove(id) {
            self.tools.remove(&task.tool_id);
            if !task.tool_id.is_empty() {
                self.completed.insert(task.tool_id);
            }
        }
    }

    pub(crate) fn track(&mut self, line: &str) {
        if ![
            "task_",
            "\"tool_use\"",
            "\"tool_result\"",
            "content_block_start",
            "content_block_stop",
            "input_json_delta",
            "<task-notification>",
        ]
        .iter()
        .any(|marker| line.contains(marker))
        {
            return;
        }
        let Ok(root) = serde_json::from_str::<Value>(line) else {
            return;
        };
        // Child agents can reuse tool ids; the parent reports their tasks.
        if field(&root, &["parent_tool_use_id"]).is_some() {
            return;
        }
        if root["type"] == "stream_event" {
            let event = &root["event"];
            if let Some(index) = event["index"].as_u64() {
                match event["type"].as_str() {
                    Some("content_block_start") => {
                        let block = &event["content_block"];
                        if block["type"] == "tool_use"
                            && matches!(
                                block["name"].as_str(),
                                Some("Bash" | "Monitor" | "Agent" | "Task" | "Workflow")
                            )
                            && let Some(id) = field(block, &["id"])
                            && self.tools.len() < MAX_PENDING
                            && self.stream.len() < MAX_PENDING
                        {
                            self.tools.insert(id.into(), tool_metadata(block));
                            self.stream.insert(index, (id.into(), String::new()));
                        }
                    }
                    Some("content_block_delta") => {
                        if let Some((_, text)) = self.stream.get_mut(&index)
                            && let Some(fragment) = event["delta"]["partial_json"].as_str()
                            && text.len() + fragment.len() <= 16 * 1024
                        {
                            text.push_str(fragment);
                        }
                    }
                    Some("content_block_stop") => {
                        if let Some((id, text)) = self.stream.remove(&index)
                            && let Ok(input) = serde_json::from_str::<Value>(&text)
                            && let Some(tool) = self.tools.get_mut(&id)
                        {
                            tool["input"] = tool_metadata(
                                &json!({"name": tool["name"], "input":input}),
                            )["input"]
                                .clone();
                        }
                    }
                    _ => {}
                }
            }
        }
        if root["type"] == "assistant"
            && let Some(blocks) = root["message"]["content"].as_array()
        {
            for block in blocks {
                if block["type"] == "tool_use"
                    && matches!(
                        block["name"].as_str(),
                        Some("Bash" | "Monitor" | "Agent" | "Task" | "Workflow")
                    )
                    && let Some(id) = field(block, &["id"])
                    && (self.tools.contains_key(id) || self.tools.len() < MAX_PENDING)
                {
                    self.tools.insert(id.into(), tool_metadata(block));
                }
            }
        }
        if root["type"] == "system" {
            let Some(id) = field(&root, &["task_id"]) else {
                return;
            };
            let final_status = |value: &Value| {
                value
                    .as_str()
                    .and_then(SubagentStatus::from_wire)
                    .is_some_and(|status| status != SubagentStatus::Running)
            };
            match root["subtype"].as_str() {
                Some("task_notification") if root["status"] != "running" => {
                    self.finish(id, field(&root, &["tool_use_id"]).unwrap_or_default());
                }
                Some("task_updated") if final_status(&root["patch"]["status"]) => {
                    self.finish(id, field(&root, &["tool_use_id"]).unwrap_or_default());
                }
                Some("task_started") if root["skip_transcript"] == true => {
                    self.finish(id, field(&root, &["tool_use_id"]).unwrap_or_default());
                }
                Some("task_started") => {
                    let tool_id = field(&root, &["tool_use_id"]).unwrap_or_default();
                    self.completed.remove(id);
                    self.completed.remove(tool_id);
                    // A tool result can arrive before task_started. Merge its
                    // record, retaining its original start time and output path.
                    let alias = self
                        .live
                        .contains_key(id)
                        .then(|| id.to_owned())
                        .or_else(|| {
                            self.live
                                .iter()
                                .find(|(_, t)| !tool_id.is_empty() && t.tool_id == tool_id)
                                .map(|(key, _)| key.clone())
                        });
                    let mut task = alias
                        .and_then(|key| self.live.remove(&key))
                        .unwrap_or_else(|| new_task(id, tool_id));
                    task.id = id.into();
                    if !tool_id.is_empty() {
                        task.tool_id = tool_id.into();
                    }
                    enrich(&mut task, &root);
                    if let Some(tool) = self.tools.get(tool_id) {
                        enrich_tool(&mut task, tool);
                    }
                    self.live.insert(id.into(), task);
                }
                Some("task_progress" | "task_updated") => {
                    if let Some(task) = self.live.get_mut(id) {
                        if root["subtype"] == "task_progress" {
                            let mut progress = root.clone();
                            progress.as_object_mut().unwrap().remove("description");
                            enrich(task, &progress);
                        } else {
                            enrich(task, &root["patch"]);
                        }
                    }
                }
                _ => {}
            }
        }
        if root["type"] != "user" {
            return;
        }
        if let Some(text) = root["message"]["content"].as_str()
            && text.trim_start().starts_with("<task-notification>")
            && let Some(id) = tag(text, "task-id")
            && tag(text, "status")
                .is_some_and(|s| matches!(s, "completed" | "failed" | "killed" | "stopped"))
        {
            self.finish(id, "");
        }
        let Some(blocks) = root["message"]["content"].as_array() else {
            return;
        };
        for block in blocks {
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(tool_id) = field(block, &["tool_use_id"]) else {
                continue;
            };
            let tool = self.tools.remove(tool_id);
            if block["is_error"] == true || self.completed.contains(tool_id) {
                continue;
            }
            let structured = &root["tool_use_result"];
            let text = result_text(&block["content"]);
            let existing = self
                .live
                .iter()
                .find(|(_, t)| t.tool_id == tool_id)
                .map(|(id, _)| id.clone());
            if tool.is_none() && existing.is_none() {
                continue;
            }
            if matches!(
                tool.as_ref().and_then(|t| t["name"].as_str()),
                Some("Agent" | "Task")
            ) && tool
                .as_ref()
                .is_none_or(|t| t["input"]["run_in_background"] != true)
                && structured["isAsync"] != true
                && existing.is_none()
            {
                continue;
            }
            let structured_id = field(
                structured,
                &[
                    "backgroundTaskId",
                    "background_task_id",
                    "task_id",
                    "taskId",
                    "agentId",
                ],
            )
            .map(str::to_owned);
            // A command's output can say anything, Claude's own notices
            // included, so text is read for a task id and output path only
            // when something other than the text says the work is in the
            // background: the call asked for it, Claude's structured result
            // names a task, or the task is already known from task_started.
            let backgrounded = structured_id.is_some()
                || existing.is_some()
                || structured["isAsync"] == true
                || tool
                    .as_ref()
                    .is_some_and(|t| t["input"]["run_in_background"] == true);
            let task_id =
                structured_id.or_else(|| backgrounded.then(|| reported_id(&text)).flatten());
            let Some(id) = existing.or(task_id) else {
                continue;
            };
            if self.completed.contains(&id) {
                continue;
            }
            if structured["status"]
                .as_str()
                .and_then(SubagentStatus::from_wire)
                .is_some_and(|s| s != SubagentStatus::Running)
            {
                self.finish(&id, tool_id);
                continue;
            }
            let task = self
                .live
                .entry(id.clone())
                .or_insert_with(|| new_task(&id, tool_id));
            if let Some(tool) = &tool {
                enrich_tool(task, tool);
            }
            enrich(task, structured);
            if task.output_file.is_none() && backgrounded {
                task.output_file = reported_path(&text);
            }
        }
    }

    /// Clone under the router lock; file I/O takes place after releasing it.
    pub(crate) fn snapshot(&self, task_id: Option<&str>) -> Vec<Value> {
        let mut tasks = self
            .live
            .values()
            .filter(|t| task_id.is_none_or(|id| t.id == id))
            .collect::<Vec<_>>();
        tasks.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
        tasks
            .into_iter()
            .map(|t| {
                json!({
                    "id": t.id, "kind": t.kind, "description": t.description,
                    "started_at": t.started_at, "output_file": t.output_file,
                })
            })
            .collect()
    }
}

fn new_task(id: &str, tool_id: &str) -> Task {
    Task {
        id: id.into(),
        tool_id: tool_id.into(),
        description: "Background task".into(),
        kind: "Task".into(),
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        output_file: None,
    }
}

fn field<'a>(v: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| v[*name].as_str().filter(|s| !s.is_empty()))
}

fn tool_metadata(block: &Value) -> Value {
    let input = &block["input"];
    let mut result = json!({"name": block["name"], "input": {}});
    for key in ["description", "command", "prompt"] {
        if let Some(value) = input[key].as_str() {
            result["input"][key] = json!(short(value));
        }
    }
    result["input"]["run_in_background"] = input["run_in_background"].clone();
    result
}

fn short(text: &str) -> String {
    text.chars().take(240).collect()
}

fn enrich(task: &mut Task, v: &Value) {
    if let Some(description) = field(v, &["description"]) {
        task.description = short(description);
    }
    if let Some(kind) = field(v, &["task_type", "subagent_type"]) {
        task.kind = match kind {
            "local_bash" => "Shell",
            "local_workflow" if task.kind == "Monitor" => "Monitor",
            "local_workflow" => "Workflow",
            "local_agent" => "Agent",
            _ => kind,
        }
        .into();
    }
    if let Some(path) = field(
        v,
        &[
            "output_file",
            "outputFile",
            "output_file_path",
            "outputFilePath",
            "backgroundOutputPath",
            "output_path",
        ],
    ) && Path::new(path).is_absolute()
    {
        task.output_file = Some(path.into());
    }
}

fn enrich_tool(task: &mut Task, tool: &Value) {
    let input = &tool["input"];
    if task.description == "Background task" {
        task.description =
            short(field(input, &["description", "command", "prompt"]).unwrap_or("Background task"));
    }
    if let Some(name) = field(tool, &["name"]) {
        task.kind = match name {
            "Bash" => "Shell",
            "Task" | "Agent" => "Agent",
            other => other,
        }
        .into();
    }
}

fn result_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.into();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn reported_id(text: &str) -> Option<String> {
    [
        "Command running in background with ID:",
        "Command was manually backgrounded by user with ID:",
        "background (ID:",
        "Monitor started (task ",
        "Agent ID:",
        "agentId:",
        "Task ID:",
        "task_id:",
        "task_id=",
        "Monitor started with ID:",
    ]
    .iter()
    .find_map(|prefix| {
        text.split_once(prefix).and_then(|(_, rest)| {
            let id = rest
                .trim_start()
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .next()?;
            (!id.is_empty()).then(|| id.into())
        })
    })
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.split_once(&format!("<{name}>"))?
        .1
        .split_once(&format!("</{name}>"))
        .map(|(value, _)| value.trim())
        .filter(|v| !v.is_empty())
}

fn reported_path(text: &str) -> Option<String> {
    if let Some(path) = tag(text, "output_file").filter(|p| Path::new(p).is_absolute()) {
        return Some(path.into());
    }
    [
        "Output is being written to:",
        "output_file:",
        "output file:",
    ]
    .iter()
    .find_map(|prefix| {
        text.split_once(prefix).and_then(|(_, rest)| {
            let path = rest.lines().next()?.trim().trim_matches('`');
            Path::new(path).is_absolute().then(|| path.to_owned())
        })
    })
}

pub(crate) fn read_outputs(mut tasks: Vec<Value>) -> Vec<Value> {
    for task in &mut tasks {
        let output = task["output_file"]
            .as_str()
            .ok_or_else(|| "Output file not reported.".to_owned())
            .and_then(|path| {
                tail(Path::new(path)).map_err(|_| "Output file unavailable.".to_owned())
            });
        match output {
            Ok((lines, truncated)) => {
                task["lines"] = json!(lines);
                task["truncated"] = json!(truncated);
            }
            Err(error) => {
                task["lines"] = json!([]);
                task["output_error"] = json!(error);
            }
        }
    }
    tasks
}

fn tail(path: &Path) -> std::io::Result<(Vec<Value>, bool)> {
    let mut file = crate::private_fs::open_without_waiting_no_symlinks(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("Not a regular file"));
    }
    let start = metadata.len().saturating_sub(TAIL_BYTES);
    // One byte more, before the window, to tell whether it starts a line.
    let from = start.saturating_sub(1);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES + (start - from))
        .read_to_end(&mut bytes)?;
    if start > from {
        if bytes.first() == Some(&b'\n') {
            // The window starts a line: keep all of it.
            bytes.remove(0);
        } else if let Some(end) = bytes.iter().position(|b| *b == b'\n') {
            // Discard a clipped first line.
            bytes.drain(..=end);
        } else {
            // A single line longer than the limit: keep its last bytes, so a
            // long, still-being-written error is visible.
            bytes.remove(0);
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().collect::<Vec<_>>();
    let truncated = start > 0 || lines.len() > TAIL_LINES;
    Ok((
        lines
            .iter()
            .skip(lines.len().saturating_sub(TAIL_LINES))
            .map(|line| json!({"text": line, "error": error_line(line)}))
            .collect(),
        truncated,
    ))
}

fn error_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    [
        "command not found",
        "error",
        "failed",
        "traceback",
        "non-zero",
        "nonzero",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || ["exit code", "exit status", "exited with code"]
            .iter()
            .any(|prefix| {
                lower.split_once(prefix).is_some_and(|(_, rest)| {
                    rest.trim_start_matches([' ', ':', '='])
                        .split_whitespace()
                        .next()
                        .and_then(|code| code.trim_end_matches(['.', ',', ')']).parse::<i32>().ok())
                        .is_some_and(|code| code != 0)
                })
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Instant};

    #[test]
    fn failed_background_tool_results_do_not_exhaust_pending_metadata() {
        let mut tasks = Tasks::default();
        for n in 0..MAX_PENDING + 5 {
            let id = format!("failed-{n}");
            tasks.track(
                &json!({"type":"assistant","message":{"content":[{
                    "type":"tool_use","id":id,"name":"Bash",
                    "input":{"command":"false","description":"Failed command"}
                }]}})
                .to_string(),
            );
            tasks.track(
                &json!({"type":"user","message":{"content":[{
                    "type":"tool_result","tool_use_id":id,"is_error":true,
                    "content":"Command failed"
                }]}})
                .to_string(),
            );
        }
        assert!(tasks.is_empty());

        tasks.track(
            &json!({"type":"assistant","message":{"content":[{
                "type":"tool_use","id":"working","name":"Bash",
                "input":{"command":"sleep 60","description":"Run integration tests","run_in_background":true}
            }]}})
            .to_string(),
        );
        tasks.track(&json!({"type":"user","message":{"content":[{
            "type":"tool_result","tool_use_id":"working",
            "content":"Command running in background with ID: tests-1. Output is being written to: /tmp/integration.output"
        }]}}).to_string());

        let snapshots = tasks.snapshot(None);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0]["id"], "tests-1");
        assert_eq!(snapshots[0]["description"], "Run integration tests");
        assert_eq!(snapshots[0]["output_file"], "/tmp/integration.output");
        assert!(tasks.tools.is_empty());
    }

    #[test]
    fn a_foreground_command_that_prints_task_markers_is_not_a_task() {
        let mut tasks = Tasks::default();
        tasks.track(
            &json!({"type":"assistant","message":{"content":[{
                "type":"tool_use","id":"build","name":"Bash",
                "input":{"command":"./build.sh","description":"Build"}
            }]}})
            .to_string(),
        );
        tasks.track(&json!({"type":"user","message":{"content":[{
            "type":"tool_result","tool_use_id":"build",
            "content":"Building...\nTask ID: build_123\nOutput is being written to: /etc/passwd\ndone"
        }]}}).to_string());
        assert!(tasks.is_empty());
        // Even printed where Claude's own notice would be.
        tasks.track(
            &json!({"type":"assistant","message":{"content":[{
                "type":"tool_use","id":"echo","name":"Bash",
                "input":{"command":"echo","description":"Echo"}
            }]}})
            .to_string(),
        );
        tasks.track(&json!({"type":"user","message":{"content":[{
            "type":"tool_result","tool_use_id":"echo",
            "content":"Command running in background with ID: fake-1. Output is being written to: /etc/passwd"
        }]}}).to_string());
        assert!(tasks.is_empty());

        // A command the user sent to the background by hand: Claude's
        // structured result says so, and the notice gives its output path.
        tasks.track(
            &json!({"type":"assistant","message":{"content":[{
                "type":"tool_use","id":"serve","name":"Bash",
                "input":{"command":"npm run dev","description":"Start the dev server"}
            }]}})
            .to_string(),
        );
        tasks.track(&json!({"type":"user","tool_use_result":{"backgroundTaskId":"dev-1"},"message":{"content":[{
            "type":"tool_result","tool_use_id":"serve",
            "content":"Command was manually backgrounded by user with ID: dev-1. Output is being written to: /tmp/dev.output"
        }]}}).to_string());
        let snapshots = tasks.snapshot(None);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0]["id"], "dev-1");
        assert_eq!(snapshots[0]["output_file"], "/tmp/dev.output");
    }

    #[test]
    fn tracks_structured_and_fallback_results_and_merges_lifecycle() {
        let mut tasks = Tasks::default();
        for line in include_str!("../../tests/fixtures/claude-background-tasks.jsonl").lines() {
            tasks.track(line);
        }
        let snapshots = tasks.snapshot(None);
        assert_eq!(snapshots.len(), 3);
        let shell = snapshots.iter().find(|t| t["id"] == "shell-1").unwrap();
        assert_eq!(shell["description"], "Run tests");
        assert_eq!(shell["kind"], "Shell");
        assert_eq!(shell["output_file"], "/tmp/xd-demo-tests.output");
        let monitor = snapshots.iter().find(|t| t["id"] == "watch-1").unwrap();
        assert_eq!(monitor["kind"], "Monitor");
        assert_eq!(monitor["output_file"], "/tmp/xd-demo-watch.output");
        assert!(monitor["started_at"].as_u64().unwrap() > 0);
        for (id, subtype, extra) in [
            (
                "shell-1",
                "task_notification",
                json!({"status":"completed"}),
            ),
            (
                "watch-1",
                "task_updated",
                json!({"patch":{"status":"failed"}}),
            ),
        ] {
            let mut event = extra;
            event["type"] = json!("system");
            event["subtype"] = json!(subtype);
            event["task_id"] = json!(id);
            tasks.track(&event.to_string());
        }
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks.snapshot(None)[0]["id"], "agent-1");
    }

    #[test]
    fn structured_paths_win_and_unrelated_or_finished_tasks_are_not_revived() {
        let mut tasks = Tasks::default();
        for event in [
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"todo","name":"TaskCreate","input":{}}]}}),
            json!({"type":"user","tool_use_result":{"task_id":"todo-1"},"message":{"content":[{"type":"tool_result","tool_use_id":"todo","content":"Task created"}]}}),
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"sleep 60"}}]}}),
            json!({"type":"user","tool_use_result":{"backgroundTaskId":"b1","backgroundOutputPath":"/tmp/structured.output"},"message":{"content":[{"type":"tool_result","tool_use_id":"bash","content":"Command running in background with ID: b1. Output is being written to: /tmp/fallback.output"}]}}),
        ] {
            tasks.track(&event.to_string());
        }
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks.snapshot(None)[0]["output_file"],
            "/tmp/structured.output"
        );
        tasks.track(&json!({"type":"user","message":{"content":"<task-notification><task-id>b1</task-id><status>completed</status></task-notification>"}}).to_string());
        assert!(tasks.is_empty());
        tasks.track(&json!({"type":"user","tool_use_result":{"backgroundTaskId":"b1"},"message":{"content":[{"type":"tool_result","tool_use_id":"bash","content":"Command running in background with ID: b1."}]}}).to_string());
        assert!(tasks.is_empty());
        assert_eq!(
            reported_id("Monitor started (task watch-2). You will be notified."),
            Some("watch-2".into())
        );
        assert_eq!(
            reported_id("Command was moved to the background (ID: b2) after running for 2s."),
            Some("b2".into())
        );
    }

    #[test]
    fn a_tail_that_starts_a_line_keeps_it() {
        let path = std::env::temp_dir().join(format!("xd-tail-{}", uuid::Uuid::new_v4()));
        let newest = "a".repeat(TAIL_BYTES as usize - 1);
        fs::write(&path, format!("x\n{newest}\n")).unwrap();
        let (lines, truncated) = tail(&path).unwrap();
        assert!(truncated);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["text"], newest.as_str());
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn tail_is_bounded_includes_partial_last_line_and_marks_errors() {
        let path = std::env::temp_dir().join(format!("xd-tail-{}", uuid::Uuid::new_v4()));
        fs::write(
            &path,
            format!(
                "{}\n{}partial: jq: command not found",
                "x".repeat(6000),
                (0..30).map(|n| format!("pass {n}\n")).collect::<String>()
            ),
        )
        .unwrap();
        let (lines, truncated) = tail(&path).unwrap();
        assert!(truncated);
        assert_eq!(lines.len(), 20);
        assert_eq!(lines.last().unwrap()["error"], true);
        assert_eq!(lines[0]["error"], false);
        assert!(
            lines
                .iter()
                .map(|l| l["text"].as_str().unwrap().len())
                .sum::<usize>()
                <= 4096
        );
        fs::remove_file(&path).unwrap();
        assert!(tail(&path).is_err());
        let unavailable = read_outputs(vec![json!({"output_file":path})]);
        assert_eq!(unavailable[0]["output_error"], "Output file unavailable.");
        assert_eq!(
            read_outputs(vec![json!({"output_file":null})])[0]["output_error"],
            "Output file not reported."
        );
        fs::write(&path, "x".repeat(6000)).unwrap();
        let (lines, truncated) = tail(&path).unwrap();
        assert!(truncated);
        assert_eq!(lines[0]["text"].as_str().unwrap().len(), 4096);
        fs::remove_file(&path).unwrap();
        for line in [
            "Error: unavailable",
            "build failed",
            "Traceback (most recent call last)",
            "non-zero exit",
        ] {
            assert!(error_line(line));
        }
        assert!(!error_line("Checks passed"));
        assert!(!error_line("Process exited with code 0"));
        assert!(error_line("Process exited with code 127"));
    }

    #[test]
    fn fifo_and_symlink_are_rejected_without_waiting() {
        use std::{ffi::CString, os::unix::fs::symlink};
        let path = std::env::temp_dir().join(format!("xd-tail-{}", uuid::Uuid::new_v4()));
        let cpath = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        let start = Instant::now();
        assert!(tail(&path).is_err());
        assert!(start.elapsed().as_secs() < 1);
        fs::remove_file(&path).unwrap();
        symlink("/etc/passwd", &path).unwrap();
        assert!(tail(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        symlink("/etc", path.join("link")).unwrap();
        assert!(tail(&path.join("link/passwd")).is_err());
        fs::remove_dir_all(path).unwrap();
    }
}
