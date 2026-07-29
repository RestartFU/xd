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
test_wav_to_f32 (void)
{
  static const guint8 pcm[] = { 0x00, 0x80, 0x00, 0x00, 0xff, 0x7f };
  g_autoptr (GBytes) wav =
    xd_voice_wav_from_s16 (pcm, sizeof pcm, 16000, 1);
  g_autoptr (GError) error = NULL;
  g_autoptr (GBytes) converted = xd_voice_wav_to_f32 (wav, &error);
  gsize length = 0;
  const float *samples = g_bytes_get_data (converted, &length);

  g_assert_no_error (error);
  g_assert_cmpuint (length, ==, 3 * sizeof *samples);
  g_assert_cmpfloat (samples[0], ==, -1.0f);
  g_assert_cmpfloat (samples[1], ==, 0.0f);
  g_assert_cmpfloat_with_epsilon (samples[2], 32767.0f / 32768.0f, 0.00001f);
}

static void
test_invalid_wav (void)
{
  g_autoptr (GBytes) wav = g_bytes_new_static ("not a wav", 9);
  g_autoptr (GError) error = NULL;
  g_autoptr (GBytes) converted = xd_voice_wav_to_f32 (wav, &error);

  g_assert_null (converted);
  g_assert_error (error, XD_VOICE_DATA_ERROR, 1);
}

static void
test_model_metadata (void)
{
  g_assert_true (xd_voice_model_metadata_valid (
    G_GUINT64_CONSTANT (574041195),
    "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"));
  g_assert_false (xd_voice_model_metadata_valid (
    G_GUINT64_CONSTANT (574041194),
    "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"));
  g_assert_false (xd_voice_model_metadata_valid (
    G_GUINT64_CONSTANT (574041195),
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
}

int
main (int   argc,
      char *argv[])
{
  g_test_init (&argc, &argv, NULL);

  g_test_add_func ("/voice-data/wav-header", test_wav_header);
  g_test_add_func ("/voice-data/wav-to-f32", test_wav_to_f32);
  g_test_add_func ("/voice-data/invalid-wav", test_invalid_wav);
  g_test_add_func ("/voice-data/model-metadata", test_model_metadata);

  return g_test_run ();
}
