#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Where a build looks for its next version.
 *
 * Three channels, and the difference between them is one rolling tag: a
 * release is the newest tagged release, a nightly is the rolling "nightly"
 * release built from master, and a dev build is the rolling "dev" release a
 * pull request publishes. The two rolling channels are identified by the commit
 * they were built from, since their tag never moves off its own name.
 *
 * The daemon and the update button in the sidebar both do this, and they used
 * to do it twice: the same URLs, the same install line and the same comparison,
 * written out in both places. A channel added to one of them and not the other
 * is a build that checks for updates it will not install.
 */
typedef enum
{
  XD_UPDATE_CHANNEL_RELEASE,
  XD_UPDATE_CHANNEL_NIGHTLY,
  XD_UPDATE_CHANNEL_DEV,
} XdUpdateChannel;

/* What this build is, from XD_CHANNEL. */
XdUpdateChannel xd_update_channel_current (void);

/* The rolling tag, or NULL for a release, whose tag is its version. */
const char     *xd_update_channel_tag     (XdUpdateChannel channel);

/* The GitHub API URL for the release to compare this build against. */
char           *xd_update_channel_check_url (XdUpdateChannel channel);

/* The shell line that installs that release over this build. */
char           *xd_update_channel_install_command (XdUpdateChannel channel);

/*
 * What the release in @json is called: its commit for a rolling channel, its
 * tag for a release. NULL when the reply is not a release object.
 */
char           *xd_update_channel_latest_from_json (XdUpdateChannel  channel,
                                                    const char      *json);

/* True when @latest is something other than what is running. */
gboolean        xd_update_channel_is_newer (XdUpdateChannel  channel,
                                            const char      *latest);

G_END_DECLS
