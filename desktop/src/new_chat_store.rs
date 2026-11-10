//! New-chat metadata and attachment payloads are separate from app preferences.
//! A single worker preserves write order without doing file I/O on GPUI's thread.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock, mpsc},
};

enum Job {
    Save {
        key: String,
        metadata: Value,
        attachments: Option<Value>,
    },
    Read {
        key: String,
        reply: mpsc::Sender<Result<Value, String>>,
    },
    List {
        reply: mpsc::Sender<Vec<(String, Value)>>,
    },
}
static ERROR: Mutex<Option<String>> = Mutex::new(None);

fn root() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!("xd-new-chat-store-tests-{}", std::process::id()))
    } else {
        crate::settings::settings_path().with_file_name("new-chat-drafts")
    }
}

fn directory(key: &str) -> PathBuf {
    // Encode components rather than interpreting remote identifiers as paths.
    let mut path = root();
    for chunk in key.as_bytes().chunks(60) {
        path.push(chunk.iter().map(|b| format!("{b:02x}")).collect::<String>());
    }
    path
}

/// The drafts under `dir` holding something to send, without their files.
/// `prefix` is the key so far, decoded from the directories above.
fn list_keys(dir: &std::path::Path, prefix: &[u8], keys: &mut Vec<(String, Value)>) {
    if let Ok(bytes) = fs::read(dir.join("draft.json"))
        && let Ok(draft) = serde_json::from_slice::<Value>(&bytes)
        && (draft["text"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty())
            || draft["has_attachments"] == true)
        && let Ok(key) = String::from_utf8(prefix.to_vec())
    {
        keys.push((key, draft));
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() % 2 != 0 || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Some(chunk) = (0..name.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&name[at..at + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()
        else {
            continue;
        };
        let mut key = prefix.to_vec();
        key.extend(chunk);
        list_keys(&entry.path(), &key, keys);
    }
}

fn attachment_path(dir: &std::path::Path, metadata: &Value) -> PathBuf {
    let name = metadata["attachment_file"]
        .as_str()
        .filter(|name| {
            name.starts_with("attachments-")
                && name.ends_with(".json")
                && !name.contains(['/', '\\'])
        })
        .unwrap_or("attachments.json");
    dir.join(name)
}

pub(super) fn take_error() -> Option<String> {
    ERROR.lock().unwrap().take()
}

fn atomic_write(path: &std::path::Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    let temp = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())
}

fn worker() -> &'static mpsc::Sender<Job> {
    static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let mut retry_payloads = std::collections::HashMap::<String, Value>::new();
            for job in receive {
                match job {
                    Job::Save {
                        key,
                        metadata,
                        attachments,
                    } => {
                        let attachments = attachments.or_else(|| retry_payloads.remove(&key));
                        let result = (|| {
                            let dir = directory(&key);
                            let mut builder = fs::DirBuilder::new();
                            builder.recursive(true);
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::DirBuilderExt;
                                builder.mode(0o700);
                            }
                            builder.create(&dir).map_err(|e| e.to_string())?;
                            let previous = fs::read(dir.join("draft.json"))
                                .ok()
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                            let payload = attachment_path(&dir, &metadata);
                            if let Some(attachments) = &attachments {
                                atomic_write(&payload, attachments)?;
                            }
                            atomic_write(&dir.join("draft.json"), &metadata)?;
                            if let Some(previous) = previous {
                                let old = attachment_path(&dir, &previous);
                                if old != payload {
                                    let _ = fs::remove_file(old);
                                }
                            }
                            Ok::<(), String>(())
                        })();
                        if let Err(error) = result {
                            if let Some(payload) = attachments {
                                retry_payloads.insert(key, payload);
                            }
                            *ERROR.lock().unwrap() = Some(error);
                        } else {
                            retry_payloads.remove(&key);
                        }
                    }
                    Job::Read { key, reply } => {
                        let result = (|| {
                            let dir = directory(&key);
                            let mut draft: Value = match fs::read(dir.join("draft.json")) {
                                Ok(bytes) => {
                                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                    return Ok(Value::Null);
                                }
                                Err(e) => return Err(e.to_string()),
                            };
                            if draft["has_attachments"] == true {
                                let bytes = fs::read(attachment_path(&dir, &draft))
                                    .map_err(|e| e.to_string())?;
                                draft["attachments"] =
                                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                            }
                            Ok(draft)
                        })();
                        let _ = reply.send(result);
                    }
                    Job::List { reply } => {
                        let mut keys = Vec::new();
                        list_keys(&root(), &[], &mut keys);
                        let _ = reply.send(keys);
                    }
                }
            }
        });
        send
    })
}

pub(super) fn save(key: String, metadata: Value, attachments: Option<Value>) -> Result<(), String> {
    worker()
        .send(Job::Save {
            key,
            metadata,
            attachments,
        })
        .map_err(|e| e.to_string())?;
    ERROR.lock().unwrap().take().map_or(Ok(()), Err)
}

pub(super) fn read(key: String) -> Result<Value, String> {
    let (reply, receive) = mpsc::channel();
    worker()
        .send(Job::Read { key, reply })
        .map_err(|e| e.to_string())?;
    let result = receive.recv().map_err(|e| e.to_string())?;
    if let Some(error) = ERROR.lock().unwrap().take() {
        return Err(error);
    }
    result
}

/// The saved drafts that hold something to send, by key, without their
/// files. Waits for the writes asked for before it.
pub(super) fn list() -> Vec<(String, Value)> {
    let (reply, receive) = mpsc::channel();
    if worker().send(Job::List { reply }).is_err() {
        return Vec::new();
    }
    receive.recv().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_chat_store_lists_the_drafts_holding_something() {
        let long = format!("local/project/{}/new", "p".repeat(90));
        save(
            long.clone(),
            json!({"text":"Fix the footer"}),
            Some(json!([])),
        )
        .unwrap();
        let files = "local/project/files/new".to_owned();
        save(
            files.clone(),
            json!({"text":" ","has_attachments":true}),
            Some(json!([{"name":"notes.txt","mime":"text/plain","data":"bm90ZXM="}])),
        )
        .unwrap();
        let emptied = "local/project/emptied/new".to_owned();
        save(emptied.clone(), json!({"text":"gone"}), Some(json!([]))).unwrap();
        save(emptied.clone(), json!({}), Some(json!([]))).unwrap();
        let listed = list();
        assert_eq!(
            listed.iter().find(|(key, _)| *key == long).unwrap().1["text"],
            "Fix the footer"
        );
        let keys = listed.into_iter().map(|(key, _)| key).collect::<Vec<_>>();
        assert!(keys.contains(&long), "{keys:?}");
        assert!(keys.contains(&files), "{keys:?}");
        assert!(!keys.contains(&emptied), "{keys:?}");
        for key in [long, files, emptied] {
            let _ = fs::remove_dir_all(directory(&key));
        }
    }

    #[test]
    fn new_chat_store_keeps_payloads_when_only_text_changes() {
        let key = "local/project/test/new".to_owned();
        save(
            key.clone(),
            json!({"text":"first","has_attachments":true}),
            Some(json!([{"name":"notes.txt","mime":"text/plain","data":"bm90ZXM="}])),
        )
        .unwrap();
        assert_eq!(read(key.clone()).unwrap()["text"], "first");
        let path = directory(&key).join("attachments.json");
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        save(
            key.clone(),
            json!({"text":"second","has_attachments":true}),
            None,
        )
        .unwrap();
        let restored = read(key.clone()).unwrap();
        assert_eq!(restored["text"], "second");
        assert_eq!(restored["attachments"][0]["name"], "notes.txt");
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        save(key.clone(), json!({}), Some(json!([]))).unwrap();
        assert!(read(key.clone()).unwrap().get("attachments").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(directory(&key)).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        fs::remove_dir_all(directory(&key)).unwrap();
        // Simulate an unavailable destination, then recover and retry with only new text.
        fs::write(directory(&key), b"blocked").unwrap();
        save(
            key.clone(),
            json!({"text":"first","has_attachments":true}),
            Some(json!([{"name":"retry.txt","mime":"text/plain","data":"bm90ZXM="}])),
        )
        .unwrap();
        assert!(read(key.clone()).is_err());
        fs::remove_file(directory(&key)).unwrap();
        save(
            key.clone(),
            json!({"text":"recovered","has_attachments":true}),
            None,
        )
        .unwrap();
        let restored = read(key.clone()).unwrap();
        assert_eq!(restored["text"], "recovered");
        assert_eq!(restored["attachments"][0]["name"], "retry.txt");
        fs::remove_dir_all(directory(&key)).unwrap();
    }
}
