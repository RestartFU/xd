#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define XD_TYPE_APPLICATION (xd_application_get_type ())
G_DECLARE_FINAL_TYPE (XdApplication, xd_application, XD, APPLICATION, AdwApplication)

XdApplication *xd_application_new (void);

/* Shared, application-wide settings ("com.restartfu.Xd"). */
GSettings *xd_application_get_settings (XdApplication *self);

G_END_DECLS
