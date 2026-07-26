#pragma once

#include <glib.h>

G_BEGIN_DECLS

/* Name of the per-folder settings file, stored inside each workspace folder. */
#define XD_FOLDER_SETTINGS_FILE ".xd.json"

/*
 * Settings attached to one folder.
 *
 * Every field except `id` may be NULL, which means "inherit from the parent
 * folder". `id` is a UUID minted when the folder is first seen; chats reference
 * it instead of a path, so renaming or moving a folder never orphans them.
 */
typedef struct
{
  char *id;
  char *backend;      /* "claude" | "codex" */
  char *model;
  char *workdir;
  char *repo;
  char *instructions;
} XdFolderSettings;

XdFolderSettings *xd_folder_settings_new       (void);
void              xd_folder_settings_free      (XdFolderSettings *self);

/* Reads .xd.json. Returns NULL and sets @error when it is missing or invalid. */
XdFolderSettings *xd_folder_settings_load      (const char        *folder_path,
                                                GError           **error);

/* Like load(), but writes a fresh file with a new UUID when none exists. */
XdFolderSettings *xd_folder_settings_ensure    (const char        *folder_path,
                                                GError           **error);

gboolean          xd_folder_settings_save      (const XdFolderSettings *self,
                                                const char             *folder_path,
                                                GError                **error);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdFolderSettings, xd_folder_settings_free)

G_END_DECLS
