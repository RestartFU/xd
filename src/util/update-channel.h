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

/*
 * What to fetch to find out whether there is a newer build.
 *
 * The API for a release or a nightly, and for a dev build the commit written
 * beside its bundle: GitHub allows sixty unauthenticated API requests an hour,
 * and a dev build looks every twenty-five seconds from both the window and the
 * daemon. Asking the API that often would spend most of the hour refused --
 * a release asset is served past that limit, and one line of it is the answer.
 */
char           *xd_update_channel_check_url (XdUpdateChannel channel);

/*
 * How often to look, in seconds.
 *
 * A dev build exists to try a branch that is being pushed to while someone
 * waits for it, so it looks often enough that the build arrives while they are
 * still looking. A nightly or a release is not being watched like that, and
 * asking GitHub every few minutes is already more often than either changes.
 */
guint           xd_update_channel_poll_seconds (XdUpdateChannel channel);

/* The shell line that installs that release over this build. */
char           *xd_update_channel_install_command (XdUpdateChannel channel);

/*
 * What the release named by @body is called: its commit for a rolling channel,
 * its tag for a release. NULL when the reply is not something this channel can
 * read -- a dev build reads a commit, the others a release object.
 */
char           *xd_update_channel_latest_from_reply (XdUpdateChannel  channel,
                                                     const char      *body);

/* True when @latest is something other than what is running. */
gboolean        xd_update_channel_is_newer (XdUpdateChannel  channel,
                                            const char      *latest);

G_END_DECLS
