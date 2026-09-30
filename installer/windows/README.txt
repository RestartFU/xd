xd for Windows x86_64

Extract the entire archive and run xd.exe. Keep libexec beside the executable.
Stable and nightly builds have separate settings, chats, and workspaces.

The desktop window and integrated browser run natively on Windows. Local
workspaces, agents, and persistent tmux sessions run in your default WSL Linux
distribution. Install WSL and a Linux distribution before starting xd:

  https://learn.microsoft.com/windows/wsl/install

In your WSL distribution, install git, tmux, OpenSSH client, and CA certificates.
For Ubuntu or Debian:

  sudo apt update
  sudo apt install git tmux openssh-client ca-certificates

Install Codex, Claude Code, JCode, and/or GitHub Copilot CLI inside that same
distribution and make them available on PATH. xd includes its matching Linux
host; a separate xd installation in WSL is unnecessary. Existing SSH connections
use the Windows OpenSSH client.

Set XD_WSL_DISTRIBUTION to select another installed distribution. Local files
and projects use Linux paths, for example /home/yourname/project. Browser
localhost addresses refer to Windows; WSL 2 forwards local servers by default.
Remote servers still need an SSH port forward or a reachable URL.

The browser requires Microsoft's Evergreen WebView2 Runtime. Windows 11 usually
includes it. If missing, run the included helper explicitly in PowerShell:

  powershell -ExecutionPolicy Bypass -File .\install-webview2.ps1

The helper downloads Microsoft's bootstrapper, checks its Microsoft signature,
and installs WebView2. It does not install WSL or change your Linux distribution.

To update, close xd and extract the newer archive into the application directory.
Your application data is stored separately from the extracted files.
