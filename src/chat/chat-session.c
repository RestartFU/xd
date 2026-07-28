#include "chat-session.h"

#include "settings/agent-secrets.h"
#include "util/host-launch.h"

#include <string.h>

#ifndef G_OS_WIN32
#include <signal.h>
#endif

/* How long a polite SIGINT gets before the process is killed outright. */
#define STOP_GRACE_SECONDS 2

/* Enough stderr to explain a failure without holding a runaway log in memory. */
#define STDERR_LIMIT 8192

struct _XdChatSession
{
  GObject parent_instance;

  const AiBackend *backend;
  AiParser *parser;

  GSubprocess *process;
  GDataInputStream *stdout_stream;
  GDataInputStream *stderr_stream;
  GOutputStream *stdin_stream;    /* held open only by a streaming backend */
  GCancellable *cancellable;
  GString *stderr_text;

  /*
   * What the running process was launched with.
   *
   * Model, effort and access are argv, so they are fixed for as long as the
   * process lives. A turn that wants different ones cannot be handed to it and
   * needs a new one -- which is why these are kept rather than the process
   * simply being reused for anything.
   */
  gboolean streaming;
  char *session_id;               /* last one the backend reported */
  char *launched_model;
  char *launched_system_prompt;
  char *launched_workdir;
  AiEffort launched_effort;
  AiAccess launched_access;

  guint kill_timeout_id;
  gboolean stopping;
  gboolean turn_complete;         /* the result line landed; finish after it */
  gboolean finished;              /* of the current turn, not of the process */
};

enum
{
  SIGNAL_SESSION_STARTED,
  SIGNAL_COMMANDS,
  SIGNAL_TEXT_DELTA,
  SIGNAL_TOOL_USE,
  SIGNAL_USAGE,
  SIGNAL_FINISHED,
  N_SIGNALS,
};

static guint signals[N_SIGNALS];

G_DEFINE_FINAL_TYPE (XdChatSession, xd_chat_session, G_TYPE_OBJECT)

static void read_next_line (XdChatSession *self);

/* --- finishing ------------------------------------------------------------ */

static void
finish (XdChatSession *self,
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
stderr_tail (XdChatSession *self)
{
  g_strstrip (self->stderr_text->str);

  return *self->stderr_text->str != '\0' ? self->stderr_text->str : NULL;
}

static void
on_process_waited (GObject      *source,
                   GAsyncResult *result,
                   gpointer      user_data)
{
  g_autoptr (XdChatSession) self = user_data;
  g_autoptr (GError) error = NULL;

  /*
   * A streaming process is not supposed to be gone. If a turn was still in
   * flight it is now unanswerable and has to be reported; if none was, the
   * chat simply has no process any more and the next turn starts one.
   */
  if (self->streaming)
    self->stdin_stream = NULL;

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
reap_process (XdChatSession *self)
{
  g_subprocess_wait_check_async (self->process, NULL, on_process_waited,
                                 g_object_ref (self));
}

/* --- reading -------------------------------------------------------------- */

static void
on_event (const AiEvent *event,
          gpointer       user_data)
{
  XdChatSession *self = user_data;

  switch (event->type)
    {
    case AI_EVENT_SESSION_STARTED:
      g_free (self->session_id);
      self->session_id = g_strdup (event->session_id);
      g_signal_emit (self, signals[SIGNAL_SESSION_STARTED], 0, event->session_id);
      break;

    case AI_EVENT_COMMANDS:
      if (event->commands != NULL)
        g_signal_emit (self, signals[SIGNAL_COMMANDS], 0, event->commands);
      break;

    case AI_EVENT_TEXT_DELTA:
      if (event->text != NULL)
        g_signal_emit (self, signals[SIGNAL_TEXT_DELTA], 0, event->text);
      break;

    case AI_EVENT_TOOL_USE:
      g_signal_emit (self, signals[SIGNAL_TOOL_USE], 0,
                     event->text != NULL ? event->text : "tool");
      break;

    case AI_EVENT_USAGE:
      g_signal_emit (self, signals[SIGNAL_USAGE], 0,
                     event->context_used, event->context_window);
      break;

    case AI_EVENT_RESULT:
      /* The backend reported the id only at the end; keep it either way. */
      if (event->session_id != NULL)
        {
          g_free (self->session_id);
          self->session_id = g_strdup (event->session_id);
          g_signal_emit (self, signals[SIGNAL_SESSION_STARTED], 0, event->session_id);
        }

      /*
       * For a process that exits when the turn does, the exit is the end and
       * saying so here would be early -- stderr may still explain a failure.
       * A streaming process outlives its turn, so this line is the only thing
       * that says the turn is over.
       *
       * Noted rather than acted on: one line can carry several events, and
       * whoever handles "finished" is entitled to drop the turn, which would
       * leave the rest of the line being parsed into freed memory.
       */
      if (self->streaming)
        self->turn_complete = TRUE;
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
  g_autoptr (XdChatSession) self = user_data;
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

  /* Reading continues first: the process is still there between turns, and
   * whoever handles "finished" may start the next one straight away. */
  read_next_line (self);

  if (self->turn_complete)
    {
      self->turn_complete = FALSE;
      finish (self, TRUE, NULL);
    }
}

static void
read_next_line (XdChatSession *self)
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
  g_autoptr (XdChatSession) self = user_data;
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

/*
 * Hands one turn to a process that reads them from stdin.
 *
 * Written synchronously: it is a single short line into a pipe with a whole
 * turn about to follow it, and an async write here would only add a way for
 * two turns to interleave on the same descriptor.
 */
static gboolean
write_turn (XdChatSession    *self,
            const AiRunSpec  *spec,
            GError          **error)
{
  g_autofree char *line = self->backend->encode_turn (self->backend, spec);
  g_autofree char *framed = NULL;

  if (line == NULL)
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                           "The turn could not be encoded.");
      return FALSE;
    }

  framed = g_strconcat (line, "\n", NULL);

  if (!g_output_stream_write_all (self->stdin_stream, framed, strlen (framed),
                                  NULL, NULL, error))
    return FALSE;

  return g_output_stream_flush (self->stdin_stream, NULL, error);
}

XdChatSession *
xd_chat_session_new (const AiBackend *backend)
{
  XdChatSession *self;

  g_return_val_if_fail (backend != NULL, NULL);

  self = g_object_new (XD_TYPE_CHAT_SESSION, NULL);
  self->backend = backend;
  self->parser = ai_parser_new (backend);

  return self;
}

gboolean
xd_chat_session_start (XdChatSession    *self,
                       const AiRunSpec  *spec,
                       GError          **error)
{
  g_autoptr (GSubprocessLauncher) launcher = NULL;
  g_autoptr (GPtrArray) argv = NULL;
  g_autoptr (GError) local_error = NULL;
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_auto (GStrv) environment = NULL;
  g_autofree char *secret_prompt = NULL;
  g_autofree char *system_prompt = NULL;
  AiRunSpec effective;

  g_return_val_if_fail (XD_IS_CHAT_SESSION (self), FALSE);
  g_return_val_if_fail (spec != NULL, FALSE);
  g_return_val_if_fail (self->process == NULL, FALSE);

  self->streaming = self->backend->encode_turn != NULL;

  secrets = xd_agent_secrets_load (NULL, &local_error);
  if (secrets == NULL)
    {
      g_prefix_error (&local_error, "Cannot load agent secrets: ");
      g_propagate_error (error, g_steal_pointer (&local_error));
      return FALSE;
    }

  /*
   * Models learn which environment variables exist, never their values. The
   * CLI process receives values below, so commands it launches inherit them
   * without a credential crossing the prompt or transcript.
   */
  effective = *spec;
  secret_prompt = xd_agent_secrets_prompt (secrets);
  if (secret_prompt != NULL)
    {
      system_prompt = spec->system_prompt != NULL
        ? g_strdup_printf ("%s\n\n%s", spec->system_prompt, secret_prompt)
        : g_strdup (secret_prompt);
      effective.system_prompt = system_prompt;
    }

  ai_parser_set_model (self->parser, effective.model);
  argv = self->backend->build_argv (self->backend, &effective);

  launcher = g_subprocess_launcher_new (G_SUBPROCESS_FLAGS_STDIN_PIPE |
                                        G_SUBPROCESS_FLAGS_STDOUT_PIPE |
                                        G_SUBPROCESS_FLAGS_STDERR_PIPE);
  environment = xd_host_environ ();
  environment = xd_agent_secrets_apply_environment (secrets, environment);
  g_subprocess_launcher_set_environ (launcher, environment);

  /* The working directory is how a folder's project context reaches the CLI:
   * both of them read the repository they are started in. */
  if (effective.workdir != NULL)
    g_subprocess_launcher_set_cwd (launcher, effective.workdir);

  self->process = g_subprocess_launcher_spawnv (launcher,
                                                (const char * const *) argv->pdata,
                                                &local_error);
  if (self->process == NULL)
    {
      if (g_error_matches (local_error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT))
        g_set_error (error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT,
                     "%s is not in PATH. xd runs the CLI you already have "
                     "installed and signed in.", self->backend->program);
      else
        g_propagate_error (error, g_steal_pointer (&local_error));

      return FALSE;
    }

  if (self->streaming)
    {
      self->stdin_stream = g_subprocess_get_stdin_pipe (self->process);

      self->launched_model = g_strdup (effective.model);
      self->launched_system_prompt = g_strdup (effective.system_prompt);
      self->launched_workdir = g_strdup (effective.workdir);
      self->launched_effort = effective.effort;
      self->launched_access = effective.access;
    }
  else
    {
      /* codex reads stdin when it is a pipe and appends it to the prompt, so
       * it has to see end-of-file straight away. */
      g_output_stream_close (g_subprocess_get_stdin_pipe (self->process),
                             NULL, NULL);
    }

  self->stdout_stream =
    g_data_input_stream_new (g_subprocess_get_stdout_pipe (self->process));
  self->stderr_stream =
    g_data_input_stream_new (g_subprocess_get_stderr_pipe (self->process));

  /* Agent turns can be long, and a single event can be large. */
  g_data_input_stream_set_newline_type (self->stdout_stream,
                                        G_DATA_STREAM_NEWLINE_TYPE_LF);
  g_buffered_input_stream_set_buffer_size (G_BUFFERED_INPUT_STREAM (self->stdout_stream),
                                           1 << 20);

  /*
   * The first turn goes out before anything is listening for the answer.
   * Failing here means the process was never given the prompt, and returning
   * with reads already queued would leave them to land on a caller that has
   * been told the turn never started.
   */
  if (self->streaming && !write_turn (self, &effective, error))
    {
      g_subprocess_force_exit (self->process);
      self->stdin_stream = NULL;
      g_clear_object (&self->stdout_stream);
      g_clear_object (&self->stderr_stream);
      g_clear_object (&self->process);
      return FALSE;
    }

  read_next_line (self);

  g_data_input_stream_read_line_async (self->stderr_stream, G_PRIORITY_LOW,
                                       self->cancellable, on_stderr_line,
                                       g_object_ref (self));

  return TRUE;
}

/*
 * Whether a turn can be handed to the process that is already running.
 *
 * Everything compared here is argv, decided when the process started and not
 * changeable afterwards. Someone who switches model mid-chat gets a new
 * process, which is what they would have got before any of this.
 */
static gboolean
matches_launch (XdChatSession   *self,
                const AiRunSpec *spec)
{
  return g_strcmp0 (self->launched_model, spec->model) == 0 &&
         g_strcmp0 (self->launched_workdir, spec->workdir) == 0 &&
         self->launched_effort == spec->effort &&
         self->launched_access == spec->access;
}

gboolean
xd_chat_session_can_continue (XdChatSession   *self,
                              const AiRunSpec *spec)
{
  g_return_val_if_fail (XD_IS_CHAT_SESSION (self), FALSE);
  g_return_val_if_fail (spec != NULL, FALSE);

  return self->streaming && self->process != NULL &&
         self->stdin_stream != NULL && !self->stopping &&
         self->finished && matches_launch (self, spec);
}

gboolean
xd_chat_session_continue (XdChatSession    *self,
                          const AiRunSpec  *spec,
                          GError          **error)
{
  g_autoptr (XdAgentSecrets) secrets = NULL;
  g_autofree char *secret_prompt = NULL;
  g_autofree char *system_prompt = NULL;
  AiRunSpec effective;

  g_return_val_if_fail (XD_IS_CHAT_SESSION (self), FALSE);
  g_return_val_if_fail (spec != NULL, FALSE);

  if (!xd_chat_session_can_continue (self, spec))
    {
      g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                           "This turn needs a process of its own.");
      return FALSE;
    }

  /* The secrets prompt was appended to the system prompt in argv and is
   * already in effect; only the turn's own text is new. */
  effective = *spec;
  secrets = xd_agent_secrets_load (NULL, NULL);
  if (secrets != NULL)
    secret_prompt = xd_agent_secrets_prompt (secrets);
  if (secret_prompt != NULL)
    {
      system_prompt = g_strdup (self->launched_system_prompt);
      effective.system_prompt = system_prompt;
    }

  self->finished = FALSE;
  self->stopping = FALSE;
  g_string_truncate (self->stderr_text, 0);

  /* The parser remembers what it has already handed out, so that a reply
   * streamed as deltas and then repeated whole is not shown twice. That memory
   * is about one turn; carrying it into the next would swallow the new one. */
  ai_parser_free (self->parser);
  self->parser = ai_parser_new (self->backend);
  ai_parser_set_model (self->parser, effective.model);

  return write_turn (self, &effective, error);
}

static gboolean
on_grace_elapsed (gpointer user_data)
{
  XdChatSession *self = user_data;

  self->kill_timeout_id = 0;

  if (self->process != NULL && !self->finished)
    {
      g_debug ("%s ignored SIGINT; killing it", self->backend->id);
      g_subprocess_force_exit (self->process);
    }

  return G_SOURCE_REMOVE;
}

void
xd_chat_session_cancel (XdChatSession *self)
{
  g_return_if_fail (XD_IS_CHAT_SESSION (self));

  if (self->process == NULL || self->finished || self->stopping)
    return;

  self->stopping = TRUE;

#ifdef G_OS_WIN32
  /* GSubprocess has no signal delivery on Windows. Force-exit is the only
   * portable cancellation primitive until a console-control bridge exists. */
  g_subprocess_force_exit (self->process);
#else
  /* SIGINT first: both CLIs treat it as "wrap up", which leaves their own
   * session file intact so the conversation can still be resumed. */
  g_subprocess_send_signal (self->process, SIGINT);

  self->kill_timeout_id = g_timeout_add_seconds (STOP_GRACE_SECONDS,
                                                 on_grace_elapsed, self);
#endif
}

gboolean
xd_chat_session_is_running (XdChatSession *self)
{
  g_return_val_if_fail (XD_IS_CHAT_SESSION (self), FALSE);

  return self->process != NULL && !self->finished;
}

static void
xd_chat_session_dispose (GObject *object)
{
  XdChatSession *self = XD_CHAT_SESSION (object);

  g_clear_handle_id (&self->kill_timeout_id, g_source_remove);

  /*
   * A streaming process is idle between turns rather than gone, so "the turn
   * finished" no longer means there is nothing to stop. Letting go of the
   * session is what ends it, and not doing this left one behind per chat.
   */
  if (self->process != NULL && (self->streaming || !self->finished))
    g_subprocess_force_exit (self->process);

  self->stdin_stream = NULL;
  g_cancellable_cancel (self->cancellable);

  g_clear_object (&self->stdout_stream);
  g_clear_object (&self->stderr_stream);
  g_clear_object (&self->process);
  g_clear_object (&self->cancellable);

  G_OBJECT_CLASS (xd_chat_session_parent_class)->dispose (object);
}

static void
xd_chat_session_finalize (GObject *object)
{
  XdChatSession *self = XD_CHAT_SESSION (object);

  g_clear_pointer (&self->parser, ai_parser_free);
  g_clear_pointer (&self->session_id, g_free);
  g_clear_pointer (&self->launched_model, g_free);
  g_clear_pointer (&self->launched_system_prompt, g_free);
  g_clear_pointer (&self->launched_workdir, g_free);
  g_string_free (self->stderr_text, TRUE);

  G_OBJECT_CLASS (xd_chat_session_parent_class)->finalize (object);
}

static void
xd_chat_session_class_init (XdChatSessionClass *klass)
{
  GObjectClass *object_class = G_OBJECT_CLASS (klass);

  object_class->dispose = xd_chat_session_dispose;
  object_class->finalize = xd_chat_session_finalize;

  signals[SIGNAL_SESSION_STARTED] =
    g_signal_new ("session-started", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_COMMANDS] =
    g_signal_new ("commands", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRV);

  signals[SIGNAL_TEXT_DELTA] =
    g_signal_new ("text-delta", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_TOOL_USE] =
    g_signal_new ("tool-use", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_STRING);

  signals[SIGNAL_USAGE] =
    g_signal_new ("usage", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 2,
                  G_TYPE_UINT64, G_TYPE_UINT64);

  signals[SIGNAL_FINISHED] =
    g_signal_new ("finished", G_TYPE_FROM_CLASS (klass), G_SIGNAL_RUN_LAST,
                  0, NULL, NULL, NULL, G_TYPE_NONE, 2,
                  G_TYPE_BOOLEAN, G_TYPE_STRING);
}

static void
xd_chat_session_init (XdChatSession *self)
{
  self->cancellable = g_cancellable_new ();
  self->stderr_text = g_string_new (NULL);
}
