#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Where a build looks for its next version.
 *
 * Two channels, and the difference between them is one rolling tag: a release
 * is the newest tagged release, a nightly is the rolling "nightly" release
 * built from master. The rolling one is identified by the commit it was built
 * from, since its tag never moves off its own name.
 *
 * The daemon and the update button in the sidebar both do this, and they used
 * to do it twice: the same URLs, the same install line and the same comparison,
 * written out in both places. A channel added to one of them and not the other
 * is a build that checks for updates it will not install.
 *
 * A branch is not a channel. It is built from source by the nightly itself and
 * installed over it -- see util/branch-build.h -- so trying one costs no
 * release, no rolling tag and no third kind of build to keep working.
 */
typedef enum
{
  XD_UPDATE_CHANNEL_RELEASE,
  XD_UPDATE_CHANNEL_NIGHTLY,
} XdUpdateChannel;

/*
 * How often to look, in seconds.
 *
 * Neither channel changes more than a few times a day, and the window and the
 * daemon read the same answer: one of them polling on its own schedule would
 * mean a machine where the client offers a build the daemon has not noticed,
 * or the reverse.
 */
#define XD_UPDATE_POLL_SECONDS (60 * 5)

/* What this build is, from XD_CHANNEL. */
XdUpdateChannel xd_update_channel_current (void);

/* The rolling tag, or NULL for a release, whose tag is its version. */
const char     *xd_update_channel_tag     (XdUpdateChannel channel);

/* What to fetch to find out whether there is a newer build. */
char           *xd_update_channel_check_url (XdUpdateChannel channel);

/* The shell line that installs that release over this build. */
char           *xd_update_channel_install_command (XdUpdateChannel channel);

/*
 * What the release named by @body is called: its commit for the rolling
 * channel, its tag for a release. NULL when the reply is not a release object.
 */
char           *xd_update_channel_latest_from_reply (XdUpdateChannel  channel,
                                                     const char      *body);

/* True when @latest is something other than what is running. */
gboolean        xd_update_channel_is_newer (XdUpdateChannel  channel,
                                            const char      *latest);

G_END_DECLS
