#pragma once

#include <gio/gio.h>

G_BEGIN_DECLS

#define XD_TYPE_VOICE_RECORDER (xd_voice_recorder_get_type ())
G_DECLARE_FINAL_TYPE (XdVoiceRecorder, xd_voice_recorder,
                      XD, VOICE_RECORDER, GObject)

XdVoiceRecorder *xd_voice_recorder_new           (void);
void             xd_voice_recorder_record_async  (XdVoiceRecorder    *self,
                                                   GCancellable       *cancellable,
                                                   GAsyncReadyCallback callback,
                                                   gpointer            user_data);
GBytes          *xd_voice_recorder_record_finish (XdVoiceRecorder    *self,
                                                   GAsyncResult       *result,
                                                   GError            **error);
void             xd_voice_recorder_stop          (XdVoiceRecorder    *self);

void  xd_voice_transcribe_async  (GBytes             *wav,
                                  const char         *api_key,
                                  GCancellable       *cancellable,
                                  GAsyncReadyCallback callback,
                                  gpointer            user_data);
char *xd_voice_transcribe_finish (GAsyncResult       *result,
                                  GError            **error);

G_END_DECLS
