#![cfg(unix)]

use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
use xd_host::StateStore;

#[test]
fn discovered_models_can_be_selected_and_inherited_without_static_entries() {
    let root = std::env::temp_dir().join(format!("xd-model-catalog-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let codex = root.join("codex");
    let claude = root.join("claude");
    fs::write(&codex, r#"#!/bin/sh
read init
printf '%s\n' '{"id":1,"result":{}}'
read initialized
read list
printf '%s\n' '{"id":2,"result":{"data":[{"model":"future-codex","displayName":"Future Codex"},{"model":"future-everyday","displayName":"Future Everyday","isDefault":true}]}}'
read unexpected
"#).unwrap();
    fs::write(&claude, r#"#!/bin/sh
read init
printf '%s\n' '{"type":"control_response","response":{"request_id":"xd-models","response":{"models":[{"value":"opus","resolvedModel":"future-claude","displayName":"Future Claude"}]}}}'
read unexpected
"#).unwrap();
    for path in [&codex, &claude] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    // This integration-test binary contains one test and owns these overrides.
    unsafe {
        std::env::set_var("XD_CODEX_EXECUTABLE", &codex);
        std::env::set_var("XD_CLAUDE_EXECUTABLE", &claude);
    }
    let store = StateStore::open(root.join("state.sqlite"), &root).unwrap();
    let catalog = store.agent_catalog().unwrap();
    let backends = catalog["backends"].as_array().unwrap();
    for (backend, model, label) in [
        ("codex", "future-codex", "Future Codex"),
        ("claude", "future-claude", "Future Claude"),
    ] {
        let entry = backends
            .iter()
            .find(|entry| entry["id"] == backend)
            .unwrap();
        assert_eq!(entry["models"][0]["id"], model);
        let folder = store
            .new_folder(&json!({"name":format!("Project {backend}")}))
            .unwrap();
        let chat = store
            .new_chat(&json!({"folder":folder["id"],"backend":backend}))
            .unwrap();
        let _ = store
            .set_option(
                &json!({"chat":chat["id"],"option":"model","backend":backend,"value":model}),
            )
            .unwrap();
        let messages = store.messages(&json!({"chat":chat["id"]})).unwrap();
        assert_eq!(
            messages["messages"][0]["content"],
            format!("Switched to {label}")
        );
        assert_eq!(
            store.chat(chat["id"].as_str().unwrap()).unwrap()["model"],
            model
        );
        store.set_folder_settings(&json!({"folder":folder["id"],"backend":backend,"model":model,"workdir":null,"repo":null})).unwrap();
        let inherited = store.new_chat(&json!({"folder":folder["id"]})).unwrap();
        assert_eq!(
            store.chat(inherited["id"].as_str().unwrap()).unwrap()["model"],
            model
        );
        assert!(store.set_option(&json!({"chat":chat["id"],"option":"model","backend":backend,"value":"unlisted-model"})).is_err());
    }
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
