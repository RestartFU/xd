#[cfg(target_os = "linux")]
mod platform {
    use std::{
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    #[derive(Default)]
    pub struct SpeechOutput {
        current: Arc<Mutex<Option<Child>>>,
    }

    impl SpeechOutput {
        pub fn speak(&self, text: &str) {
            self.stop();
            let child = ["espeak-ng", "espeak"].into_iter().find_map(|program| {
                Command::new(program)
                    .arg(text)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()
            });
            let Some(child) = child else {
                return;
            };
            let id = child.id();
            if let Ok(mut current) = self.current.lock() {
                *current = Some(child);
            } else {
                return;
            }
            let current = self.current.clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(50));
                    let Ok(mut child) = current.lock() else {
                        return;
                    };
                    let Some(active) = child.as_mut() else {
                        return;
                    };
                    if active.id() != id {
                        return;
                    }
                    match active.try_wait() {
                        Ok(Some(_)) | Err(_) => {
                            child.take();
                            return;
                        }
                        Ok(None) => {}
                    }
                }
            });
        }

        pub fn stop(&self) {
            if let Ok(mut current) = self.current.lock()
                && let Some(mut child) = current.take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for SpeechOutput {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{
        io::Write,
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    #[derive(Default)]
    pub struct SpeechOutput {
        current: Arc<Mutex<Option<Child>>>,
    }

    impl SpeechOutput {
        pub fn speak(&self, text: &str) {
            self.stop();
            let Ok(mut child) = Command::new("/usr/bin/say")
                .args(["-f", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            let wrote_text = child
                .stdin
                .take()
                .is_some_and(|mut input| input.write_all(text.as_bytes()).is_ok());
            if !wrote_text {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            let id = child.id();
            if let Ok(mut current) = self.current.lock() {
                *current = Some(child);
            } else {
                return;
            }
            let current = self.current.clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(50));
                    let Ok(mut child) = current.lock() else {
                        return;
                    };
                    let Some(active) = child.as_mut() else {
                        return;
                    };
                    if active.id() != id {
                        return;
                    }
                    match active.try_wait() {
                        Ok(Some(_)) | Err(_) => {
                            child.take();
                            return;
                        }
                        Ok(None) => {}
                    }
                }
            });
        }

        pub fn stop(&self) {
            if let Ok(mut current) = self.current.lock()
                && let Some(mut child) = current.take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for SpeechOutput {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        io::Write,
        os::windows::process::CommandExt,
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SPEAK_SCRIPT: &str = r#"
Add-Type -AssemblyName System.Speech
$text = [Console]::In.ReadToEnd()
if ($text.Length -eq 0) { exit 0 }
$voice = New-Object System.Speech.Synthesis.SpeechSynthesizer
try { $voice.Speak($text) } finally { $voice.Dispose() }
"#;

    #[derive(Default)]
    pub struct SpeechOutput {
        current: Arc<Mutex<Option<Child>>>,
    }

    impl SpeechOutput {
        pub fn speak(&self, text: &str) {
            self.stop();
            let Ok(mut child) = Command::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-WindowStyle",
                    "Hidden",
                    "-Command",
                    SPEAK_SCRIPT,
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            let wrote_text = child
                .stdin
                .take()
                .is_some_and(|mut input| input.write_all(text.as_bytes()).is_ok());
            if !wrote_text {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            let id = child.id();
            if let Ok(mut current) = self.current.lock() {
                *current = Some(child);
            } else {
                return;
            }
            let current = self.current.clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(50));
                    let Ok(mut child) = current.lock() else {
                        return;
                    };
                    let Some(active) = child.as_mut() else {
                        return;
                    };
                    if active.id() != id {
                        return;
                    }
                    match active.try_wait() {
                        Ok(Some(_)) | Err(_) => {
                            child.take();
                            return;
                        }
                        Ok(None) => {}
                    }
                }
            });
        }

        pub fn stop(&self) {
            if let Ok(mut current) = self.current.lock()
                && let Some(mut child) = current.take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for SpeechOutput {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    #[derive(Default)]
    pub struct SpeechOutput;

    impl SpeechOutput {
        pub fn speak(&self, _: &str) {}
        pub fn stop(&self) {}
    }
}

pub use platform::SpeechOutput;
