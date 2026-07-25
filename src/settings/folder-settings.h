#pragma once

#include <glib.h>

G_BEGIN_DECLS

/* Name of the per-folder settings file, stored inside each workspace folder. */
#define HY_FOLDER_SETTINGS_FILE ".hy.json"

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
} HyFolderSettings;

HyFolderSettings *hy_folder_settings_new       (void);
void              hy_folder_settings_free      (HyFolderSettings *self);

/* Reads .hy.json. Returns NULL and sets @error when it is missing or invalid. */
HyFolderSettings *hy_folder_settings_load      (const char        *folder_path,
                                                GError           **error);

/* Like load(), but writes a fresh file with a new UUID when none exists. */
HyFolderSettings *hy_folder_settings_ensure    (const char        *folder_path,
                                                GError           **error);

gboolean          hy_folder_settings_save      (const HyFolderSettings *self,
                                                const char             *folder_path,
                                                GError                **error);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (HyFolderSettings, hy_folder_settings_free)

G_END_DECLS
