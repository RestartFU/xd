use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::{Value, json};
use uuid::Uuid;

const MAX_ENTRIES: usize = 256;

pub struct SecretSet {
    pub environment: Vec<(String, String)>,
    pub prompt: Option<String>,
}

pub struct SecretsStore {
    global_path: Option<PathBuf>,
    access: Mutex<()>,
}

impl SecretsStore {
    pub fn new(data_directory: Option<PathBuf>) -> Self {
        let global_path = env::var_os("XD_AGENT_SECRETS_FILE")
            .map(PathBuf::from)
            .or_else(|| data_directory.map(|directory| directory.join("agent-secrets.json")));
        Self {
            global_path,
            access: Mutex::new(()),
        }
    }

    pub fn names(&self, folder_id: Option<&str>) -> Result<Vec<String>, String> {
        let _access = self.lock()?;
        Ok(self.load(&self.path(folder_id)?)?.into_keys().collect())
    }

    pub fn set(&self, folder_id: Option<&str>, entries: &Value) -> Result<(), String> {
        let entries = entries
            .as_array()
            .ok_or_else(|| "set-agent-secrets needs an entries array.".to_string())?;
        if entries.len() > MAX_ENTRIES {
            return Err(format!("At most {MAX_ENTRIES} secrets can be stored."));
        }
        let _access = self.lock()?;
        let path = self.path(folder_id)?;
        let existing = self.load(&path)?;
        let mut desired = BTreeMap::new();
        let mut names = HashSet::new();
        for node in entries {
            let entry = node
                .as_object()
                .ok_or_else(|| "Every secret entry must be an object.".to_string())?;
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| valid_name(name))
                .ok_or_else(|| "A secret has an invalid environment name.".to_string())?;
            if !names.insert(name) {
                return Err("Secret names must be unique.".into());
            }
            let value = match entry.get("value") {
                Some(Value::String(value)) if !value.is_empty() => value.clone(),
                Some(Value::String(_)) => return Err("A replacement secret needs a value.".into()),
                Some(_) => return Err("A secret value must be text.".into()),
                None => existing
                    .get(name)
                    .cloned()
                    .ok_or_else(|| "A new secret needs a value.".to_string())?,
            };
            desired.insert(name.to_owned(), value);
        }
        save(&path, &desired)
    }

    pub fn effective(&self, folder_ids: &[String]) -> Result<SecretSet, String> {
        let _access = self.lock()?;
        let Some(global) = self.global_path.as_ref() else {
            return Ok(SecretSet {
                environment: Vec::new(),
                prompt: None,
            });
        };
        let mut values = self.load(global)?;
        for folder_id in folder_ids {
            for (name, value) in self.load(&folder_path(global, folder_id))? {
                if !values.contains_key(&name) && values.len() >= MAX_ENTRIES {
                    return Err(format!(
                        "Effective secret set exceeds {MAX_ENTRIES} entries."
                    ));
                }
                values.insert(name, value);
            }
        }
        let names = values.keys().cloned().collect::<Vec<_>>();
        let prompt = (!names.is_empty()).then(|| {
            format!(
                "[Agent secrets available as environment variables: {}. Their values are not included in this prompt. Use them when needed, and never print or expose their values.]",
                names.join(", ")
            )
        });
        Ok(SecretSet {
            environment: values.into_iter().collect(),
            prompt,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.access
            .lock()
            .map_err(|_| "Agent secrets storage is unavailable.".to_string())
    }

    fn path(&self, folder_id: Option<&str>) -> Result<PathBuf, String> {
        let global = self
            .global_path
            .as_ref()
            .ok_or_else(|| "Agent secrets storage is not configured.".to_string())?;
        match folder_id {
            Some("") => Err("A folder id cannot be empty.".into()),
            Some(folder_id) => Ok(folder_path(global, folder_id)),
            None => Ok(global.clone()),
        }
    }

    fn load(&self, path: &Path) -> Result<BTreeMap<String, String>, String> {
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let text = fs::read_to_string(path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        let root = serde_json::from_str::<Value>(&text)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        let secrets = root
            .get("secrets")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{} has no secrets object", path.display()))?;
        if secrets.len() > MAX_ENTRIES {
            return Err(format!(
                "{} contains more than {MAX_ENTRIES} secrets",
                path.display()
            ));
        }
        let mut values = BTreeMap::new();
        for (name, value) in secrets {
            let value = value
                .as_str()
                .filter(|value| !value.is_empty())
                .filter(|_| valid_name(name))
                .ok_or_else(|| format!("{} contains an invalid secret entry", path.display()))?;
            values.insert(name.clone(), value.to_owned());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Cannot secure {}: {error}", path.display()))?;
        Ok(values)
    }
}

fn save(path: &Path, values: &BTreeMap<String, String>) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Agent secrets path has no parent directory.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Cannot secure {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("Cannot save {}: {error}", path.display()))?;
        serde_json::to_writer_pretty(&mut file, &json!({"version": 1, "secrets": values}))
            .map_err(|error| format!("Cannot save {}: {error}", path.display()))?;
        file.write_all(b"\n")
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("Cannot save {}: {error}", path.display()))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("Cannot save {}: {error}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Cannot secure {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn folder_path(global: &Path, folder_id: &str) -> PathBuf {
    let directory = PathBuf::from(format!("{}.d", global.display()));
    directory.join(format!("{}.json", sha256_hex(folder_id.as_bytes())))
}

// Kept local to avoid a dependency for the persisted folder-secret filename.
fn sha256_hex(input: &[u8]) -> String {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    let mut hash = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("four bytes"));
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (value, addition) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *value = value.wrapping_add(addition);
        }
    }
    hash.into_iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory() -> PathBuf {
        let path = env::temp_dir().join(format!("xd-secret-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn sha256_matches_the_persisted_folder_filename() {
        assert_eq!(
            sha256_hex(b"folder-1"),
            "77a70a8db9013a7bc1fe10eef636f80f615dae8a11ed4eb4833c62daf59fb39c"
        );
    }

    #[test]
    fn preserves_omitted_existing_values_and_never_returns_them() {
        let root = temporary_directory();
        let store = SecretsStore::new(Some(root.clone()));
        store
            .set(None, &json!([{"name": "API_TOKEN", "value": "private"}]))
            .unwrap();
        assert_eq!(store.names(None).unwrap(), vec!["API_TOKEN"]);
        store.set(None, &json!([{"name": "API_TOKEN"}])).unwrap();
        assert_eq!(
            store.effective(&[]).unwrap().environment,
            vec![("API_TOKEN".into(), "private".into())]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn folder_values_override_global_values_in_order() {
        let root = temporary_directory();
        let store = SecretsStore::new(Some(root.clone()));
        store
            .set(None, &json!([{"name": "TOKEN", "value": "global"}]))
            .unwrap();
        store
            .set(
                Some("parent"),
                &json!([
                    {"name": "TOKEN", "value": "parent"},
                    {"name": "PARENT_ONLY", "value": "yes"}
                ]),
            )
            .unwrap();
        store
            .set(Some("child"), &json!([{"name": "TOKEN", "value": "child"}]))
            .unwrap();
        let effective = store.effective(&["parent".into(), "child".into()]).unwrap();
        assert!(
            effective
                .environment
                .contains(&("TOKEN".into(), "child".into()))
        );
        assert!(effective.prompt.unwrap().contains("PARENT_ONLY, TOKEN"));
        let _ = fs::remove_dir_all(root);
    }
}
