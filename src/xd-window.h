#pragma once

#include <adwaita.h>

#include "xd-app.h"

G_BEGIN_DECLS

#define XD_TYPE_WINDOW (xd_window_get_type ())
G_DECLARE_FINAL_TYPE (XdWindow, xd_window, XD, WINDOW, AdwApplicationWindow)

XdWindow *xd_window_new (XdApplication *app);

G_END_DECLS
