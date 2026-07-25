#include "chat-session.h"

#include <signal.h>

/* How long a polite SIGINT gets before the process is killed outright. */
#define STOP_GRACE_SECONDS 2

/* Enough stderr to explain a failure without holding a runaway log in memory. */
#define STDERR_LIMIT 8192

struct _HyChatSession
{
  GObject parent_instance;

  const AiBackend *backend;
  AiParser *parser;

  GSubprocess *process;
  GDataInputStream *stdout_stream;
  GDataInputStream *stderr_stream;
  GCancellable *cancellable;
  GString *stderr_text;

  guint kill_timeout_id;
  gboolean stopping;
  gboolean finished;
};

enum
{
  SIGNAL_SESSION_STARTED,
  SIGNAL_TEXT_DELTA,
  SIGNAL_TOOL_USE,
  SIGNAL_FINISHED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (HyChatSession, hy_chat_session, G_TYPE_OBJECT)

static void read_next_line (HyChatSession *self);

/* --- finishing ------------------------------------------------------------ */

static void
finish (HyChatSession *self,
        gboolean       success,
        const char    *message)
{
  if (self->finished)
    return;

  self->finished = TRUE;
  g_clear_handle_id (&self->kill_timeout_id, g_source_remove);

  g_signal_emit (self, signals[SIGNAL_FINISHED], 0, success,
                 message != NULL ? message : "");
}

/* The tail of stderr is what actually explains a non-zero exit. */
static const char *
stderr_tail (HyChatSession *self)
{
  g_strstrip (self->stderr_text->str);

  return *self->stderr_text->str != '\0' ? self->stderr_text->str : NULL;
}

static void
on_process_waited (GObject      *source,
                   GAsyncResult *result,
                   gpointer      user_data)
{
  g_autoptr (HyChatSession) self = user_data;
  g_autoptr (GError) error = NULL;

  if (g_subprocess_wait_check_finish (G_SUBPROCESS (source), result, &error))
    {
      finish (self, TRUE, NULL);
      return;
    }

  /* A cancelled turn is a normal outcome, not a failure to report. */
  if (self->stopping)
    {
      finish (self, TRUE, NULL);
      return;
    }

  {
    const char *tail = stderr_tail (self);

    finish (self, FALSE, tail != NULL ? tail : error->message);
  }
}

static void
reap_process (HyChatSession *self)
{
  g_subprocess_wait_check_async (self->process, NULL, on_process_waited,
                                 g_object_ref (self));
}

/* --- reading -------------------------------------------------------------- */

static void
on_event (const AiEvent *event,
          gpointer       user_data)
{
  HyChatSession *self = user_data;

  switch (event->type)
    {
    case AI_EVENT_SESSION_STARTED:
      g_signal_emit (self, signals[SIGNAL_SESSION_STARTED], 0, event->session_id);
      break;

    case AI_EVENT_TEXT_DELTA:
      if (event->text != NULL)
        g_signal_emit (self, signals[SIGNAL_TEXT_DELTA], 0, event->text);
      break;

    case AI_EVENT_TOOL_USE:
      g_signal_emit (self, signals[SIGNAL_TOOL_USE], 0,
                     event->text != NULL ? event->text : "tool");
      break;

    case AI_EVENT_RESULT:
      /* The backend reported the id only at the end; keep it either way. */
      if (event->session_id != NULL)
        g_signal_emit (self, signals[SIGNAL_SESSION_STARTED], 0, event->session_id);
      break;

    case AI_EVENT_ERROR:
      /* Interrupting a CLI makes it report the turn as failed, which is true
       * from its side and misleading from ours: the user stopped it, and
       * being told the backend died is both wrong and alarming. */
      finish (self, self->stopping, self->stopping ? NULL : event->text);
      break;

    default:
      break;
    }
}

static void
on_line_read (GObject      *source,
              GAsyncResult *result,
              gpointer      user_data)
{
  g_autoptr (HyChatSession) self = user_data;
  g_autoptr (GError) error = NULL;
  g_autofree char *line = NULL;
  gsize length = 0;

  line = g_data_input_stream_read_line_finish_utf8 (G_DATA_INPUT_STREAM (source),
                                                    result, &length, &error);

  if (error != NULL)
    {
      if (!g_error_matches (error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
        g_debug ("%s: read failed: %s", self->backend->id, error->message);

      reap_process (self);
      return;
    }

  if (line == NULL)
    {
      /* End of output: the child is done talking. */
      reap_process (self);
      return;
    }

  ai_parser_feed_line (self->parser, line, on_event, self);

  read_next_line (self);
}

static void
read_next_line (HyChatSession *self)
{
  g_data_input_stream_read_line_async (self->stdout_stream, G_PRIORITY_DEFAULT,
                                       self->cancellable, on_line_read,
                                       g_object_ref (self));
}

static void
on_stderr_line (GObject      *source,
                GAsyncResult *result,
                gpointer      user_data)
{
  g_autoptr (HyChatSession) self = user_data;
  g_autofree char *line = NULL;

  line = g_data_input_stream_read_line_finish_utf8 (G_DATA_INPUT_STREAM (source),
                                                    result, NULL, NULL);
  if (line == NULL)
    return;

  if (self->stderr_text->len < STDERR_LIMIT)
    g_string_append_printf (self->stderr_text, "%s\n", line);

  g_data_input_stream_read_line_async (self->stderr_stream, G_PRIORITY_LOW,
                                       self->cancellable, on_stderr_line,
                                       g_object_ref (self));
}

/* --- lifecycle ------------------------------------------------------------ */

HyChatSession *
hy_chat_session_new (const AiBackend *backend)
{
  HyChatSession *self;

  g_return_val_if_fail (backend != NULL, NULL);

  self = g_object_new (HY_TYPE_CHAT_SESSION, NULL);
  self->backend = backend;
  self->parser = ai_parser_new (backend);

  return self;
}

gboolean
hy_chat_session_start (HyChatSession    *self,
                       const AiRunSpec  *spec,
                       GError          **error)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GPtrArray) argv = NULL;
  g_autoptr (GError) local_error = NULL;

  g_return_val_if_fail (HY_IS_CHAT_SESSION (self), FALSE);
  g_return_val_if_fail (spec != NULL, FALSE);
  g_return_val_if_fail (self->process == NULL, FALSE);

  argv = self->backend->build_argv (self->backend, spec);

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDIN_PIPE |
                                        G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_PIPE);

  /* The working directory is how a folder's project context reaches the CLI:
   * both of them read the repository they are started in. */
  if (spec->workdir != NULL)
    g_subprocess_launcher_set_cwd (launcher, spec->workdir);

  self->process = g_subprocess_launcher_spawnv (launcher,
                                                (const char * const *) argv->pdata,
                                                &local_error);
  if (self->process == NULL)
    {
      if (g_error_matches (local_error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT))
        g_set_error (error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT,
                     "%s is not in PATH. hy runs the CLI you already have "
                     "installed and signed in.", self->backend->program);
      else
        g_propagate_error (error, g_steal_pointer (&local_error));

      return FALSE;
    }

  /* codex reads stdin when it is a pipe and appends it to the prompt, so it
   * has to see end-of-file straight away. */
  g_output_stream_close (g_subprocess_get_stdin_pipe (self->process), NULL, NULL);

  self->stdout_stream =
    g_data_input_stream_new (g_subprocess_get_stdout_pipe (self->process));
  self->stderr_stream =
    g_data_input_stream_new (g_subprocess_get_stderr_pipe (self->process));

  /* Agent turns can be long, and a single event can be large. */
  g_data_input_stream_set_newline_type (self->stdout_stream,
                                        G_DATA_STREAM_NEWLINE_TYPE_LF);
  g_buffered_input_stream_set_buffer_size (G_BUFFERED_INPUT_STREAM (self->stdout_stream),
                                           1 << 20);

  read_next_line (self);

  g_data_input_stream_read_line_async (self->stderr_stream, G_PRIORITY_LOW,
                                       self->cancellable, on_stderr_line,
                                       g_object_ref (self));

  return TRUE;
}

static gboolean
on_grace_elapsed (gpointer user_data)
{
  HyChatSession *self = user_data;

  self->kill_timeout_id = 0;

  if (self->process != NULL && !self->finished)
    {
      g_debug ("%s ignored SIGINT; killing it", self->backend->id);
      g_subprocess_force_exit (self->process);
    }

  return G_SOURCE_REMOVE;
}

void
hy_chat_session_cancel (HyChatSession *self)
{
  g_return_if_fail (HY_IS_CHAT_SESSION (self));

  if (self->process == NULL || self->finished || self->stopping)
    return;

  self->stopping = TRUE;

  /* SIGINT first: both CLIs treat it as "wrap up", which leaves their own
   * session file intact so the conversation can still be resumed. */
  g_subprocess_send_signal (self->process, SIGINT);

  self->kill_timeout_id = g_timeout_add_seconds (STOP_GRACE_SECONDS,
                                                 on_grace_elapsed, self);
}

gboolean
hy_chat_session_is_running (HyChatSession *self)
{
  g_return_val_if_fail (HY_IS_CHAT_SESSION (self), FALSE);

  return self->process != NULL && !self->finished;
}

static void
hy_chat_session_dispose (GObject *object)
{
  HyChatSession *self = HY_CHAT_SESSION (object);

  g_clear_handle_id (&self->kill_timeout_id, g_source_remove);

  if (self->process != NULL && !self->finished)
    g_subprocess_force_exit (self->process);

  g_cancellable_cancel (self->cancellable);

  g_clear_object (&self->stdout_stream);
  g_clear_object (&self->stderr_stream);
  g_clear_object (&self->process);
  g_clear_object (&self->cancellable);

  G_OBJECT_CLASS (hy_chat_session_parent_class)->dispose (object);
}

static void
hy_chat_session_finalize (GObject *object)
{
  HyChatSession *self = HY_CHAT_SESSION (object);

  g_clear_pointer (&self->parser, ai_parser_free);
  g_string_free (self->stderr_text, TRUE);

  G_OBJECT_CLASS (hy_chat_session_parent_class)->finalize (object);
}

static void
hy_chat_session_class_init (HyChatSessionClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = hy_chat_session_dispose;
  object_class->finalize = hy_chat_session_finalize;

  signals[SIGNAL_SESSION_STARTED] =
    g_signal_new ("session-started", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_TEXT_DELTA] =
    g_signal_new ("text-delta", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_TOOL_USE] =
    g_signal_new ("tool-use", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_FINISHED] =
    g_signal_new ("finished", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 2,
                  G_TYPE_BOOLEAN, G_TYPE_STRING);
}

static void
hy_chat_session_init (HyChatSession *self)
{
  self->cancellable = g_cancellable_new ();
  self->stderr_text = g_string_new (NULL);
}
