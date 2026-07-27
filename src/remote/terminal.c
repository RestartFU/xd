#include "terminal.h"

#include "util/host-launch.h"

#include <errno.h>
#include <fcntl.h>
#include <glib-unix.h>
#include <pty.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <unistd.h>

/*
 * Raw terminal bytes are only replayable from their beginning. Once a session
 * reaches this hard ceiling it is ended rather than retaining a tail whose
 * cursor operations and modes no longer have the state they depend on.
 */
#define HISTORY_LIMIT (16 * 1024 * 1024)
#define IO_BUDGET     (64 * 1024)
#define INPUT_LIMIT   (1 * 1024 * 1024)
#define REPLAY_ITEM_LIMIT 65536

struct _XdRemoteTerminal
{
  GObject parent_instance;

  char *id;
  char *chat_id;
  char *title;
  guint columns;
  guint rows;
  GPid pid;
  pid_t session_id;
  int master_fd;
  guint read_id;
  guint write_id;
  guint child_id;
  guint kill_id;
  GPtrArray *replay;       /* XdTerminalReplayItem* */
  gsize replay_bytes;
  GByteArray *pending_input;
  gsize pending_offset;
  gboolean closing;
  gboolean closed;
};

enum
{
  SIGNAL_OUTPUT,
  SIGNAL_CLOSED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (XdRemoteTerminal, xd_remote_terminal, G_TYPE_OBJECT)

static gboolean force_terminal_down (gpointer user_data);
static void schedule_force_down (XdRemoteTerminal *self);

static void
replay_item_free (XdTerminalReplayItem *item)
{
  g_clear_pointer (&item->data, g_bytes_unref);
  g_free (item);
}

static gboolean
record_geometry (XdRemoteTerminal *self,
                 guint             columns,
                 guint             rows)
{
  if (self->replay->len > 0)
    {
      XdTerminalReplayItem *last =
        g_ptr_array_index (self->replay, self->replay->len - 1);

      if (last->data == NULL &&
          last->columns == columns && last->rows == rows)
        return TRUE;
    }

  if (self->replay->len >= REPLAY_ITEM_LIMIT)
    return FALSE;

  XdTerminalReplayItem *item = g_new0 (XdTerminalReplayItem, 1);

  item->columns = columns;
  item->rows = rows;
  g_ptr_array_add (self->replay, item);

  return TRUE;
}

static void
emit_closed (XdRemoteTerminal *self)
{
  if (self->closed)
    return;

  self->closed = TRUE;
  g_signal_emit (self, signals[SIGNAL_CLOSED], 0);
}

static gboolean
record_output (XdRemoteTerminal *self,
               const guint8     *data,
               gsize             length)
{
  g_autoptr (GBytes) bytes = NULL;

  if (length > HISTORY_LIMIT - self->replay_bytes ||
      self->replay->len >= REPLAY_ITEM_LIMIT)
    {
      static const char warning[] =
        "\r\n[xd: terminal closed after exceeding its replay limit]\r\n";
      g_autoptr (GBytes) notice =
        g_bytes_new_static (warning, sizeof warning - 1);

      g_signal_emit (self, signals[SIGNAL_OUTPUT], 0, notice);
      xd_remote_terminal_close (self);
      return FALSE;
    }

  bytes = g_bytes_new (data, length);
  {
    XdTerminalReplayItem *item = g_new0 (XdTerminalReplayItem, 1);

    item->data = g_bytes_ref (bytes);
    g_ptr_array_add (self->replay, item);
    self->replay_bytes += length;
  }
  g_signal_emit (self, signals[SIGNAL_OUTPUT], 0, bytes);

  return TRUE;
}

static gboolean
process_info (const char *pid_name,
              pid_t      *session_id,
              char       *process_state)
{
  g_autofree char *path = g_build_filename ("/proc", pid_name, "stat", NULL);
  g_autofree char *stat = NULL;
  char *after_name;
  char state;
  int parent;
  int group;
  int session;

  if (!g_file_get_contents (path, &stat, NULL, NULL))
    return FALSE;

  /* comm is parenthesized and may itself contain spaces or parentheses. */
  after_name = strrchr (stat, ')');
  if (after_name == NULL ||
      sscanf (after_name + 1, " %c %d %d %d",
              &state, &parent, &group, &session) != 4)
    return FALSE;

  *session_id = (pid_t) session;
  if (process_state != NULL)
    *process_state = state;
  return TRUE;
}

static gboolean
visit_session_members (XdRemoteTerminal *self,
                       int               signal_number)
{
  g_autoptr (GDir) proc = g_dir_open ("/proc", 0, NULL);
  const char *name;
  gboolean found = FALSE;

  while (proc != NULL && (name = g_dir_read_name (proc)) != NULL)
    {
      pid_t session;
      pid_t process;

      if (!g_ascii_isdigit (name[0]) ||
          !process_info (name, &session, NULL) ||
          session != self->session_id)
        continue;

      process = (pid_t) g_ascii_strtoll (name, NULL, 10);
      found = TRUE;
      if (signal_number != 0)
        kill (process, signal_number);
    }

  return found;
}

static gboolean
drain_available_output (XdRemoteTerminal *self)
{
  while (self->master_fd >= 0)
    {
      guint8 buffer[8192];
      ssize_t count = read (self->master_fd, buffer, sizeof buffer);

      if (count > 0)
        {
          if (!record_output (self, buffer, (gsize) count))
            return FALSE;
          continue;
        }
      if (count < 0 && errno == EINTR)
        continue;
      if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
        return TRUE;
      return count == 0;
    }

  return FALSE;
}

static gboolean
on_pty_readable (int          fd,
                 GIOCondition condition,
                 gpointer     user_data)
{
  XdRemoteTerminal *self = user_data;
  guint8 buffer[8192];
  gsize handled = 0;

  while (handled < IO_BUDGET)
    {
      ssize_t count = read (fd, buffer, sizeof buffer);

      if (count > 0)
        {
          handled += (gsize) count;
          if (!record_output (self, buffer, (gsize) count))
            {
              self->read_id = 0;
              return G_SOURCE_REMOVE;
            }
          continue;
        }

      if (count < 0 && errno == EINTR)
        continue;
      if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
        return G_SOURCE_CONTINUE;

      self->read_id = 0;
      return G_SOURCE_REMOVE;
    }

  return G_SOURCE_CONTINUE;
}

static void
on_child_exited (GPid     pid,
                 int      status,
                 gpointer user_data)
{
  XdRemoteTerminal *self = user_data;

  self->child_id = 0;
  self->pid = 0;
  g_spawn_close_pid (pid);

  if (self->read_id != 0)
    {
      g_source_remove (self->read_id);
      self->read_id = 0;
    }
  if (self->write_id != 0)
    {
      g_source_remove (self->write_id);
      self->write_id = 0;
    }

  /*
   * A child can exit after writing more than one read callback's budget. No
   * writer remains now, so draining to EOF is finite and preserves its tail.
   */
  drain_available_output (self);

  if (self->master_fd >= 0)
    {
      close (self->master_fd);
      self->master_fd = -1;
    }

  if (visit_session_members (self, 0))
    {
      self->closing = TRUE;
      visit_session_members (self, SIGHUP);
      schedule_force_down (self);
    }
  else if (self->kill_id != 0)
    {
      g_source_remove (self->kill_id);
      self->kill_id = 0;
    }

  if (self->kill_id == 0)
    emit_closed (self);
}

static const char *
shell_from_environment (GStrv env)
{
  const char *shell = g_environ_getenv (env, "SHELL");

  return shell != NULL && *shell != '\0' ? shell : "/bin/sh";
}

XdRemoteTerminal *
xd_remote_terminal_new (const char  *chat_id,
                        const char  *workdir,
                        guint        columns,
                        guint        rows,
                        GError     **error)
{
  g_autoptr (XdRemoteTerminal) self = NULL;
  g_auto (GStrv) env = NULL;
  struct winsize size = { 0 };
  pid_t pid;
  int master = -1;

  g_return_val_if_fail (chat_id != NULL && *chat_id != '\0', NULL);
  g_return_val_if_fail (workdir != NULL && *workdir != '\0', NULL);

  if (!g_file_test (workdir, G_FILE_TEST_IS_DIR))
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_NOT_DIRECTORY,
                   "%s is not a directory.", workdir);
      return NULL;
    }

  self = g_object_new (XD_TYPE_REMOTE_TERMINAL, NULL);
  self->id = g_uuid_string_random ();
  self->chat_id = g_strdup (chat_id);
  self->title = g_path_get_basename (workdir);
  self->columns = CLAMP (columns, 1, G_MAXUSHORT);
  self->rows = CLAMP (rows, 1, G_MAXUSHORT);

  size.ws_col = (unsigned short) self->columns;
  size.ws_row = (unsigned short) self->rows;
  env = xd_host_environ ();
  env = g_environ_setenv (env, "TERM", "xterm-256color", TRUE);
  env = g_environ_setenv (env, "COLORTERM", "truecolor", TRUE);

  pid = forkpty (&master, NULL, NULL, &size);
  if (pid < 0)
    {
      int saved_errno = errno;

      g_set_error (error, G_IO_ERROR, g_io_error_from_errno (saved_errno),
                   "Cannot open a terminal: %s.", g_strerror (saved_errno));
      return NULL;
    }

  if (pid == 0)
    {
      const char *shell = shell_from_environment (env);
      char *const argv[] = { (char *) shell, NULL };

      if (chdir (workdir) != 0)
        _exit (126);

      execvpe (shell, argv, env);
      _exit (127);
    }

  self->pid = pid;
  self->session_id = pid;
  {
    int flags = fcntl (master, F_GETFL);

    if (flags < 0 ||
        fcntl (master, F_SETFD, FD_CLOEXEC) < 0 ||
        fcntl (master, F_SETFL, flags | O_NONBLOCK) < 0)
      {
        int saved_errno = errno;

        close (master);
        kill (-pid, SIGKILL);
        kill (pid, SIGKILL);
        waitpid (pid, NULL, 0);
        self->pid = 0;
        g_set_error (error, G_IO_ERROR, g_io_error_from_errno (saved_errno),
                     "Cannot configure a terminal: %s.",
                     g_strerror (saved_errno));
        return NULL;
      }
  }

  self->master_fd = master;
  g_assert (record_geometry (self, self->columns, self->rows));
  self->read_id =
    g_unix_fd_add_full (G_PRIORITY_DEFAULT, master,
                        G_IO_IN | G_IO_HUP | G_IO_ERR,
                        on_pty_readable, g_object_ref (self), g_object_unref);
  self->child_id =
    g_child_watch_add_full (G_PRIORITY_DEFAULT, pid, on_child_exited,
                            g_object_ref (self), g_object_unref);

  return g_steal_pointer (&self);
}

const char *
xd_remote_terminal_get_id (XdRemoteTerminal *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), NULL);
  return self->id;
}

const char *
xd_remote_terminal_get_chat_id (XdRemoteTerminal *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), NULL);
  return self->chat_id;
}

const char *
xd_remote_terminal_get_title (XdRemoteTerminal *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), NULL);
  return self->title;
}

guint
xd_remote_terminal_get_columns (XdRemoteTerminal *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), 0);
  return self->columns;
}

guint
xd_remote_terminal_get_rows (XdRemoteTerminal *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), 0);
  return self->rows;
}

gboolean
xd_remote_terminal_is_closing (XdRemoteTerminal *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), TRUE);
  return self->closing || self->closed;
}

GPtrArray *
xd_remote_terminal_get_replay (XdRemoteTerminal *self)
{
  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), NULL);

  return self->replay;
}

static gboolean
flush_pending_input (int          fd,
                     GIOCondition condition,
                     gpointer     user_data)
{
  XdRemoteTerminal *self = user_data;
  gsize handled = 0;

  while (self->pending_offset < self->pending_input->len &&
         handled < IO_BUDGET)
    {
      gsize available = self->pending_input->len - self->pending_offset;
      gsize amount = MIN (available, IO_BUDGET - handled);
      ssize_t count = write (fd, self->pending_input->data + self->pending_offset,
                             amount);

      if (count > 0)
        {
          self->pending_offset += (gsize) count;
          handled += (gsize) count;
          continue;
        }
      if (count < 0 && errno == EINTR)
        continue;
      if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
        return G_SOURCE_CONTINUE;

      self->write_id = 0;
      xd_remote_terminal_close (self);
      return G_SOURCE_REMOVE;
    }

  if (self->pending_offset == self->pending_input->len)
    {
      g_byte_array_set_size (self->pending_input, 0);
      self->pending_offset = 0;
      self->write_id = 0;
      return G_SOURCE_REMOVE;
    }

  return G_SOURCE_CONTINUE;
}

gboolean
xd_remote_terminal_write (XdRemoteTerminal *self,
                          const guint8     *data,
                          gsize             length,
                          GError          **error)
{
  gsize pending;

  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), FALSE);

  if (self->master_fd < 0 || self->closing)
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_CLOSED,
                           "The terminal is closed.");
      return FALSE;
    }

  pending = self->pending_input->len - self->pending_offset;
  if (length > INPUT_LIMIT - MIN (pending, INPUT_LIMIT))
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_NO_SPACE,
                           "The terminal input queue is full.");
      return FALSE;
    }

  /*
   * Admission is based on bytes still waiting, so consumed bytes must not stay
   * in the allocation forever while a busy client replenishes the tail.
   */
  if (self->pending_offset > 0 &&
      (self->pending_offset >= IO_BUDGET ||
       self->pending_input->len + length > INPUT_LIMIT))
    {
      g_byte_array_remove_range (self->pending_input, 0, self->pending_offset);
      self->pending_offset = 0;
    }

  g_byte_array_append (self->pending_input, data, (guint) length);

  if (self->write_id == 0)
    self->write_id =
      g_unix_fd_add_full (G_PRIORITY_DEFAULT, self->master_fd,
                          G_IO_OUT | G_IO_HUP | G_IO_ERR,
                          flush_pending_input, g_object_ref (self),
                          g_object_unref);

  return TRUE;
}

gboolean
xd_remote_terminal_resize (XdRemoteTerminal *self,
                           guint             columns,
                           guint             rows,
                           GError          **error)
{
  struct winsize size = { 0 };
  guint new_columns;
  guint new_rows;

  g_return_val_if_fail (XD_IS_REMOTE_TERMINAL (self), FALSE);

  new_columns = CLAMP (columns, 1, G_MAXUSHORT);
  new_rows = CLAMP (rows, 1, G_MAXUSHORT);

  if (self->replay->len >= REPLAY_ITEM_LIMIT)
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_NO_SPACE,
                           "The terminal replay is full.");
      xd_remote_terminal_close (self);
      return FALSE;
    }

  size.ws_col = (unsigned short) new_columns;
  size.ws_row = (unsigned short) new_rows;

  /*
   * Drain bytes already queued at the old geometry before recording the new
   * one. Never suspend the pty session to make this boundary exact: an
   * interactive shell observes its foreground child stopping and moves that
   * job into the background even if it is immediately continued.
   */
  if (!drain_available_output (self))
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_CLOSED,
                           "The terminal closed while resizing.");
      return FALSE;
    }

  if (ioctl (self->master_fd, TIOCSWINSZ, &size) != 0)
    {
      int saved_errno = errno;

      g_set_error (error, G_IO_ERROR, g_io_error_from_errno (saved_errno),
                   "Cannot resize terminal: %s.", g_strerror (saved_errno));
      return FALSE;
    }

  self->columns = new_columns;
  self->rows = new_rows;
  g_assert (record_geometry (self, new_columns, new_rows));

  return TRUE;
}

static gboolean
force_terminal_down (gpointer user_data)
{
  XdRemoteTerminal *self = user_data;

  self->kill_id = 0;
  visit_session_members (self, SIGKILL);
  emit_closed (self);

  return G_SOURCE_REMOVE;
}

static void
schedule_force_down (XdRemoteTerminal *self)
{
  if (self->kill_id == 0)
    self->kill_id = g_timeout_add_seconds_full (
      G_PRIORITY_DEFAULT, 2, force_terminal_down,
      g_object_ref (self), g_object_unref);
}

void
xd_remote_terminal_close (XdRemoteTerminal *self)
{
  g_return_if_fail (XD_IS_REMOTE_TERMINAL (self));

  if (self->closing || self->closed)
    return;

  self->closing = TRUE;

  if (self->read_id != 0)
    {
      g_source_remove (self->read_id);
      self->read_id = 0;
    }
  if (self->write_id != 0)
    {
      g_source_remove (self->write_id);
      self->write_id = 0;
    }

  if (self->master_fd >= 0)
    {
      close (self->master_fd);
      self->master_fd = -1;
    }

  if (self->pid > 0)
    {
      /* Jobs may have their own process groups, but retain this pty session. */
      visit_session_members (self, SIGHUP);

      /*
       * Shells may trap SIGHUP. Closing a shared tab still has a finite
       * meaning: after a grace period, force the whole pty process group down.
       */
      schedule_force_down (self);
    }
}

static void
xd_remote_terminal_dispose (GObject *object)
{
  XdRemoteTerminal *self = XD_REMOTE_TERMINAL (object);

  xd_remote_terminal_close (self);

  if (self->read_id != 0)
    {
      g_source_remove (self->read_id);
      self->read_id = 0;
    }
  if (self->write_id != 0)
    {
      g_source_remove (self->write_id);
      self->write_id = 0;
    }
  if (self->child_id != 0)
    {
      g_source_remove (self->child_id);
      self->child_id = 0;
    }
  if (self->kill_id != 0)
    {
      g_source_remove (self->kill_id);
      self->kill_id = 0;
    }

  G_OBJECT_CLASS (xd_remote_terminal_parent_class)->dispose (object);
}

static void
xd_remote_terminal_finalize (GObject *object)
{
  XdRemoteTerminal *self = XD_REMOTE_TERMINAL (object);

  if (self->master_fd >= 0)
    close (self->master_fd);

  g_clear_pointer (&self->replay, g_ptr_array_unref);
  g_clear_pointer (&self->pending_input, g_byte_array_unref);
  g_free (self->id);
  g_free (self->chat_id);
  g_free (self->title);

  G_OBJECT_CLASS (xd_remote_terminal_parent_class)->finalize (object);
}

static void
xd_remote_terminal_class_init (XdRemoteTerminalClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = xd_remote_terminal_dispose;
  object_class->finalize = xd_remote_terminal_finalize;

  signals[SIGNAL_OUTPUT] =
    g_signal_new ("output", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_BYTES);
  signals[SIGNAL_CLOSED] =
    g_signal_new ("closed", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 0);
}

static void
xd_remote_terminal_init (XdRemoteTerminal *self)
{
  self->master_fd = -1;
  self->replay =
    g_ptr_array_new_with_free_func ((GDestroyNotify) replay_item_free);
  self->pending_input = g_byte_array_new ();
}
