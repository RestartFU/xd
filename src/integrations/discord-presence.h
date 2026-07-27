#pragma once

#include <glib.h>

G_BEGIN_DECLS

typedef struct _XdDiscordPresence XdDiscordPresence;

XdDiscordPresence *xd_discord_presence_new              (void);
void               xd_discord_presence_set_state        (XdDiscordPresence *self,
                                                          const char        *state);
void               xd_discord_presence_free             (XdDiscordPresence *self);

/*
 * Public for the headless protocol test. The returned JSON is one complete
 * SET_ACTIVITY command and belongs to the caller.
 */
char              *xd_discord_presence_build_activity   (const char *state,
                                                          gint64      started_at,
                                                          guint32     process_id,
                                                          guint64     nonce);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdDiscordPresence, xd_discord_presence_free)

G_END_DECLS
