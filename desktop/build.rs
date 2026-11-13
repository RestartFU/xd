//! Gives the Windows executable its icon and the name Windows shows for it.
//!
//! GPUI takes a window's icon from icon resource 1 of the executable, and
//! Explorer, the taskbar, Alt+Tab and the installer's shortcuts read the same
//! one. Task Manager and the "Open with" list name the program from its
//! version resource. Without either, xd showed as a blank icon called
//! "xd.exe".

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Resources are compiled by the Windows SDK, so only on a Windows machine,
    // exactly as GPUI embeds its manifest.
    #[cfg(windows)]
    windows::embed();
}

#[cfg(windows)]
mod windows {
    use std::{env, fs, path::PathBuf};

    pub fn embed() {
        if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let icon = manifest.join("../installer/windows/xd.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        println!("cargo:rerun-if-env-changed=XD_BUILD_PROFILE");

        let version = env::var("CARGO_PKG_VERSION").unwrap();
        let nightly = env::var("XD_BUILD_PROFILE").as_deref() == Ok("nightly");
        let script = resource_script(
            // The resource compiler reads a backslash as an escape.
            &icon.to_string_lossy().replace('\\', "/"),
            &version,
            if nightly { "xd Nightly" } else { "xd" },
        );
        let path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("xd.rc");
        fs::write(&path, script).unwrap();
        embed_resource::compile(&path, embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }

    /// `1 ICON` is the resource GPUI asks for. No manifest: GPUI embeds one.
    fn resource_script(icon: &str, version: &str, product: &str) -> String {
        let mut fields = version
            .split(['.', '-', '+'])
            .map(|field| field.parse::<u16>().unwrap_or(0))
            .chain(std::iter::repeat(0));
        let numeric = format!(
            "{},{},{},{}",
            fields.next().unwrap(),
            fields.next().unwrap(),
            fields.next().unwrap(),
            0
        );
        format!(
            r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "RestartFU"
            VALUE "FileDescription", "{product}"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "xd"
            VALUE "LegalCopyright", "MIT License"
            VALUE "OriginalFilename", "xd.exe"
            VALUE "ProductName", "{product}"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
        )
    }
}
