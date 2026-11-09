# Sessions and layout

xd organizes work as **project → session → chat tabs**. A session owns one
workspace, usually a Git branch or worktree, and each chat tab in it keeps its
own history, draft, assistant, and model.

## Side pane and terminal

Each chat remembers its browser pages and whether the side pane is open. The
session header has two layout controls: side pane and bottom terminal. Hover over
either icon to see its label.

- **Side pane** starts with a browser page. Use its `+` menu to add another page,
  **Files**, or **Changes**.
- **Bottom terminal** opens a shell below the chat.

Both panes can stay open together, and hiding them keeps their content.

The integrated browser keeps separate tabs for each chat and shares site data
between them. Swipe horizontally on a supported touchpad to go back or forward.

## Settling sessions

Hover over a session in the sidebar and press its check mark to settle it, or use
its right-click menu. The session moves to the **Settled** shelf below your
projects without being deleted, and **Undo** appears beside the sidebar's
Settings button for five seconds. If it was the open session, the next one down
opens. A session can't be settled while it is running, has a queued message, or
is waiting on an approval or question.

Press a settled session's undo arrow to bring it back. Sending it a message does
the same, and hands it back to the automatic rules.

Settling only changes where a session is listed: its terminals and anything its
agent is still doing keep running. The host stops an agent's process once it has
nothing left to do, within two minutes of settling or after 30 idle minutes
otherwise, and the next message resumes the conversation.

On Android, settle a session from its **⋮** menu or the chat tab menu. The
**Settled** shelf sits below the projects, and the auto-settle rules are in
Settings.

### Auto-settle

By default the host settles sessions after three days without activity and when
their pull request merges or closes. Change this in **Settings → General →
Auto-settle**. Those rules belong to the host, so they keep running while the app
is closed, and each host keeps its own. To keep one session out of them, choose
**Disable auto-settle** from its right-click menu.

## Auto-delete

The host can also delete sessions for you. In **Settings → General →
Auto-delete**, turn on **Auto-delete inactive sessions** and choose how many days
a session may sit idle. It is off until you do, because deleting can't be undone.

- The session goes with its chats and terminals, and so does the worktree xd made
  for it, but its branch and every commit on it stay in the repository.
- A worktree with uncommitted changes or untracked files keeps its whole session
  until you commit or clean it up, and so do commits on a detached HEAD that no
  branch holds.
- Using a session's terminal counts as activity.
- Sessions you've kept out of auto-settle are not deleted, and neither is one you
  brought back from the **Settled** shelf, until it sees new activity.
