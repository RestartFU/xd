#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

#define XD_VOICE_DATA_ERROR (xd_voice_data_error_quark ())

GQuark xd_voice_data_error_quark (void);

GBytes *xd_voice_wav_from_s16       (const guint8 *pcm,
                                     gsize         length,
                                     guint         sample_rate,
                                     guint         channels);
char   *xd_voice_transcript_parse   (const guint8 *json,
                                     gsize         length,
                                     GError      **error);

G_END_DECLS
