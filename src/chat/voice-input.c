#include "voice-input.h"

#include "util/app-paths.h"
#include "util/voice-data.h"

#include <dlfcn.h>
#include <errno.h>
#include <ggml-backend.h>
#include <glib/gstdio.h>
#include <pulse/error.h>
#include <pulse/simple.h>
#include <libsoup/soup.h>
#include <stdbool.h>
#include <sys/stat.h>
#include <whisper.h>

#define VOICE_SAMPLE_RATE       16000
#define VOICE_CHANNELS          1
#define VOICE_CHUNK_MSEC        100
#define VOICE_MAX_SECONDS       120
#define VOICE_MIN_BYTES         (VOICE_SAMPLE_RATE * 2 / 4)
#define VOICE_MODEL_FILE        "ggml-large-v3-turbo-q5_0.bin"
#define VOICE_MODEL_SIZE        G_GUINT64_CONSTANT (574041195)
#define VOICE_MODEL_SHA256 \
  "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"
#define VOICE_MODEL_URL \
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/" \
  "98aa99a0a9db05ae2342309f5096248665f7cba3/" VOICE_MODEL_FILE

struct _XdVoiceRecorder
{
  GObject parent_instance;
  gint stop_requested;
};

G_DEFINE_FINAL_TYPE (XdVoiceRecorder, xd_voice_recorder, G_TYPE_OBJECT)

static void
xd_voice_recorder_class_init (XdVoiceRecorderClass *klass)
{
}

static void
xd_voice_recorder_init (XdVoiceRecorder *self)
{
}

XdVoiceRecorder *
xd_voice_recorder_new (void)
{
  return g_object_new (XD_TYPE_VOICE_RECORDER, NULL);
}

static void
record_thread (GTask        *task,
               gpointer      source_object,
               gpointer      task_data,
               GCancellable *cancellable)
{
  XdVoiceRecorder *self = source_object;
  const pa_sample_spec samples = {
    .format = PA_SAMPLE_S16LE,
    .rate = VOICE_SAMPLE_RATE,
    .channels = VOICE_CHANNELS,
  };
  const gsize chunk_size =
    VOICE_SAMPLE_RATE * VOICE_CHANNELS * 2 * VOICE_CHUNK_MSEC / 1000;
  const gsize max_size =
    VOICE_SAMPLE_RATE * VOICE_CHANNELS * 2 * VOICE_MAX_SECONDS;
  g_autoptr (GByteArray) pcm = g_byte_array_sized_new (VOICE_SAMPLE_RATE * 2);
  g_autofree guint8 *chunk = g_malloc (chunk_size);
  pa_simple *stream;
  int pulse_error = 0;

  if (g_task_return_error_if_cancelled (task))
    return;

  stream = pa_simple_new (
    NULL, XD_APP_NAME, PA_STREAM_RECORD, NULL, "Voice prompt",
    &samples, NULL, NULL, &pulse_error);
  if (stream == NULL)
    {
      g_task_return_new_error (
        task, G_IO_ERROR, G_IO_ERROR_FAILED,
        "Cannot open microphone: %s", pa_strerror (pulse_error));
      return;
    }

  while (!g_atomic_int_get (&self->stop_requested) &&
         !g_cancellable_is_cancelled (cancellable) &&
         pcm->len < max_size)
    {
      if (pa_simple_read (stream, chunk, chunk_size, &pulse_error) < 0)
        {
          pa_simple_free (stream);
          g_task_return_new_error (
            task, G_IO_ERROR, G_IO_ERROR_FAILED,
            "Cannot record microphone: %s", pa_strerror (pulse_error));
          return;
        }

      g_byte_array_append (pcm, chunk, chunk_size);
    }

  pa_simple_free (stream);
  if (g_task_return_error_if_cancelled (task))
    return;

  if (pcm->len < VOICE_MIN_BYTES)
    {
      g_task_return_new_error (task, G_IO_ERROR, G_IO_ERROR_FAILED,
                               "Recording was too short.");
      return;
    }

  g_task_return_pointer (
    task,
    xd_voice_wav_from_s16 (
      pcm->data, pcm->len, VOICE_SAMPLE_RATE, VOICE_CHANNELS),
    (GDestroyNotify) g_bytes_unref);
}

void
xd_voice_recorder_record_async (XdVoiceRecorder    *self,
                                GCancellable       *cancellable,
                                GAsyncReadyCallback callback,
                                gpointer            user_data)
{
  g_autoptr (GTask) task = NULL;

  g_return_if_fail (XD_IS_VOICE_RECORDER (self));

  g_atomic_int_set (&self->stop_requested, FALSE);
  task = g_task_new (self, cancellable, callback, user_data);
  g_task_set_source_tag (task, xd_voice_recorder_record_async);
  g_task_run_in_thread (task, record_thread);
}

GBytes *
xd_voice_recorder_record_finish (XdVoiceRecorder *self,
                                 GAsyncResult    *result,
                                 GError         **error)
{
  g_return_val_if_fail (
    g_task_is_valid (result, self) &&
    g_async_result_is_tagged (result, xd_voice_recorder_record_async),
    NULL);

  return g_task_propagate_pointer (G_TASK (result), error);
}

void
xd_voice_recorder_stop (XdVoiceRecorder *self)
{
  g_return_if_fail (XD_IS_VOICE_RECORDER (self));

  g_atomic_int_set (&self->stop_requested, TRUE);
}

struct _XdVoiceModel
{
  GObject parent_instance;
  gint progress;
};

G_DEFINE_FINAL_TYPE (XdVoiceModel, xd_voice_model, G_TYPE_OBJECT)

static void
xd_voice_model_class_init (XdVoiceModelClass *klass)
{
}

static void
xd_voice_model_init (XdVoiceModel *self)
{
}

XdVoiceModel *
xd_voice_model_new (void)
{
  return g_object_new (XD_TYPE_VOICE_MODEL, NULL);
}

static char *
voice_model_path (void)
{
  return g_build_filename (
    xd_app_data_dir (), "speech", VOICE_MODEL_FILE, NULL);
}

static char *
voice_model_marker_path (void)
{
  g_autofree char *path = voice_model_path ();

  return g_strconcat (path, ".sha256", NULL);
}

static gboolean
voice_model_is_verified (const char *path)
{
  g_autofree char *marker_path = voice_model_marker_path ();
  g_autofree char *marker = NULL;
  GStatBuf stat_buffer;

  if (g_stat (path, &stat_buffer) != 0 ||
      !S_ISREG (stat_buffer.st_mode) ||
      (guint64) stat_buffer.st_size != VOICE_MODEL_SIZE ||
      !g_file_get_contents (marker_path, &marker, NULL, NULL))
    return FALSE;

  g_strstrip (marker);
  return xd_voice_model_metadata_valid (
    (guint64) stat_buffer.st_size, marker);
}

char *
xd_voice_model_find (void)
{
  const char *override = g_getenv ("XD_VOICE_MODEL_PATH");
  g_autofree char *path = NULL;

  if (override != NULL && *override != '\0')
    return g_file_test (override, G_FILE_TEST_IS_REGULAR)
           ? g_strdup (override)
           : NULL;

  path = voice_model_path ();
  return voice_model_is_verified (path)
         ? g_steal_pointer (&path)
         : NULL;
}

guint
xd_voice_model_get_progress (XdVoiceModel *self)
{
  g_return_val_if_fail (XD_IS_VOICE_MODEL (self), 0);

  return (guint) g_atomic_int_get (&self->progress);
}

static GFileOutputStream *
open_model_temporary (const char  *model_path,
                      GFile      **temporary,
                      GError     **error)
{
  for (guint attempt = 0; attempt < 8; attempt++)
    {
      g_autofree char *path = g_strdup_printf (
        "%s.download-%08x", model_path, g_random_int ());
      g_autoptr (GFile) file = g_file_new_for_path (path);
      GFileOutputStream *output =
        g_file_create (file, G_FILE_CREATE_PRIVATE, NULL, error);

      if (output != NULL)
        {
          *temporary = g_steal_pointer (&file);
          return output;
        }

      if (!g_error_matches (*error, G_IO_ERROR, G_IO_ERROR_EXISTS))
        return NULL;

      g_clear_error (error);
    }

  g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_EXISTS,
                       "Cannot create a temporary speech model file.");
  return NULL;
}

static gboolean
download_model (XdVoiceModel  *self,
                GCancellable  *cancellable,
                char         **model_path_out,
                GError       **error)
{
  g_autofree char *model_path = voice_model_path ();
  g_autofree char *model_dir = g_path_get_dirname (model_path);
  g_autofree char *marker_path = voice_model_marker_path ();
  g_autoptr (SoupSession) session = NULL;
  g_autoptr (SoupMessage) message = NULL;
  g_autoptr (GInputStream) input = NULL;
  g_autoptr (GFile) temporary = NULL;
  g_autoptr (GFile) destination = NULL;
  g_autoptr (GFileOutputStream) output = NULL;
  g_autoptr (GChecksum) checksum = NULL;
  guint8 buffer[128 * 1024];
  guint64 total = 0;
  gboolean success = FALSE;

  if (voice_model_is_verified (model_path))
    {
      *model_path_out = g_steal_pointer (&model_path);
      g_atomic_int_set (&self->progress, 100);
      return TRUE;
    }

  if (g_mkdir_with_parents (model_dir, 0700) != 0)
    {
      g_set_error (error, G_IO_ERROR, g_io_error_from_errno (errno),
                   "Cannot create speech model directory: %s",
                   g_strerror (errno));
      return FALSE;
    }

  output = open_model_temporary (model_path, &temporary, error);
  if (output == NULL)
    return FALSE;

  session = soup_session_new_with_options (
    "timeout", 0, "idle-timeout", 30,
    "user-agent", "xd/" XD_VERSION, NULL);
  message = soup_message_new ("GET", VOICE_MODEL_URL);
  input = soup_session_send (session, message, cancellable, error);
  if (input == NULL)
    goto out;

  if (!SOUP_STATUS_IS_SUCCESSFUL (soup_message_get_status (message)))
    {
      g_set_error (error, G_IO_ERROR, G_IO_ERROR_FAILED,
                   "Speech model download failed with HTTP status %u.",
                   soup_message_get_status (message));
      goto out;
    }

  checksum = g_checksum_new (G_CHECKSUM_SHA256);
  while (TRUE)
    {
      gssize count =
        g_input_stream_read (input, buffer, sizeof buffer, cancellable, error);

      if (count < 0)
        goto out;
      if (count == 0)
        break;
      if (total + (guint64) count > VOICE_MODEL_SIZE)
        {
          g_set_error_literal (error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                               "Speech model download is larger than expected.");
          goto out;
        }

      if (!g_output_stream_write_all (
            G_OUTPUT_STREAM (output), buffer, (gsize) count, NULL,
            cancellable, error))
        goto out;

      g_checksum_update (checksum, buffer, (gsize) count);
      total += (guint64) count;
      g_atomic_int_set (
        &self->progress, (gint) MIN (99, total * 100 / VOICE_MODEL_SIZE));
    }

  if (!g_output_stream_close (G_OUTPUT_STREAM (output), cancellable, error))
    goto out;

  if (!xd_voice_model_metadata_valid (
        total, g_checksum_get_string (checksum)))
    {
      g_set_error_literal (
        error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
        "Speech model download failed its integrity check.");
      goto out;
    }

  destination = g_file_new_for_path (model_path);
  if (!g_file_move (
        temporary, destination, G_FILE_COPY_OVERWRITE, cancellable,
        NULL, NULL, error))
    goto out;
  g_clear_object (&temporary);

  if (!g_file_set_contents_full (
        marker_path, VOICE_MODEL_SHA256 "\n", -1,
        G_FILE_SET_CONTENTS_CONSISTENT, 0600, error))
    goto out;

  g_atomic_int_set (&self->progress, 100);
  *model_path_out = g_steal_pointer (&model_path);
  success = TRUE;

out:
  if (temporary != NULL)
    g_file_delete (temporary, NULL, NULL);
  return success;
}

static void
ensure_model_thread (GTask        *task,
                     gpointer      source_object,
                     gpointer      task_data,
                     GCancellable *cancellable)
{
  XdVoiceModel *self = source_object;
  g_autoptr (GError) error = NULL;
  g_autofree char *model_path = NULL;

  if (g_task_return_error_if_cancelled (task))
    return;

  if (!download_model (self, cancellable, &model_path, &error))
    {
      g_task_return_error (task, g_steal_pointer (&error));
      return;
    }

  g_task_return_pointer (
    task, g_steal_pointer (&model_path), g_free);
}

void
xd_voice_model_ensure_async (XdVoiceModel      *self,
                             GCancellable       *cancellable,
                             GAsyncReadyCallback callback,
                             gpointer            user_data)
{
  g_autoptr (GTask) task = NULL;

  g_return_if_fail (XD_IS_VOICE_MODEL (self));

  g_atomic_int_set (&self->progress, 0);
  task = g_task_new (self, cancellable, callback, user_data);
  g_task_set_source_tag (task, xd_voice_model_ensure_async);
  g_task_run_in_thread (task, ensure_model_thread);
}

char *
xd_voice_model_ensure_finish (XdVoiceModel  *self,
                              GAsyncResult  *result,
                              GError       **error)
{
  g_return_val_if_fail (
    g_task_is_valid (result, self) &&
    g_async_result_is_tagged (result, xd_voice_model_ensure_async),
    NULL);

  return g_task_propagate_pointer (G_TASK (result), error);
}

typedef struct
{
  GBytes *wav;
  char *model_path;
} Transcription;

static void
transcription_free (Transcription *transcription)
{
  g_bytes_unref (transcription->wav);
  g_free (transcription->model_path);
  g_free (transcription);
}

static bool
abort_transcription (gpointer user_data)
{
  return g_cancellable_is_cancelled (user_data);
}

static void
quiet_whisper_log (enum ggml_log_level level,
                   const char         *message,
                   void               *user_data)
{
}

static void
load_whisper_backends (void)
{
  static gsize loaded = 0;

  if (g_once_init_enter (&loaded))
    {
      Dl_info info = { 0 };

      if (dladdr ((void *) ggml_backend_load_all_from_path, &info) != 0 &&
          info.dli_fname != NULL)
        {
          g_autofree char *directory = g_path_get_dirname (info.dli_fname);

          ggml_backend_load_all_from_path (directory);
        }
      else
        {
          ggml_backend_load_all ();
        }

      g_once_init_leave (&loaded, 1);
    }
}

static void
transcribe_thread (GTask        *task,
                   gpointer      source_object,
                   gpointer      task_data,
                   GCancellable *cancellable)
{
  static gsize logging_configured = 0;
  static const char prompt[] =
    "Software engineering, source code, commands, file paths, APIs, "
    "libraries, acronyms, capitalization, and punctuation.";
  Transcription *transcription = task_data;
  g_autoptr (GError) error = NULL;
  g_autoptr (GBytes) pcm = NULL;
  const float *samples;
  gsize sample_bytes = 0;
  struct whisper_context_params context_params;
  struct whisper_full_params params;
  struct whisper_context *context;
  g_autoptr (GString) text = NULL;
  int segments;

  if (g_once_init_enter (&logging_configured))
    {
      whisper_log_set (quiet_whisper_log, NULL);
      g_once_init_leave (&logging_configured, 1);
    }

  if (g_task_return_error_if_cancelled (task))
    return;

  load_whisper_backends ();
  pcm = xd_voice_wav_to_f32 (transcription->wav, &error);
  if (pcm == NULL)
    {
      g_task_return_error (task, g_steal_pointer (&error));
      return;
    }
  samples = g_bytes_get_data (pcm, &sample_bytes);

  context_params = whisper_context_default_params ();
  context_params.use_gpu = FALSE;
  context_params.flash_attn = TRUE;
  context = whisper_init_from_file_with_params (
    transcription->model_path, context_params);
  if (context == NULL)
    {
      g_task_return_new_error (
        task, G_IO_ERROR, G_IO_ERROR_FAILED,
        "Cannot load the local speech model.");
      return;
    }

  params = whisper_full_default_params (WHISPER_SAMPLING_BEAM_SEARCH);
  params.n_threads = CLAMP (g_get_num_processors (), 1, 8);
  params.translate = FALSE;
  params.no_context = TRUE;
  params.no_timestamps = TRUE;
  params.print_special = FALSE;
  params.print_progress = FALSE;
  params.print_realtime = FALSE;
  params.print_timestamps = FALSE;
  params.initial_prompt = prompt;
  params.language = "auto";
  params.abort_callback = abort_transcription;
  params.abort_callback_user_data = cancellable;

  if (whisper_full (
        context, params, samples, (int) (sample_bytes / sizeof *samples)) != 0)
    {
      whisper_free (context);
      if (g_cancellable_is_cancelled (cancellable))
        g_task_return_new_error (
          task, G_IO_ERROR, G_IO_ERROR_CANCELLED,
          "Voice transcription was cancelled.");
      else
        g_task_return_new_error (
          task, G_IO_ERROR, G_IO_ERROR_FAILED,
          "Local voice transcription failed.");
      return;
    }

  text = g_string_new (NULL);
  segments = whisper_full_n_segments (context);
  for (int i = 0; i < segments; i++)
    g_string_append (text, whisper_full_get_segment_text (context, i));
  whisper_free (context);

  g_strstrip (text->str);
  if (*text->str == '\0')
    {
      g_task_return_new_error (
        task, G_IO_ERROR, G_IO_ERROR_FAILED,
        "No speech was detected.");
      return;
    }

  g_task_return_pointer (
    task, g_string_free (g_steal_pointer (&text), FALSE), g_free);
}

void
xd_voice_transcribe_async (GBytes             *wav,
                           const char         *model_path,
                           GCancellable       *cancellable,
                           GAsyncReadyCallback callback,
                           gpointer            user_data)
{
  g_autoptr (GTask) task = NULL;
  Transcription *transcription;

  g_return_if_fail (wav != NULL);
  g_return_if_fail (model_path != NULL && *model_path != '\0');

  task = g_task_new (NULL, cancellable, callback, user_data);
  g_task_set_source_tag (task, xd_voice_transcribe_async);
  transcription = g_new0 (Transcription, 1);
  transcription->wav = g_bytes_ref (wav);
  transcription->model_path = g_strdup (model_path);
  g_task_set_task_data (
    task, transcription, (GDestroyNotify) transcription_free);
  g_task_run_in_thread (task, transcribe_thread);
}

char *
xd_voice_transcribe_finish (GAsyncResult *result,
                            GError      **error)
{
  g_return_val_if_fail (
    g_task_is_valid (result, NULL) &&
    g_async_result_is_tagged (result, xd_voice_transcribe_async),
    NULL);

  return g_task_propagate_pointer (G_TASK (result), error);
}
