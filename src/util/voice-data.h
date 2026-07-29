#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

#define XD_VOICE_DATA_ERROR (xd_voice_data_error_quark ())

GQuark xd_voice_data_error_quark (void);

GBytes  *xd_voice_wav_from_s16          (const guint8 *pcm,
                                         gsize         length,
                                         guint         sample_rate,
                                         guint         channels);
GBytes  *xd_voice_wav_to_f32            (GBytes       *wav,
                                         GError      **error);
gboolean xd_voice_model_metadata_valid  (guint64       length,
                                         const char   *sha256);

G_END_DECLS
