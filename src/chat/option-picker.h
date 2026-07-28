#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define XD_TYPE_OPTION_PICKER (xd_option_picker_get_type ())
G_DECLARE_FINAL_TYPE (XdOptionPicker, xd_option_picker, XD, OPTION_PICKER, AdwBin)

/*
 * Compact choice button for the composer.
 *
 * The button names the current value. Its popover uses the same rounded,
 * descriptive rows as xd's other pickers instead of GtkDropDown's native
 * list surface.
 */
XdOptionPicker *xd_option_picker_new          (const char *const *labels,
                                               const char *const *descriptions);

void            xd_option_picker_set_choices  (XdOptionPicker    *self,
                                               const char *const *labels,
                                               const char *const *descriptions);
guint           xd_option_picker_get_selected (XdOptionPicker    *self);
void            xd_option_picker_set_selected (XdOptionPicker    *self,
                                               guint              selected);
void            xd_option_picker_set_label    (XdOptionPicker    *self,
                                               guint              position,
                                               const char        *label);

G_END_DECLS
