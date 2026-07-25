#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define HY_TYPE_MODEL_PICKER (hy_model_picker_get_type ())
G_DECLARE_FINAL_TYPE (HyModelPicker, hy_model_picker, HY, MODEL_PICKER, AdwBin)

/*
 * Chooses which assistant and model answers a chat.
 *
 * Both are picked together, because a model only means anything to the CLI it
 * belongs to. Emits ::model-chosen with the backend id and the model id, the
 * latter NULL for "whatever the CLI is configured to use".
 */
HyModelPicker *hy_model_picker_new          (void);

void           hy_model_picker_set_selected (HyModelPicker *self,
                                             const char    *backend_id,
                                             const char    *model_id);

G_END_DECLS
