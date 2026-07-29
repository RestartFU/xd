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

#define XD_TYPE_VOICE_MODEL (xd_voice_model_get_type ())
G_DECLARE_FINAL_TYPE (XdVoiceModel, xd_voice_model,
                      XD, VOICE_MODEL, GObject)

XdVoiceModel *xd_voice_model_new             (void);
char         *xd_voice_model_find            (void);
guint         xd_voice_model_get_progress    (XdVoiceModel       *self);
void          xd_voice_model_ensure_async    (XdVoiceModel       *self,
                                               GCancellable       *cancellable,
                                               GAsyncReadyCallback callback,
                                               gpointer            user_data);
char         *xd_voice_model_ensure_finish   (XdVoiceModel       *self,
                                               GAsyncResult       *result,
                                               GError            **error);

void  xd_voice_transcribe_async  (GBytes             *wav,
                                  const char         *model_path,
                                  GCancellable       *cancellable,
                                  GAsyncReadyCallback callback,
                                  gpointer            user_data);
char *xd_voice_transcribe_finish (GAsyncResult       *result,
                                  GError            **error);

G_END_DECLS
