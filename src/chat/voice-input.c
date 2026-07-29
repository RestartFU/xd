#include "voice-input.h"

#include "util/voice-data.h"

#include <pulse/error.h>
#include <pulse/simple.h>
#include <libsoup/soup.h>

#define VOICE_SAMPLE_RATE       16000
#define VOICE_CHANNELS          1
#define VOICE_CHUNK_MSEC        100
#define VOICE_MAX_SECONDS       120
#define VOICE_MIN_BYTES         (VOICE_SAMPLE_RATE * 2 / 4)
#define TRANSCRIPTION_URL       "https://api.openai.com/v1/audio/transcriptions"
#define TRANSCRIPTION_MODEL     "gpt-transcribe"

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

typedef struct
{
  SoupSession *session;
  SoupMessage *message;
} TranscriptionRequest;

static void
transcription_request_free (TranscriptionRequest *request)
{
  g_clear_object (&request->session);
  g_clear_object (&request->message);
  g_free (request);
}

static void
on_transcription_response (GObject      *source,
                           GAsyncResult *result,
                           gpointer      user_data)
{
  GTask *task = user_data;
  TranscriptionRequest *request = g_task_get_task_data (task);
  g_autoptr (GError) error = NULL;
  g_autoptr (GBytes) body =
    soup_session_send_and_read_finish (
      SOUP_SESSION (source), result, &error);
  g_autofree char *text = NULL;
  gsize length = 0;
  const guint8 *data;

  if (body == NULL)
    {
      g_task_return_error (task, g_steal_pointer (&error));
      g_object_unref (task);
      return;
    }

  data = g_bytes_get_data (body, &length);
  text = xd_voice_transcript_parse (data, length, &error);
  if (text == NULL)
    {
      if (error == NULL)
        g_set_error (
          &error, G_IO_ERROR, G_IO_ERROR_FAILED,
          "Transcription failed with HTTP status %u.",
          soup_message_get_status (request->message));
      g_task_return_error (task, g_steal_pointer (&error));
    }
  else if (!SOUP_STATUS_IS_SUCCESSFUL (
             soup_message_get_status (request->message)))
    {
      g_task_return_new_error (
        task, G_IO_ERROR, G_IO_ERROR_FAILED,
        "Transcription failed with HTTP status %u.",
        soup_message_get_status (request->message));
    }
  else
    {
      g_task_return_pointer (task, g_steal_pointer (&text), g_free);
    }

  g_object_unref (task);
}

void
xd_voice_transcribe_async (GBytes             *wav,
                           const char         *api_key,
                           GCancellable       *cancellable,
                           GAsyncReadyCallback callback,
                           gpointer            user_data)
{
  static const char context[] =
    "A software engineering instruction. Preserve code symbols, command "
    "names, file paths, library names, acronyms, capitalization, and "
    "punctuation exactly when spoken.";
  g_autoptr (GTask) task = NULL;
  g_autofree char *authorization = NULL;
  SoupMultipart *multipart;
  TranscriptionRequest *request;

  g_return_if_fail (wav != NULL);
  g_return_if_fail (api_key != NULL && *api_key != '\0');

  task = g_task_new (NULL, cancellable, callback, user_data);
  g_task_set_source_tag (task, xd_voice_transcribe_async);

  multipart = soup_multipart_new (SOUP_FORM_MIME_TYPE_MULTIPART);
  soup_multipart_append_form_string (multipart, "model", TRANSCRIPTION_MODEL);
  soup_multipart_append_form_string (multipart, "prompt", context);
  soup_multipart_append_form_file (
    multipart, "file", "voice-prompt.wav", "audio/wav", wav);

  request = g_new0 (TranscriptionRequest, 1);
  request->session = soup_session_new_with_options (
    "timeout", 60, "user-agent", "xd/" XD_VERSION, NULL);
  request->message =
    soup_message_new_from_multipart (TRANSCRIPTION_URL, multipart);
  soup_multipart_free (multipart);

  if (request->message == NULL)
    {
      transcription_request_free (request);
      g_task_return_new_error (task, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                               "Cannot create transcription request.");
      return;
    }

  authorization = g_strdup_printf ("Bearer %s", api_key);
  soup_message_headers_replace (
    soup_message_get_request_headers (request->message),
    "Authorization", authorization);

  g_task_set_task_data (
    task, request, (GDestroyNotify) transcription_request_free);
  soup_session_send_and_read_async (
    request->session, request->message, G_PRIORITY_DEFAULT, cancellable,
    on_transcription_response, g_object_ref (task));
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
