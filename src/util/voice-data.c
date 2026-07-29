#include "voice-data.h"

#include <string.h>

#define VOICE_MODEL_SIZE   G_GUINT64_CONSTANT (574041195)
#define VOICE_MODEL_SHA256 \
  "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"

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

static guint16
read_u16_le (const guint8 *bytes)
{
  guint16 value;

  memcpy (&value, bytes, sizeof value);
  return GUINT16_FROM_LE (value);
}

static guint32
read_u32_le (const guint8 *bytes)
{
  guint32 value;

  memcpy (&value, bytes, sizeof value);
  return GUINT32_FROM_LE (value);
}

GBytes *
xd_voice_wav_to_f32 (GBytes  *wav,
                     GError **error)
{
  gsize length = 0;
  const guint8 *bytes;
  guint32 data_length;
  gsize sample_count;
  float *samples;

  g_return_val_if_fail (wav != NULL, NULL);

  bytes = g_bytes_get_data (wav, &length);
  if (length < 44 ||
      memcmp (bytes, "RIFF", 4) != 0 ||
      memcmp (bytes + 8, "WAVEfmt ", 8) != 0 ||
      read_u32_le (bytes + 16) != 16 ||
      read_u16_le (bytes + 20) != 1 ||
      read_u16_le (bytes + 22) != 1 ||
      read_u32_le (bytes + 24) != 16000 ||
      read_u16_le (bytes + 34) != 16 ||
      memcmp (bytes + 36, "data", 4) != 0)
    {
      g_set_error_literal (error, XD_VOICE_DATA_ERROR, 1,
                           "Recorded audio has an invalid WAV header.");
      return NULL;
    }

  data_length = read_u32_le (bytes + 40);
  if ((data_length & 1) != 0 || data_length > length - 44)
    {
      g_set_error_literal (error, XD_VOICE_DATA_ERROR, 1,
                           "Recorded audio data is truncated.");
      return NULL;
    }

  sample_count = data_length / 2;
  samples = g_new (float, sample_count);
  for (gsize i = 0; i < sample_count; i++)
    {
      gint16 sample;

      memcpy (&sample, bytes + 44 + i * 2, sizeof sample);
      sample = GINT16_FROM_LE (sample);
      samples[i] = sample / 32768.0f;
    }

  return g_bytes_new_take (samples, sample_count * sizeof *samples);
}

gboolean
xd_voice_model_metadata_valid (guint64     length,
                               const char *sha256)
{
  return length == VOICE_MODEL_SIZE &&
         g_strcmp0 (sha256, VOICE_MODEL_SHA256) == 0;
}
