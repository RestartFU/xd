#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define HY_TYPE_APPLICATION (hy_application_get_type ())
G_DECLARE_FINAL_TYPE (HyApplication, hy_application, HY, APPLICATION, AdwApplication)

HyApplication *hy_application_new (void);

/* Shared, application-wide settings ("com.restartfu.Hy"). */
GSettings *hy_application_get_settings (HyApplication *self);

G_END_DECLS
