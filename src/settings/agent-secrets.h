#pragma once

#include <glib.h>

G_BEGIN_DECLS

typedef struct _XdAgentSecrets XdAgentSecrets;

/*
 * Secrets supplied to agent CLI processes as environment variables.
 *
 * @path may be NULL for this build's private per-user store. A missing file is
 * an empty store; malformed or unreadable files are errors rather than a turn
 * silently starting without credentials.
 */
XdAgentSecrets *xd_agent_secrets_load              (const char      *path,
                                                    GError         **error);
/*
 * One folder's private store. Its stable folder id is hashed into a filename
 * beside the global store, so values never enter the workspace and renaming or
 * moving the folder does not lose them.
 */
XdAgentSecrets *xd_agent_secrets_load_for_folder   (const char      *folder_id,
                                                    GError         **error);
/*
 * Global values plus @folder_ids in outermost-to-innermost order. A nearer
 * folder replaces a value of the same name.
 */
XdAgentSecrets *xd_agent_secrets_load_effective    (const char *const *folder_ids,
                                                    GError         **error);
void            xd_agent_secrets_free              (XdAgentSecrets *self);
gboolean        xd_agent_secret_name_is_valid      (const char      *name);
GStrv           xd_agent_secrets_names             (XdAgentSecrets *self);
gboolean        xd_agent_secrets_contains          (XdAgentSecrets *self,
                                                    const char      *name);
gboolean        xd_agent_secrets_set               (XdAgentSecrets *self,
                                                    const char      *name,
                                                    const char      *value,
                                                    GError         **error);
void            xd_agent_secrets_remove            (XdAgentSecrets *self,
                                                    const char      *name);
gboolean        xd_agent_secrets_save               (XdAgentSecrets *self,
                                                    GError         **error);

/* @environment is updated in place and returned for g_auto(GStrv). */
GStrv           xd_agent_secrets_apply_environment (XdAgentSecrets *self,
                                                    GStrv           environment);

/* Names only. Values never enter model context. NULL when the store is empty. */
char           *xd_agent_secrets_prompt             (XdAgentSecrets *self);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdAgentSecrets, xd_agent_secrets_free)

G_END_DECLS
