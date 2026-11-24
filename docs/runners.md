# Runners (design)

Status: proposal, branch `feat/host-inherited-runners`. Phase 1 (the spike) is
built; see [Spike results](#spike-results).

## Goal

Add compute without setting up your stack again. A **runner** is a machine that
runs xd sessions as if they were running on the device you work from: your git
credentials, global agent rules, skills, MCP servers, and Claude and Codex
logins all come with the work. The runner needs only `xd-host` and the
toolchains your projects build with.

This replaces the standalone daemon. Today one daemon owns every session, and
using another machine means installing and signing in to everything there by
hand. After this change:

- **Runners** own sessions: history, terminals, worktrees and running turns.
  They keep working while every client is closed.
- **Clients** (desktop and Android) are thin. They gather sessions from every
  runner on your account and show them in one sidebar.
- The **local runner** is what the daemon becomes: the desktop starts it on
  demand, and it uses the local environment directly.
- A central **control plane**, one multi-tenant service run by xd's
  maintainer for every user, holds accounts (GitHub sign-in) and the runners
  registered to each. It only keeps the registry: clients connect to runners
  directly, and environments and session state never reach it.

```text
                 ┌────────────────── central control plane ──────────────┐
                 │ GitHub OAuth · account → runners registry · heartbeats│
                 └───────▲───────────────────▲───────────────────▲───────┘
                         │ list runners      │ list runners      │ register / heartbeat
                         │                   │                   │
   desktop (origin) ─────┼── pinned TLS ─────┼──────────────► runner A (VPS)
     pushes its          │                   │                    sessions, worktrees,
     origin profile      │                   │                    agents, terminals
                         │                   │
   Android ──────────────┴── pinned TLS ─────┴──────────────► runner B (box)
     uses the profile a runner already holds
```

Secrets never pass through the control plane. Clients talk to runners directly.

## Identity and authentication

- **Account** = a GitHub user, identified by the numeric GitHub user id rather
  than the login, which can be renamed.
- **Clients** sign in with the GitHub OAuth device flow, using the minimal
  scope (`read:user`). The control plane exchanges that for its own session
  token, which the client uses to list runners.
- **Runners** sign in once with `xd-host runner login`, using the same device
  flow. That binds the runner to its owner's GitHub id. The runner then
  registers its name, reachable addresses, TLS certificate fingerprint and
  version, and sends a heartbeat.
- **Client → runner:** the client gets the runner's address and fingerprint
  from the registry, pins the certificate, and presents its GitHub token. The
  runner checks it with `GET https://api.github.com/user` and admits the client
  only if the id matches its owner. It then issues a revocable runner device
  token, so later connections don't need GitHub. Revoking the device on the
  runner, or signing out on the control plane, cuts it off.
- Pairing codes stay available for runners that aren't registered to an
  account (offline or air-gapped setups).

## Control plane

One central, multi-tenant service operated by xd's maintainer, used by every
xd install. It stores only:

- `accounts`: GitHub id, login, avatar.
- `runners`: id, owner, name, addresses, fingerprint, version, OS and
  architecture, last heartbeat, and capacity (CPU, memory, running sessions).
- `client_sessions`: control-plane tokens issued to signed-in clients.

API sketch (JSON over HTTPS):

| Call | Who | Purpose |
|---|---|---|
| `POST /auth/github/device` + `/poll` | client, runner | device flow, returns a control-plane token |
| `GET /runners` | client | runners on the account, with liveness |
| `PUT /runners/{id}` | runner | register or update addresses, fingerprint, version |
| `POST /runners/{id}/heartbeat` | runner | liveness and load |
| `DELETE /runners/{id}` | client, runner | remove from the account |

Out of scope by decision: relaying connections, storing environments, and
storing session state. Possible later: push notifications for Android, and a
relay for runners with no reachable address.

## The origin profile

The origin is the device whose environment a session inherits. Usually that's
your main desktop. A runner holds one **origin profile** per origin. Each lives
in its own home, `DATA/origins/<origin-id>/home`, with mode 0700. The runner's
own home is never touched, and origins never share a profile.

### What it contains

| Part | Source on the origin | On the runner |
|---|---|---|
| Claude config | `~/.claude` (CLAUDE.md, skills, agents, commands, plugins, settings), `~/.claude.json` (MCP servers) | `CLAUDE_CONFIG_DIR` |
| Codex config | `~/.codex` (AGENTS.md, config.toml, skills, prompts) | `CODEX_HOME` |
| Agent logins | Claude `.credentials.json` (macOS: keychain item), Codex `auth.json` | inside those homes, mode 0600 |
| Git | `~/.gitconfig`, `~/.config/git/`, credentials from the configured helper for remotes in use | `GIT_CONFIG_GLOBAL`, plus a credential store in the profile |
| GitHub CLI | `~/.config/gh` | `GH_CONFIG_DIR` |
| Shell env | the login shell's environment, filtered | the agent's and terminal's environment |
| xd settings | account and agent preferences | the runner's settings for that origin |

`~/.ssh` comes along whole, keys included. The runner must push and pull
over SSH as the origin does while the origin is offline. Every file there
except `config`, `known_hosts*`, `authorized_keys` and `*.pub` is treated as
a key, whatever it is called: written mode 0600, never rewritten, and passed
to `ssh` with `-i`. Pushing a profile to a runner therefore hands it the
origin's keys, so runners are machines the user owns.

Excluded: transcripts and caches (`projects/`, `sessions/`, `todos/`,
`shell-snapshots/`, `statsig/`, logs, history files), and anything
machine-bound (`DISPLAY`, `XDG_RUNTIME_DIR`, `SSH_AUTH_SOCK`, `PATH`).

### Sync

- Content-addressed. The client sends a manifest (path, mode, sha256, kind);
  the runner replies with the hashes it lacks; the client uploads only those.
  The runner applies the update atomically, so a turn never sees half a profile.
- The client syncs when it connects, when files change on the origin (watched),
  and before starting a turn it initiated.
- Paths inside config files that point into the origin's home are rewritten to
  the profile home.
- Android never pushes a profile. A session started from the phone uses the
  account's **primary origin profile** on that runner (chosen in settings; by
  default the one pushed most recently).

### Working while the origin is offline

Runners keep working when every client is closed, so the profile is a cached
copy, not a live forward:

- Git uses the cached credential store, not a forwarded helper. A forwarded
  ssh-agent is used only while the origin is connected, so HTTPS remotes with
  the gh token are the reliable path.
- **Login refresh** is the hard part. Claude and Codex rotate their refresh
  tokens, so two copies that refresh independently log each other out. Plan:
  every profile tracks a version for each credential. When a runner's CLI
  refreshes, the runner reports the new file to the origin at its next
  connection, and the newer one wins on both sides. The spike has to measure
  how often this happens and whether a dedicated per-runner login (an extra
  xd account, as `accounts.rs` already supports) is the safer default.

### MCP servers and hooks

Each stdio MCP server and command hook runs wherever it behaves exactly as it
does on the origin:

- **On the runner**, when everything it names inside the origin's home came
  along whole: the program, the folder it belongs to (its checkout or package,
  up to 64 MiB), the files a wrapper starts in turn, and the folders its
  arguments and variables point to. Such a server keeps working while the
  origin is offline.
- **Hosted on the origin** otherwise. That covers Python virtualenvs, a hook
  inside a checkout too large to carry, and a server whose data folder only
  the origin has. The origin keeps the real command under an id. The
  runner's configuration starts `~/.xd/origin-exec <id>` instead, and its
  standard input, output, error and exit code are tunneled through the
  origin's link (`origin-link`, `origin-exec-*`). The runner only ever names
  an id, the origin runs only programs its own configuration offered, and the
  server's variables never leave the origin. A hosted program is available
  while the origin is linked. Without the link, its MCP server fails to start
  and its hook fails closed.

Codex runs a hook only while `[hooks.state]` holds its current hash. The
runner recomputes the hash after rewriting a hook, for exactly the hooks the
origin trusts. A hook the origin doesn't trust stays untrusted on the runner
too.

### What can't be inherited

Binaries and toolchains. They differ across OS and architecture. xd installs
the same Claude and Codex CLI versions the origin uses. Project toolchains
(cargo, node, go…) are the runner's job, possibly later from a devcontainer or
Dockerfile in the repository.

## Sessions on runners

- **Placement.** A new session picks a runner, local by default, from the same
  picker on desktop and Android. A session stays on its runner. Moving it
  is an explicit action that transfers the worktree (see below) and the
  history.
- **Workspace.** The runner clones the project's remote with the profile's
  credentials, under `DATA/origins/<origin-id>/repos`, and makes worktrees
  there. Starting from a local branch with unpushed commits or uncommitted
  changes sends a `git bundle` and a patch, so the session starts exactly where
  the origin was.
- **State.** History, queues, approvals, terminals and subagent records live
  in the runner's store, as they do in the daemon today. The client merges
  `tree` snapshots from every connected runner and tags each session with its
  runner.
- **Browser and previews.** The browser stays on the client. The agent's
  browser tools already go through the host to the client. Dev-server ports on
  a runner get forwarded to the client later.

## Code changes

1. **Host → runner.** Most of today's daemon *is* the runner: store, turns,
   terminals, pairing over TLS, device tokens. New parts:
   - `origin_profile`: collect (client side), the manifest diff, atomic
     apply (runner side), credential versioning.
   - An environment per session: `user_environment` and `accounts::user_home`
     currently assume "the machine the host runs on". They need to take the
     session's origin profile: `HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
     `GIT_CONFIG_GLOBAL`, `GH_CONFIG_DIR`, the filtered env. The same applies
     to every agent, terminal, git and helper process a session starts.
   - Registration and heartbeat to the control plane, and GitHub-token
     admission next to device tokens.
2. **Clients.** Connect to several runners at once and merge their trees, run
   GitHub sign-in, show a runner picker and badge, and (desktop only) collect
   and push the origin profile. Desktop and Android ship together.
3. **Control plane.** A new service (a separate crate or repository): GitHub
   OAuth device flow, the registry, heartbeats.

## Phases

1. **Spike, host only.** Collect a profile on this machine, push it to a
   second `xd-host` running with another data directory and home, and run a
   Claude and a Codex chat there that see the inherited CLAUDE.md, skills,
   MCP servers, git identity and push access. Measure login refresh behavior.
2. **Origin profiles for real.** Wire ops, the manifest diff, file watching,
   per-session environments for every process a session starts, and multiple
   origins on one runner.
3. **Control plane and GitHub sign-in.** The service, `xd-host runner
   login`, registration, GitHub-token admission.
4. **Multi-runner clients.** Desktop and Android connect to every runner,
   merge sessions, show a placement picker and runner badges. The local
   runner replaces the daemon.
5. **Mobility.** Move sessions between runners, origin-hosted MCP servers,
   port forwarding, relay.

## Spike results

Built in `host/src/origin_profile.rs` and `xd-host profile push|pull|status|show`.
It was tested on one VPS. The runner was a second `xd-host daemon` started with
`env -i`, an empty `HOME`, and its own data directory. Then the real profile of
the machine's user was pushed to it, and Claude and Codex chats were driven
over the protocol.

**What works**

- Size: the first push is 4,890 files and 214 MiB in about 20 s over the local
  socket. A push with nothing changed sends 0 files in about 3 s, almost all
  of it hashing. A push after two skills were added sent only their 4 files.
- Claude on the runner had the origin's global CLAUDE.md, all 30 skills
  (including skills symlinked into the home from a checkout) and the plugins.
  It was signed in to the origin's claude.ai account, and that account's
  connectors appeared. It also got the HTTP MCP server, a stdio MCP server
  carried with its runtime and project folder (bun plus a 53 MiB Node
  project), and xd's own tools.
- Codex on the runner had the origin's AGENTS.md, its skills (symlinked ones
  too) and its MCP servers, and was signed in with ChatGPT.
- With no `SSH_AUTH_SOCK` and an empty home, `git ls-remote` over SSH
  authenticated with the inherited key, including through the origin's
  `url.ssh://…insteadOf https://` rewrite. `gh auth status` showed the origin's
  account.
- The login-shell variables came from a clean `env -i` login shell, so only
  what the user's startup files set came along. Variables pointing into the
  origin's home (`CARGO_HOME`, `NPM_CONFIG_PREFIX`, …) were left out on purpose.
- Neither CLI refreshed its login during the test (the Claude token had 7.5 h
  left), so the origin's sign-ins were untouched.

**Hosted MCP servers and hooks (second round)**

The origin ran `xd-host profile link`, which pushes the profile and keeps the
link open. It found 4 programs to host: the `discord` and `src-mcp` venvs,
`minecraft` (its game data stays on the origin), and the Cinnabar dispatch
hook. On the runner (empty home, `env -i`):

- Claude listed `src-mcp`, `discord`, `minecraft`, `lens` and `xd` with no
  startup failures. It called `src-mcp project_list` (the indexed `artemis`
  project), `discord list_guilds` (the account's servers) and `minecraft
  list` (installed version, no instances). Its Bash ran, with the PreToolUse
  hook answered on the origin. `git ls-remote` over SSH and `gh api user`
  (`RestartFU`) worked.
- Codex called `src-mcp` and `discord` the same way and ran the same shell
  commands. It has no `minecraft` server, and neither does the origin's Codex
  config. Its Cinnabar hooks are untrusted on the origin (their trust hashes
  were made for an older path), so they're untrusted on the runner too.
- Neither CLI refreshed a login.

**What the first round found, and what it meant for the design**

- **Python virtualenvs aren't portable.** Two MCP servers launch from
  `.venv`s (one through a wrapper in `~/.local/bin`). A venv hardcodes its
  interpreter and paths, so it can't be copied. These are now hosted on the
  origin. Rebuilding a venv on the runner (`uv sync`) could later let them
  work offline too.
- **Some hooks are bound to the origin.** A PreToolUse hook whose script lives
  in a 2 GiB private checkout was carried alone and failed closed, so Claude's
  Bash was blocked on the runner. It's now hosted on the origin. A hook still
  sees the runner's paths in its event (its `cwd`), so a hook that decides by
  path, such as "is this one of my repositories", decides by the runner's
  paths.
- **Process coverage.** The host's own `gh`/`git` helpers in `storage.rs` used
  the daemon's process environment, so pull-request lookups ran signed out.
  The spike puts the profile on `run_git_with_env` and `run_gh`. The roughly 40
  other direct `Command::new("git")` sites (settlement, deletion, CI watch,
  worktrees…) still don't have it. That is the phase 2 Executor work: every
  process a session starts goes through one place that applies its origin.
- **Codex** warns that it won't create PATH helpers under `/tmp` when
  `CODEX_HOME` is there. That only happened in the test; real profiles live
  in the data directory.
- **Git identity** in repository-local config (`.git/config`) is not part of
  the profile. Only global config is. A fresh clone on a runner gets the
  origin's global identity, so a repo-local identity has to come with the
  workspace transfer.

**Spike shortcuts to replace in phase 2**

- One runner-wide *active* origin (the last one pushed with `activate`)
  instead of each session carrying its origin. Several origins can be stored,
  but only one is applied at a time, and only a local request can switch it:
  a paired device's push is stored, never activated.
- Direct `git` spawns outside the storage helpers (worktree setup, the CI
  watch) still run with the runner's own environment until the Executor
  boundary lands.
- One request carries every changed body. Real sync streams the bodies in
  chunks.
- `profile push` reaches the runner over its local socket. Paired devices get
  the same `origin-profile-*` operations over TLS: the origin is the device's
  id there, and a device can only push its own profile.
- `pull` reports logins the runner refreshed, but the origin doesn't adopt
  them back yet.
- On macOS, Claude's login is in the keychain and isn't read yet.
- Hosted programs run in the origin's home with its login environment.
  Variables the CLI sets for a hook (`CLAUDE_PROJECT_DIR`) aren't forwarded
  yet.

## Open questions

- Login refresh: shared, versioned credentials versus a dedicated login per
  runner. Decided by the spike.
- How long a runner keeps an origin's cached secrets after that origin has
  stopped connecting, and whether removing a runner from the account wipes
  them remotely.
- Whether the control plane is a crate in this repository or its own
  repository, and where it's hosted.
- Abuse limits for a public multi-tenant service (runners per account,
  heartbeat rate).
