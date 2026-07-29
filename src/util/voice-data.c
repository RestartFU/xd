#include "voice-data.h"

#include <json-glib/json-glib.h>

G_DEFINE_QUARK (xd-voice-data-error, xd_voice_data_error)

static void
append_u16_le (GByteArray *bytes,
               guint16     value)
{
  guint16 encoded = GUINT16_TO_LE (value);

  g_byte_array_append (bytes, (const guint8 *) &encoded, sizeof encoded);
}

static void
append_u32_le (GByteArray *bytes,
               guint32     value)
{
  guint32 encoded = GUINT32_TO_LE (value);

  g_byte_array_append (bytes, (const guint8 *) &encoded, sizeof encoded);
}

GBytes *
xd_voice_wav_from_s16 (const guint8 *pcm,
                       gsize         length,
                       guint         sample_rate,
                       guint         channels)
{
  g_autoptr (GByteArray) bytes = NULL;
  guint32 data_length;
  guint32 byte_rate;
  guint16 block_align;

  g_return_val_if_fail (pcm != NULL || length == 0, NULL);
  g_return_val_if_fail (length <= G_MAXUINT32 - 36, NULL);
  g_return_val_if_fail (sample_rate > 0, NULL);
  g_return_val_if_fail (channels > 0 && channels <= G_MAXUINT16, NULL);
  g_return_val_if_fail (sample_rate <= G_MAXUINT32 / (channels * 2), NULL);

  data_length = (guint32) length;
  byte_rate = sample_rate * channels * 2;
  block_align = (guint16) channels * 2;
  bytes = g_byte_array_sized_new (44 + length);

  g_byte_array_append (bytes, (const guint8 *) "RIFF", 4);
  append_u32_le (bytes, 36 + data_length);
  g_byte_array_append (bytes, (const guint8 *) "WAVEfmt ", 8);
  append_u32_le (bytes, 16);
  append_u16_le (bytes, 1);
  append_u16_le (bytes, (guint16) channels);
  append_u32_le (bytes, sample_rate);
  append_u32_le (bytes, byte_rate);
  append_u16_le (bytes, block_align);
  append_u16_le (bytes, 16);
  g_byte_array_append (bytes, (const guint8 *) "data", 4);
  append_u32_le (bytes, data_length);
  if (length > 0)
    g_byte_array_append (bytes, pcm, length);

  return g_byte_array_free_to_bytes (g_steal_pointer (&bytes));
}

char *
xd_voice_transcript_parse (const guint8 *json,
                           gsize         length,
                           GError      **error)
{
  g_autoptr (JsonParser) parser = json_parser_new ();
  JsonObject *root;
  const char *text;
  char *result;

  g_return_val_if_fail (json != NULL || length == 0, NULL);

  if (!json_parser_load_from_data (
        parser, (const char *) json, (gssize) length, error))
    return NULL;

  if (!JSON_NODE_HOLDS_OBJECT (json_parser_get_root (parser)))
    {
      g_set_error_literal (error, XD_VOICE_DATA_ERROR, 1,
                           "Transcription service returned invalid JSON.");
      return NULL;
    }

  root = json_node_get_object (json_parser_get_root (parser));
  if (json_object_has_member (root, "error") &&
      JSON_NODE_HOLDS_OBJECT (json_object_get_member (root, "error")))
    {
      JsonObject *service_error = json_object_get_object_member (root, "error");
      const char *message =
        json_object_get_string_member_with_default (
          service_error, "message", "Transcription failed.");

      g_set_error_literal (error, XD_VOICE_DATA_ERROR, 1, message);
      return NULL;
    }

  text = json_object_get_string_member_with_default (root, "text", NULL);
  if (text == NULL)
    {
      g_set_error_literal (error, XD_VOICE_DATA_ERROR, 1,
                           "Transcription response contained no text.");
      return NULL;
    }

  result = g_strdup (text);
  g_strstrip (result);
  if (*result == '\0')
    {
      g_free (result);
      g_set_error_literal (error, XD_VOICE_DATA_ERROR, 1,
                           "No speech was detected.");
      return NULL;
    }

  return result;
}
