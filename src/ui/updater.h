#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

#define XD_TYPE_UPDATER (xd_updater_get_type ())
G_DECLARE_FINAL_TYPE (XdUpdater, xd_updater, XD, UPDATER, AdwBin)

/*
 * A button that appears when there is a newer build, and installs it.
 *
 * Which build it looks for is the build's own: a nightly follows the nightly,
 * a release follows releases. Nothing is announced until there is something to
 * do about it, so the usual state of this widget is invisible.
 *
 * It only offers when it can actually do it: an app started from a checkout is
 * not the copy the installer would replace, and updating it would put a new
 * build somewhere the person running this one would not see it.
 */
XdUpdater *xd_updater_new (void);

G_END_DECLS
