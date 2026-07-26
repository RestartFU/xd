#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Where this build keeps things.
 *
 * A nightly is built with a different data directory, application id and
 * settings path from a release, so the two can be installed at once without
 * either one editing the other's chats -- which is what a nightly is for. The
 * name comes from the build (XD_DATA_NAME); nothing here decides it.
 */

/* $XDG_DATA_HOME/xd, or xd-nightly. Created if it is not there. */
const char *xd_app_data_dir        (void);

/* The chat database, inside it. */
char       *xd_app_database_path   (void);

/*
 * The workspace tree.
 *
 * Under the data directory rather than in the home directory: it is the app's
 * own storage, it belongs beside the database that refers to it, and a nightly
 * having its own is the whole point. A tree left in ~/Workspaces by an earlier
 * version is moved across on first use -- a folder's id lives in a dotfile
 * inside it, so nothing that refers to one is lost by moving it.
 */
char       *xd_app_workspaces_root (void);

G_END_DECLS
