//! `xd-host profile`: push this machine's origin profile to a runner on the
//! same machine through its local socket. Paired runners get the same
//! operations over TLS from the desktop; this is the spike's way in.

use std::{collections::HashSet, time::Duration};

use serde_json::{Value, json};
use xd_host::origin_profile;

use crate::{HostOptions, daemon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProfileAction {
    Push,
    Pull,
    Status,
    Show,
}

/// A large first push carries every skill and plugin in one frame.
const PUSH_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) fn default_origin() -> String {
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default();
    let name = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let name = name.trim_start_matches('.');
    if origin_profile::valid_origin(name) {
        name.to_owned()
    } else {
        "origin".into()
    }
}

pub(crate) fn run(options: HostOptions, action: ProfileAction, origin: &str) -> Result<(), String> {
    match action {
        ProfileAction::Show => {
            let collected = origin_profile::collect_local().map_err(|e| e.to_string())?;
            println!("{}", summary(&collected));
            Ok(())
        }
        ProfileAction::Status => print(request(&options, json!({"op": "origin-profile-status"}))?),
        ProfileAction::Pull => {
            let mut reply = request(&options, json!({"op": "origin-profile-pull", "origin": origin}))?;
            // Sign-ins are listed, never printed.
            if let Some(changed) = reply["changed"].as_array_mut() {
                for file in changed {
                    if let Some(file) = file.as_object_mut() {
                        file.remove("data");
                    }
                }
            }
            print(reply)
        }
        ProfileAction::Push => push(&options, origin),
    }
}

fn push(options: &HostOptions, origin: &str) -> Result<(), String> {
    let collected = origin_profile::collect_local().map_err(|e| e.to_string())?;
    eprintln!("{}", summary(&collected));
    let manifest = collected.manifest();
    let offer = request(
        options,
        json!({"op": "origin-profile-offer", "origin": origin, "manifest": manifest}),
    )?;
    if offer["ok"] != true {
        return Err(offer["error"].as_str().unwrap_or("The runner refused the profile.").into());
    }
    let wanted = offer["missing"]
        .as_array()
        .map(|missing| {
            missing
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let blobs = collected.blobs(&wanted).map_err(|e| e.to_string())?;
    let bytes = collected
        .files
        .iter()
        .filter(|file| wanted.contains(&file.sha256))
        .map(|file| file.size)
        .sum::<u64>();
    eprintln!(
        "sending {} of {} files ({} KiB) to origin {origin}",
        wanted.len(),
        collected.files.len(),
        bytes / 1024
    );
    let reply = request(
        options,
        json!({"op": "origin-profile-push", "origin": origin, "manifest": manifest, "blobs": blobs}),
    )?;
    print(reply)
}

fn summary(collected: &origin_profile::Collected) -> String {
    let secrets = collected
        .files
        .iter()
        .filter(|file| file.secret)
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    let mut lines = vec![
        format!("origin home: {}", collected.origin_home),
        format!(
            "files: {} ({} KiB), sign-ins and keys: {}",
            collected.files.len(),
            collected.total_bytes() / 1024,
            secrets.join(", ")
        ),
        format!(
            "variables: {}",
            collected.env.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    ];
    let mut tops = collected
        .files
        .iter()
        .map(|file| {
            let mut parts = file.path.splitn(3, '/');
            let first = parts.next().unwrap_or_default();
            match parts.next() {
                Some(second) if parts.next().is_some() => format!("{first}/{second}/"),
                Some(second) => format!("{first}/{second}"),
                None => first.to_owned(),
            }
        })
        .collect::<Vec<_>>();
    tops.dedup();
    lines.push(format!("carries: {}", tops.join(" ")));
    lines.extend(collected.notes.iter().map(|note| format!("note: {note}")));
    lines.join("\n")
}

fn request(options: &HostOptions, body: Value) -> Result<Value, String> {
    let mut stream = daemon::connect(options, false, false).map_err(|e| e.to_string())?;
    daemon::request_within(&mut stream, body, PUSH_TIMEOUT).map_err(|e| e.to_string())
}

fn print(reply: Value) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
    if reply["ok"] == true {
        Ok(())
    } else {
        Err(reply["error"].as_str().unwrap_or("The runner refused.").to_owned())
    }
}
