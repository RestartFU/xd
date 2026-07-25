#pragma once

#include <adwaita.h>

#include "hy-app.h"

G_BEGIN_DECLS

#define HY_TYPE_WINDOW (hy_window_get_type ())
G_DECLARE_FINAL_TYPE (HyWindow, hy_window, HY, WINDOW, AdwApplicationWindow)

HyWindow *hy_window_new (HyApplication *app);

G_END_DECLS
