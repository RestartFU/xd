#include "util/voice-data.h"

#include <string.h>

static guint16
read_u16 (const guint8 *bytes)
{
  guint16 value;

  memcpy (&value, bytes, sizeof value);
  return GUINT16_FROM_LE (value);
}

static guint32
read_u32 (const guint8 *bytes)
{
  guint32 value;

  memcpy (&value, bytes, sizeof value);
  return GUINT32_FROM_LE (value);
}

static void
test_wav_header (void)
{
  static const guint8 pcm[] = { 1, 2, 3, 4 };
  g_autoptr (GBytes) wav = xd_voice_wav_from_s16 (pcm, sizeof pcm, 16000, 1);
  gsize length;
  const guint8 *bytes = g_bytes_get_data (wav, &length);

  g_assert_cmpuint (length, ==, 48);
  g_assert_cmpmem (bytes, 4, "RIFF", 4);
  g_assert_cmpuint (read_u32 (bytes + 4), ==, 40);
  g_assert_cmpmem (bytes + 8, 8, "WAVEfmt ", 8);
  g_assert_cmpuint (read_u32 (bytes + 16), ==, 16);
  g_assert_cmpuint (read_u16 (bytes + 20), ==, 1);
  g_assert_cmpuint (read_u16 (bytes + 22), ==, 1);
  g_assert_cmpuint (read_u32 (bytes + 24), ==, 16000);
  g_assert_cmpuint (read_u32 (bytes + 28), ==, 32000);
  g_assert_cmpuint (read_u16 (bytes + 32), ==, 2);
  g_assert_cmpuint (read_u16 (bytes + 34), ==, 16);
  g_assert_cmpmem (bytes + 36, 4, "data", 4);
  g_assert_cmpuint (read_u32 (bytes + 40), ==, 4);
  g_assert_cmpmem (bytes + 44, 4, pcm, sizeof pcm);
}

static void
test_transcript (void)
{
  static const char json[] = "{\"text\":\"  Fix café parsing.  \"}";
  g_autoptr (GError) error = NULL;
  g_autofree char *text =
    xd_voice_transcript_parse ((const guint8 *) json, strlen (json), &error);

  g_assert_no_error (error);
  g_assert_cmpstr (text, ==, "Fix café parsing.");
}

static void
test_service_error (void)
{
  static const char json[] =
    "{\"error\":{\"message\":\"API key is invalid.\"}}";
  g_autoptr (GError) error = NULL;
  g_autofree char *text =
    xd_voice_transcript_parse ((const guint8 *) json, strlen (json), &error);

  g_assert_null (text);
  g_assert_error (error, XD_VOICE_DATA_ERROR, 1);
  g_assert_cmpstr (error->message, ==, "API key is invalid.");
}

static void
test_empty_transcript (void)
{
  static const char json[] = "{\"text\":\"  \"}";
  g_autoptr (GError) error = NULL;
  g_autofree char *text =
    xd_voice_transcript_parse ((const guint8 *) json, strlen (json), &error);

  g_assert_null (text);
  g_assert_error (error, XD_VOICE_DATA_ERROR, 1);
  g_assert_cmpstr (error->message, ==, "No speech was detected.");
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/voice-data/wav-header", test_wav_header);
  g_test_add_func ("/voice-data/transcript", test_transcript);
  g_test_add_func ("/voice-data/service-error", test_service_error);
  g_test_add_func ("/voice-data/empty", test_empty_transcript);

  return g_test_run ();
}
